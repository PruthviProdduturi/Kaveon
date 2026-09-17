use arrow::datatypes::{Field, Schema, SchemaRef};
use arrow::record_batch::RecordBatch;
use kaveon_core::{BatchOperator, Expr, KaveonError, Result};
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
}
