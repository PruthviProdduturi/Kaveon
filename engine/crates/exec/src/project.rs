use arrow::datatypes::{DataType, Field, Schema, SchemaRef};
use arrow::record_batch::RecordBatch;
use kaveon_core::{BatchOperator, Expr, KaveonError, Result, ScalarValue, StoragePredicate};
use std::sync::Arc;

use crate::expr_eval::evaluate;

pub struct ProjectOperator {
    source: Box<dyn BatchOperator>,
    exprs: Vec<Expr>,
    output_schema: SchemaRef,
    memory: Option<kaveon_core::OperatorMemoryAccount>,
}

impl ProjectOperator {
    pub fn new(source: Box<dyn BatchOperator>, exprs: Vec<Expr>) -> Result<Self> {
        let source_schema = source.schema().clone();

        let mut fields = Vec::with_capacity(exprs.len());
        for expr in &exprs {
            let (field, _) = resolve_field(expr, &source_schema)?;
            fields.push(field);
        }

        let output_schema = Arc::new(Schema::new(fields));

        Ok(Self {
            source,
            exprs,
            output_schema,
            memory: None,
        })
    }

    pub fn with_memory(mut self, memory: kaveon_core::OperatorMemoryAccount) -> Self {
        self.memory = Some(memory);
        self
    }
}

impl BatchOperator for ProjectOperator {
    fn schema(&self) -> &SchemaRef {
        &self.output_schema
    }

    fn apply_runtime_predicate(&mut self, predicate: &StoragePredicate) -> Result<bool> {
        // A runtime join predicate can cross a projection only when every
        // referenced output column is a direct source column (possibly with
        // an alias). Rewriting only that identity subset keeps computed
        // projections exact while allowing storage pruning through common
        // SELECT key AS alias shapes.
        let Some(predicate) = self.rewrite_runtime_predicate(predicate) else {
            return Ok(false);
        };
        self.source.apply_runtime_predicate(&predicate)
    }

    fn next_batch(&mut self) -> Result<Option<RecordBatch>> {
        let Some(batch) = self.source.next_batch()? else {
            return Ok(None);
        };

        let _workspace = self
            .memory
            .as_ref()
            .map(|memory| memory.reserve((batch.get_array_memory_size() as u64).saturating_mul(2)))
            .transpose()?;
        let mut reservations = Vec::new();
        let columns: Vec<_> = self
            .exprs
            .iter()
            .map(|expr| {
                let column =
                    crate::expr_eval::with_expression_memory(self.memory.as_ref(), || {
                        evaluate(unaliased(expr), &batch)
                    })?;
                if let Some(memory) = &self.memory {
                    reservations.push(memory.reserve(column.get_array_memory_size() as u64)?);
                }
                Ok(column)
            })
            .collect::<Result<_>>()?;

        let projected = RecordBatch::try_new(self.output_schema.clone(), columns)?;
        Ok(Some(projected))
    }
}

impl ProjectOperator {
    fn source_column_for_output(&self, output: &str) -> Option<String> {
        self.output_schema
            .fields()
            .iter()
            .zip(&self.exprs)
            .find(|(field, _)| field.name() == output)
            .and_then(|(_, expr)| match unaliased(expr) {
                Expr::Column(column) => Some(column.clone()),
                _ => None,
            })
    }

    fn rewrite_runtime_predicate(&self, predicate: &StoragePredicate) -> Option<StoragePredicate> {
        let column = |name: &str| self.source_column_for_output(name);
        Some(match predicate {
            StoragePredicate::Compare {
                column: name,
                op,
                value,
            } => StoragePredicate::Compare {
                column: column(name)?,
                op: *op,
                value: value.clone(),
            },
            StoragePredicate::IsNull { column: name } => StoragePredicate::IsNull {
                column: column(name)?,
            },
            StoragePredicate::IsNotNull { column: name } => StoragePredicate::IsNotNull {
                column: column(name)?,
            },
            StoragePredicate::In {
                column: name,
                values,
            } => StoragePredicate::In {
                column: column(name)?,
                values: values.clone(),
            },
            StoragePredicate::Like {
                column: name,
                pattern,
                negated,
                case_insensitive,
            } => StoragePredicate::Like {
                column: column(name)?,
                pattern: pattern.clone(),
                negated: *negated,
                case_insensitive: *case_insensitive,
            },
            StoragePredicate::And(children) => StoragePredicate::And(
                children
                    .iter()
                    .map(|child| self.rewrite_runtime_predicate(child))
                    .collect::<Option<Vec<_>>>()?,
            ),
            StoragePredicate::Or(children) => StoragePredicate::Or(
                children
                    .iter()
                    .map(|child| self.rewrite_runtime_predicate(child))
                    .collect::<Option<Vec<_>>>()?,
            ),
            StoragePredicate::Not(child) => {
                StoragePredicate::Not(Box::new(self.rewrite_runtime_predicate(child)?))
            }
        })
    }
}

