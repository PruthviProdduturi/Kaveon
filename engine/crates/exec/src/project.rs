use arrow::datatypes::{Field, Schema, SchemaRef};
use arrow::record_batch::RecordBatch;
use kaveon_core::{BatchOperator, Expr, KaveonError, Result};
use std::sync::Arc;

use crate::expr_eval::evaluate;

pub struct ProjectOperator {
    source: Box<dyn BatchOperator>,
    exprs: Vec<Expr>,
    output_schema: SchemaRef,
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
        })
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

        let columns: Vec<_> = self
            .exprs
            .iter()
            .map(|expr| evaluate(unaliased(expr), &batch))
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
            Ok((
                Field::new(output_name, arrow::datatypes::DataType::Float64, true),
                None,
            ))
        }
        Expr::Star => Err(KaveonError::Execution(
            "star should be expanded before projection".into(),
        )),
        Expr::Literal(val) => Ok((Field::new(format!("{val:?}"), val.data_type(), true), None)),
        Expr::BinaryOp { .. } => Ok((
            Field::new(
                format!("{expr:?}"),
                arrow::datatypes::DataType::Float64,
                true,
            ),
            None,
        )),
        _ => Ok((
            Field::new(
                format!("{expr:?}"),
                arrow::datatypes::DataType::Boolean,
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
