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
        Expr::Column(name) => {
            let field = schema
                .field_with_name(name)
                .map_err(|_| {
                    KaveonError::Execution(format!("projection column '{name}' not in input"))
                })?
                .clone();
            Ok((field, None))
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