fn unaliased(expr: &Expr) -> &Expr {
    match expr {
        Expr::Alias { expr, .. } => unaliased(expr),
        _ => expr,
    }
}

fn resolve_field(expr: &Expr, schema: &SchemaRef) -> Result<(Field, Option<String>)> {
    match expr {
        Expr::WindowFunction { .. } => {
            let name = crate::window::window_output_name(expr);
            let field = schema.field_with_name(&name)?.clone();
            Ok((field, None))
        }
        // A column keeps the name it was written with: `c_name` over a join
        // output holding `customer.c_name` is the field `c_name`.
        Expr::Column(name) => {
            let index = crate::expr_eval::resolve_column_index(schema, name).map_err(|_| {
                KaveonError::Execution(format!("projection column '{name}' not in input"))
            })?;
            Ok((schema.field(index).clone().with_name(name), None))
        }
        Expr::Alias { expr, name } => {
            let (mut field, _) = resolve_field(expr, schema)?;
            field = field.with_name(name);
            Ok((field, Some(name.clone())))
        }
        Expr::Function { name, args } => {
            let output_name = format_function_name(name, args);
            // Composite ROW dereferences are lowered to ROW_FIELD.  Probing
            // the evaluator with an empty batch cannot infer the child type
            // because the evaluator quite correctly returns a typed NULL
            // array for the empty input.  Resolve the child from the source
            // Struct type instead so every fragment, including an empty
            // partition, publishes the same projection schema.
            if name.eq_ignore_ascii_case("ROW_FIELD")
                && args.len() == 2
                && let (Some(DataType::Struct(fields)), Expr::Literal(ScalarValue::Utf8(field))) =
                    (expression_data_type(&args[0], schema), &args[1])
                && let Some(child) = fields
                    .iter()
                    .find(|candidate| candidate.name().eq_ignore_ascii_case(field))
            {
                return Ok((
                    Field::new(output_name, child.data_type().clone(), true),
                    None,
                ));
            }
            // `evaluate` sees an empty batch while the projection schema is
            // built.  A ROW subscript has no rows from which to select its
            // constant field in that probe, so the evaluator correctly
            // returns a Null array; use the source struct type to preserve
            // the actual output type for the subsequent non-empty batches.
            if name.eq_ignore_ascii_case("ELEMENT_AT")
                && args.len() == 2
                && let (Some(DataType::Struct(fields)), Expr::Literal(ScalarValue::Int64(index))) =
                    (expression_data_type(&args[0], schema), &args[1])
                && let Some(field) = index
                    .checked_sub(1)
                    .and_then(|index| usize::try_from(index).ok())
                    .and_then(|index| fields.get(index))
            {
                return Ok((
                    Field::new(output_name, field.data_type().clone(), true),
                    None,
                ));
            }
            // A ZIP_WITH lambda can return a typed value even when the
            // projection probe has zero rows. Infer its element type from
            // the lambda's parameter columns instead of accepting the
            // evaluator's empty-input Null array.
            if name.eq_ignore_ascii_case("ZIP_WITH")
                && let Some(data_type) = expression_data_type(expr, schema)
            {
                return Ok((Field::new(output_name, data_type, true), None));
            }
            if name.eq_ignore_ascii_case("FILTER")
                && let Some(data_type) = expression_data_type(expr, schema)
            {
                return Ok((Field::new(output_name, data_type, true), None));
            }
            if name.eq_ignore_ascii_case("TRANSFORM")
                && let Some(data_type) = expression_data_type(expr, schema)
            {
                return Ok((Field::new(output_name, data_type, true), None));
            }
            if name.eq_ignore_ascii_case("REDUCE")
                && let Some(data_type) = expression_data_type(expr, schema)
            {
                return Ok((Field::new(output_name, data_type, true), None));
            }
            let data_type = evaluate(expr, &RecordBatch::new_empty(schema.clone()))?
                .data_type()
                .clone();
            Ok((Field::new(output_name, data_type, true), None))
        }
        Expr::Star => Err(KaveonError::Execution(
            "star should be expanded before projection".into(),
        )),
        Expr::Literal(val) => Ok((Field::new(format!("{val:?}"), val.data_type(), true), None)),
        Expr::BinaryOp { .. } => Ok((
            Field::new(
                format!("{expr:?}"),
                evaluate(expr, &RecordBatch::new_empty(schema.clone()))?
                    .data_type()
                    .clone(),
                true,
            ),
            None,
        )),
        _ => Ok((
            Field::new(
                format!("{expr:?}"),
                evaluate(expr, &RecordBatch::new_empty(schema.clone()))?
                    .data_type()
                    .clone(),
                true,
            ),
            None,
        )),
    }
}

fn expression_data_type(expr: &Expr, schema: &SchemaRef) -> Option<DataType> {
    match expr {
        Expr::Literal(value) => Some(value.data_type()),
        Expr::Column(name) => crate::expr_eval::resolve_column_index(schema, name)
            .ok()
            .map(|index| schema.field(index).data_type().clone()),
        Expr::Alias { expr, .. } => expression_data_type(expr, schema),
        Expr::Function { name, args } if name.eq_ignore_ascii_case("ROW") => {
            let fields = args
                .iter()
                .enumerate()
                .map(|(index, arg)| {
                    Some(Field::new(
                        format!("field{index}"),
                        expression_data_type(arg, schema)?,
                        true,
                    ))
                })
                .collect::<Option<Vec<_>>>()?;
            Some(DataType::Struct(fields.into()))
        }
        Expr::Function { name, args } if name.eq_ignore_ascii_case("ARRAY") => {
            let element_type = args
                .iter()
                .filter_map(|arg| expression_data_type(arg, schema))
                .find(|data_type| *data_type != DataType::Null)?;
            Some(DataType::List(Arc::new(Field::new(
                "item",
                element_type,
                true,
            ))))
        }
        Expr::Function { name, args }
            if (name.eq_ignore_ascii_case("EUCLIDEAN_DISTANCE")
                || name.eq_ignore_ascii_case("DOT_PRODUCT"))
                && args.len() == 2 =>
        {
            let (DataType::List(_) | DataType::LargeList(_)) =
                expression_data_type(&args[0], schema)?
            else {
                return None;
            };
            let (DataType::List(_) | DataType::LargeList(_)) =
                expression_data_type(&args[1], schema)?
            else {
                return None;
            };
            Some(DataType::Float64)
        }
        Expr::Function { name, args }
            if name.eq_ignore_ascii_case("COMBINATIONS") && args.len() == 2 =>
        {
            let (DataType::List(field) | DataType::LargeList(field)) =
                expression_data_type(&args[0], schema)?
            else {
                return None;
            };
            Some(DataType::List(Arc::new(Field::new(
                "item",
                DataType::List(Arc::new(Field::new(
                    "item",
                    field.data_type().clone(),
                    true,
                ))),
                true,
            ))))
        }
        Expr::Function { name, args }
            if (name.eq_ignore_ascii_case("ARRAY_FIRST")
                || name.eq_ignore_ascii_case("ARRAY_LAST"))
                && args.len() == 2 =>
        {
            let (DataType::List(field) | DataType::LargeList(field)) =
                expression_data_type(&args[0], schema)?
            else {
                return None;
            };
            let Expr::Function {
                name: lambda_name,
                args: lambda_args,
            } = &args[1]
            else {
                return None;
            };
            if lambda_name != "__KAVEON_LAMBDA" || lambda_args.len() != 2 {
                return None;
            }
            let Expr::Literal(ScalarValue::Utf8(parameter)) = &lambda_args[0] else {
                return None;
            };
            let mut fields = vec![Field::new(parameter, field.data_type().clone(), true)];
            fields.extend(
                schema
                    .fields()
                    .iter()
                    .filter(|candidate| candidate.name() != parameter)
                    .map(|field| field.as_ref().clone()),
            );
            (expression_data_type(&lambda_args[1], &Arc::new(Schema::new(fields)))?
                == DataType::Boolean)
                .then_some(field.data_type().clone())
        }
        Expr::Function { name, args } if name.eq_ignore_ascii_case("MAP") && args.len() == 2 => {
            let (DataType::List(keys) | DataType::LargeList(keys)) =
                expression_data_type(&args[0], schema)?
            else {
                return None;
            };
            let (DataType::List(values) | DataType::LargeList(values)) =
                expression_data_type(&args[1], schema)?
            else {
                return None;
            };
            let entries = DataType::Struct(
                vec![
                    Field::new("key", keys.data_type().clone(), false),
                    Field::new("value", values.data_type().clone(), true),
                ]
                .into(),
            );
            Some(DataType::Map(
                Arc::new(Field::new("entries", entries, false)),
                false,
            ))
        }
        Expr::Function { name, args }
            if name.eq_ignore_ascii_case("ROW_FIELD") && args.len() == 2 =>
        {
            let DataType::Struct(fields) = expression_data_type(&args[0], schema)? else {
                return None;
            };
            let Expr::Literal(ScalarValue::Utf8(field)) = &args[1] else {
                return None;
            };
            fields
                .iter()
                .find(|candidate| candidate.name().eq_ignore_ascii_case(field))
                .map(|candidate| candidate.data_type().clone())
        }
        Expr::Function { name, args }
            if (name.eq_ignore_ascii_case("MAP_ELEMENT_AT")
                || name.eq_ignore_ascii_case("ELEMENT_AT"))
                && args.len() == 2 =>
        {
            let DataType::Map(entries, _) = expression_data_type(&args[0], schema)? else {
                return None;
            };
            let DataType::Struct(fields) = entries.data_type() else {
                return None;
            };
            fields.get(1).map(|field| field.data_type().clone())
        }
        Expr::Function { name, args }
            if name.eq_ignore_ascii_case("ZIP_WITH") && args.len() == 3 =>
        {
            let left = expression_data_type(&args[0], schema)?;
            let right = expression_data_type(&args[1], schema)?;
            let left_type = match left {
                DataType::List(field) | DataType::LargeList(field) => field.data_type().clone(),
                _ => return None,
            };
            let right_type = match right {
                DataType::List(field) | DataType::LargeList(field) => field.data_type().clone(),
                _ => return None,
            };
            let Expr::Function {
                name: lambda_name,
                args: lambda_args,
            } = &args[2]
            else {
                return None;
            };
            if lambda_name != "__KAVEON_LAMBDA" || lambda_args.len() != 3 {
                return None;
            }
            let (
                Expr::Literal(ScalarValue::Utf8(left_name)),
                Expr::Literal(ScalarValue::Utf8(right_name)),
            ) = (&lambda_args[0], &lambda_args[1])
            else {
                return None;
            };
            if left_name == right_name {
                return None;
            }
            let mut fields = vec![
                Field::new(left_name, left_type, true),
                Field::new(right_name, right_type, true),
            ];
            fields.extend(
                schema
                    .fields()
                    .iter()
                    .filter(|field| field.name() != left_name && field.name() != right_name)
                    .map(|field| field.as_ref().clone()),
            );
            let element_type =
                expression_data_type(&lambda_args[2], &Arc::new(Schema::new(fields)))?;
            Some(DataType::List(Arc::new(Field::new(
                "item",
                element_type,
                true,
            ))))
        }
        Expr::Function { name, args } if name.eq_ignore_ascii_case("FILTER") && args.len() == 2 => {
            let data_type = expression_data_type(&args[0], schema)?;
            let element_type = match data_type {
                DataType::List(field) | DataType::LargeList(field) => field.data_type().clone(),
                _ => return None,
            };
            let Expr::Function {
                name: lambda_name,
                args: lambda_args,
            } = &args[1]
            else {
                return None;
            };
            if lambda_name != "__KAVEON_LAMBDA" || lambda_args.len() != 2 {
                return None;
            }
            let Expr::Literal(ScalarValue::Utf8(parameter)) = &lambda_args[0] else {
                return None;
            };
            let mut fields = vec![Field::new(parameter, element_type.clone(), true)];
            fields.extend(
                schema
                    .fields()
                    .iter()
                    .filter(|field| field.name() != parameter)
                    .map(|field| field.as_ref().clone()),
            );
            let predicate_type =
                expression_data_type(&lambda_args[1], &Arc::new(Schema::new(fields)))?;
            if predicate_type != DataType::Boolean {
                return None;
            }
            Some(DataType::List(Arc::new(Field::new(
                "item",
                element_type,
                true,
            ))))
        }
        Expr::Function { name, args }
            if name.eq_ignore_ascii_case("TRANSFORM") && args.len() == 2 =>
        {
            let data_type = expression_data_type(&args[0], schema)?;
            let element_type = match data_type {
                DataType::List(field) | DataType::LargeList(field) => field.data_type().clone(),
                _ => return None,
            };
            let Expr::Function {
                name: lambda_name,
                args: lambda_args,
            } = &args[1]
            else {
                return None;
            };
            if lambda_name != "__KAVEON_LAMBDA" || lambda_args.len() != 2 {
                return None;
            }
            let Expr::Literal(ScalarValue::Utf8(parameter)) = &lambda_args[0] else {
                return None;
            };
            let mut fields = vec![Field::new(parameter, element_type, true)];
            fields.extend(
                schema
                    .fields()
                    .iter()
                    .filter(|field| field.name() != parameter)
                    .map(|field| field.as_ref().clone()),
            );
            let result_type =
                expression_data_type(&lambda_args[1], &Arc::new(Schema::new(fields)))?;
            Some(DataType::List(Arc::new(Field::new(
                "item",
                result_type,
                true,
            ))))
        }
        Expr::Function { name, args }
            if matches!(
                name.to_ascii_uppercase().as_str(),
                "ALL_MATCH" | "ANY_MATCH" | "NONE_MATCH"
            ) && args.len() == 2 =>
        {
            let data_type = expression_data_type(&args[0], schema)?;
            let element_type = match data_type {
                DataType::List(field) | DataType::LargeList(field) => field.data_type().clone(),
                _ => return None,
            };
            let Expr::Function {
                name: lambda_name,
                args: lambda_args,
            } = &args[1]
            else {
                return None;
            };
            if lambda_name != "__KAVEON_LAMBDA" || lambda_args.len() != 2 {
                return None;
            }
            let Expr::Literal(ScalarValue::Utf8(parameter)) = &lambda_args[0] else {
                return None;
            };
            let mut fields = vec![Field::new(parameter, element_type, true)];
            fields.extend(
                schema
                    .fields()
                    .iter()
                    .filter(|field| field.name() != parameter)
                    .map(|field| field.as_ref().clone()),
            );
            (expression_data_type(&lambda_args[1], &Arc::new(Schema::new(fields)))?
                == DataType::Boolean)
                .then_some(DataType::Boolean)
        }
        Expr::Function { name, args }
            if name.eq_ignore_ascii_case("MAP_FILTER") && args.len() == 2 =>
        {
            let DataType::Map(entry_field, sorted) = expression_data_type(&args[0], schema)? else {
                return None;
            };
            let Expr::Function {
                name: lambda_name,
                args: lambda_args,
            } = &args[1]
            else {
                return None;
            };
            if lambda_name != "__KAVEON_LAMBDA" || lambda_args.len() != 3 {
                return None;
            }
            let DataType::Struct(entry_fields) = entry_field.data_type() else {
                return None;
            };
            if entry_fields.len() != 2 {
                return None;
            }
            let (
                Expr::Literal(ScalarValue::Utf8(key_name)),
                Expr::Literal(ScalarValue::Utf8(value_name)),
            ) = (&lambda_args[0], &lambda_args[1])
            else {
                return None;
            };
            if key_name == value_name {
                return None;
            }
            let mut fields = vec![
                Field::new(key_name, entry_fields[0].data_type().clone(), false),
                Field::new(value_name, entry_fields[1].data_type().clone(), true),
            ];
            fields.extend(
                schema
                    .fields()
                    .iter()
                    .filter(|field| field.name() != key_name && field.name() != value_name)
                    .map(|field| field.as_ref().clone()),
            );
            if expression_data_type(&lambda_args[2], &Arc::new(Schema::new(fields)))
                != Some(DataType::Boolean)
            {
                return None;
            }
            Some(DataType::Map(entry_field, sorted))
        }
        Expr::Function { name, args }
            if name.eq_ignore_ascii_case("TRANSFORM_VALUES") && args.len() == 2 =>
        {
            let DataType::Map(entry_field, sorted) = expression_data_type(&args[0], schema)? else {
                return None;
            };
            let Expr::Function {
                name: lambda_name,
                args: lambda_args,
            } = &args[1]
            else {
                return None;
            };
            if lambda_name != "__KAVEON_LAMBDA" || lambda_args.len() != 3 {
                return None;
            }
            let DataType::Struct(entry_fields) = entry_field.data_type() else {
                return None;
            };
            if entry_fields.len() != 2 {
                return None;
            }
            let (
                Expr::Literal(ScalarValue::Utf8(key_name)),
                Expr::Literal(ScalarValue::Utf8(value_name)),
            ) = (&lambda_args[0], &lambda_args[1])
            else {
                return None;
            };
            if key_name == value_name {
                return None;
            }
            let mut fields = vec![
                Field::new(key_name, entry_fields[0].data_type().clone(), false),
                Field::new(value_name, entry_fields[1].data_type().clone(), true),
            ];
            fields.extend(
                schema
                    .fields()
                    .iter()
                    .filter(|field| field.name() != key_name && field.name() != value_name)
                    .map(|field| field.as_ref().clone()),
            );
            let result_type =
                expression_data_type(&lambda_args[2], &Arc::new(Schema::new(fields)))?;
            let result_fields: arrow::datatypes::Fields = vec![
                Arc::new(entry_fields[0].as_ref().clone()),
                Arc::new(Field::new("value", result_type, true)),
            ]
            .into();
            Some(DataType::Map(
                Arc::new(Field::new(
                    "entries",
                    DataType::Struct(result_fields),
                    false,
                )),
                sorted,
            ))
        }
        Expr::Function { name, args }
            if name.eq_ignore_ascii_case("TRANSFORM_KEYS") && args.len() == 2 =>
        {
            let DataType::Map(entry_field, sorted) = expression_data_type(&args[0], schema)? else {
                return None;
            };
            let Expr::Function {
                name: lambda_name,
                args: lambda_args,
            } = &args[1]
            else {
                return None;
            };
            if lambda_name != "__KAVEON_LAMBDA" || lambda_args.len() != 3 {
                return None;
            }
            let DataType::Struct(entry_fields) = entry_field.data_type() else {
                return None;
            };
            if entry_fields.len() != 2 {
                return None;
            }
            let (
                Expr::Literal(ScalarValue::Utf8(key_name)),
                Expr::Literal(ScalarValue::Utf8(value_name)),
            ) = (&lambda_args[0], &lambda_args[1])
            else {
                return None;
            };
            if key_name == value_name {
                return None;
            }
            let mut fields = vec![
                Field::new(key_name, entry_fields[0].data_type().clone(), false),
                Field::new(value_name, entry_fields[1].data_type().clone(), true),
            ];
            fields.extend(
                schema
                    .fields()
                    .iter()
                    .filter(|field| field.name() != key_name && field.name() != value_name)
                    .map(|field| field.as_ref().clone()),
            );
            let result_type =
                expression_data_type(&lambda_args[2], &Arc::new(Schema::new(fields)))?;
            if result_type == DataType::Null {
                return None;
            }
            let result_fields: arrow::datatypes::Fields = vec![
                Arc::new(Field::new("key", result_type, false)),
                Arc::new(entry_fields[1].as_ref().clone()),
            ]
            .into();
            Some(DataType::Map(
                Arc::new(Field::new(
                    "entries",
                    DataType::Struct(result_fields),
                    false,
                )),
                sorted,
            ))
        }
        Expr::Function { name, args }
            if name.eq_ignore_ascii_case("MAP_ZIP_WITH") && args.len() == 3 =>
        {
            let DataType::Map(left_entry, left_sorted) = expression_data_type(&args[0], schema)?
            else {
                return None;
            };
            let DataType::Map(right_entry, right_sorted) = expression_data_type(&args[1], schema)?
            else {
                return None;
            };
            let Expr::Function {
                name: lambda_name,
                args: lambda_args,
            } = &args[2]
            else {
                return None;
            };
            if lambda_name != "__KAVEON_LAMBDA" || lambda_args.len() != 4 {
                return None;
            }
            let (DataType::Struct(left_fields), DataType::Struct(right_fields)) =
                (left_entry.data_type(), right_entry.data_type())
            else {
                return None;
            };
            if left_fields.len() != 2
                || right_fields.len() != 2
                || left_fields[0].data_type() != right_fields[0].data_type()
            {
                return None;
            }
            let (
                Expr::Literal(ScalarValue::Utf8(key_name)),
                Expr::Literal(ScalarValue::Utf8(left_name)),
                Expr::Literal(ScalarValue::Utf8(right_name)),
            ) = (&lambda_args[0], &lambda_args[1], &lambda_args[2])
            else {
                return None;
            };
            if [key_name, left_name, right_name]
                .iter()
                .enumerate()
                .any(|(index, name)| [key_name, left_name, right_name][..index].contains(name))
            {
                return None;
            }
            let mut fields = vec![
                Field::new(key_name, left_fields[0].data_type().clone(), false),
                Field::new(left_name, left_fields[1].data_type().clone(), true),
                Field::new(right_name, right_fields[1].data_type().clone(), true),
            ];
            fields.extend(
                schema
                    .fields()
                    .iter()
                    .filter(|field| {
                        ![key_name, left_name, right_name]
                            .iter()
                            .any(|name| field.name() == *name)
                    })
                    .map(|field| field.as_ref().clone()),
            );
            let result_type =
                expression_data_type(&lambda_args[3], &Arc::new(Schema::new(fields)))?;
            if result_type == DataType::Null {
                return None;
            }
            let result_fields: arrow::datatypes::Fields = vec![
                Arc::new(left_fields[0].as_ref().clone()),
                Arc::new(Field::new("value", result_type, true)),
            ]
            .into();
            Some(DataType::Map(
                Arc::new(Field::new(
                    "entries",
                    DataType::Struct(result_fields),
                    false,
                )),
                left_sorted && right_sorted,
            ))
        }
        Expr::Function { name, args } if name.eq_ignore_ascii_case("REDUCE") && args.len() == 4 => {
            let state_type = expression_data_type(&args[1], schema)?;
            let Expr::Function {
                name: lambda_name,
                args: lambda_args,
            } = &args[3]
            else {
                return None;
            };
            if lambda_name != "__KAVEON_LAMBDA" || lambda_args.len() != 2 {
                return None;
            }
            let Expr::Literal(ScalarValue::Utf8(parameter)) = &lambda_args[0] else {
                return None;
            };
            let mut fields = vec![Field::new(parameter, state_type, true)];
            fields.extend(
                schema
                    .fields()
                    .iter()
                    .filter(|field| field.name() != parameter)
                    .map(|field| field.as_ref().clone()),
            );
            expression_data_type(&lambda_args[1], &Arc::new(Schema::new(fields)))
        }
        Expr::BinaryOp { left, op, right } => {
            if matches!(
                op,
                kaveon_core::BinaryOp::Eq
                    | kaveon_core::BinaryOp::Ne
                    | kaveon_core::BinaryOp::Lt
                    | kaveon_core::BinaryOp::Le
                    | kaveon_core::BinaryOp::Gt
                    | kaveon_core::BinaryOp::Ge
            ) {
                return Some(DataType::Boolean);
            }
            let left = expression_data_type(left, schema)?;
            let right = expression_data_type(right, schema)?;
            if left == right {
                Some(left)
            } else if matches!(left, DataType::Float64) || matches!(right, DataType::Float64) {
                Some(DataType::Float64)
            } else {
                None
            }
        }
        _ => None,
    }
}

fn format_function_name(name: &str, args: &[Expr]) -> String {
    let arg_names: Vec<String> = args
        .iter()
        .map(|a| match a {
            Expr::Column(c) => c.clone(),
            Expr::Star => "*".into(),
            _ => "expr".into(),
        })
        .collect();
    format!("{}({})", name.to_lowercase(), arg_names.join(", "))
}

#[cfg(test)]
mod tests {
    use super::*;
    use arrow::array::{Int64Array, StringArray};
    use arrow::datatypes::DataType;
    use kaveon_core::collect_batches;

    struct Once(Option<RecordBatch>, SchemaRef);

    impl BatchOperator for Once {
        fn schema(&self) -> &SchemaRef {
            &self.1
        }

        fn next_batch(&mut self) -> Result<Option<RecordBatch>> {
            Ok(self.0.take())
        }
    }

    fn joined() -> Box<dyn BatchOperator> {
        let schema = Arc::new(Schema::new(vec![
            Field::new("customer.c_custkey", DataType::Int64, true),
            Field::new("customer.c_name", DataType::Utf8, true),
            Field::new("orders.o_custkey", DataType::Int64, true),
            Field::new("orders.o_comment", DataType::Utf8, true),
        ]));
        let batch = RecordBatch::try_new(
            schema.clone(),
            vec![
                Arc::new(Int64Array::from(vec![1, 2])),
                Arc::new(StringArray::from(vec!["a", "b"])),
                Arc::new(Int64Array::from(vec![1, 2])),
                Arc::new(StringArray::from(vec!["x", "y"])),
            ],
        )
        .unwrap();
        Box::new(Once(Some(batch), schema))
    }

    #[test]
    fn bare_columns_reach_qualified_join_outputs_and_keep_their_written_name() {
        let mut project = ProjectOperator::new(
            joined(),
            vec![
                Expr::Column("c_name".into()),
                Expr::Column("orders.o_comment".into()),
                Expr::Alias {
                    expr: Box::new(Expr::Column("o_custkey".into())),
                    name: "key".into(),
                },
            ],
        )
        .unwrap();
        let names = project
            .schema()
            .fields()
            .iter()
            .map(|field| field.name().clone())
            .collect::<Vec<_>>();
        assert_eq!(names, vec!["c_name", "orders.o_comment", "key"]);
        let batches = collect_batches(&mut project).unwrap();
        assert_eq!(batches[0].num_rows(), 2);
        assert_eq!(
            batches[0]
                .column(2)
                .as_any()
                .downcast_ref::<Int64Array>()
                .unwrap()
                .value(1),
            2
        );
    }

    #[test]
    fn an_ambiguous_bare_column_is_refused() {
        let schema = Arc::new(Schema::new(vec![
            Field::new("n1.n_name", DataType::Utf8, true),
            Field::new("n2.n_name", DataType::Utf8, true),
        ]));
        let error = ProjectOperator::new(
            Box::new(Once(None, schema)),
            vec![Expr::Column("n_name".into())],
        )
        .err()
        .expect("ambiguous");
        assert!(error.to_string().contains("n_name"), "{error}");
    }

    #[test]
    fn runtime_predicate_rewrites_through_identity_alias_only() {
        let project = ProjectOperator::new(
            joined(),
            vec![Expr::Alias {
                expr: Box::new(Expr::Column("orders.o_custkey".into())),
                name: "customer_key".into(),
            }],
        )
        .unwrap();
        let predicate = StoragePredicate::In {
            column: "customer_key".into(),
            values: vec![ScalarValue::Int64(2)],
        };
        assert_eq!(
            project.rewrite_runtime_predicate(&predicate),
            Some(StoragePredicate::In {
                column: "orders.o_custkey".into(),
                values: vec![ScalarValue::Int64(2)],
            })
        );
        let computed = Expr::Function {
            name: "abs".into(),
            args: vec![Expr::Column("orders.o_custkey".into())],
        };
        let project = ProjectOperator::new(
            joined(),
            vec![Expr::Alias {
                expr: Box::new(computed),
                name: "customer_key".into(),
            }],
        )
        .unwrap();
        assert!(project.rewrite_runtime_predicate(&predicate).is_none());
    }
}
