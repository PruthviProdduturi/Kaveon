use std::sync::Arc;

use arrow::array::{Array, ArrayRef, Int64Array};
use arrow::datatypes::{DataType, Field, Schema, SchemaRef};
use arrow::record_batch::{RecordBatch, RecordBatchOptions};
use kaveon_core::{BatchOperator, Expr, KaveonError, Result};

/// Expands one constant ARRAY expression into rows.
///
/// The planner keeps this operator deliberately small: it is a leaf relation,
/// so there is no input batch or lateral binding to resolve. The distributed
/// planner can therefore run it as an ordinary zero-input fragment and joins
/// can consume it through the existing exchange path.
pub struct UnnestOperator {
    schema: SchemaRef,
    values: ArrayRef,
    ordinality: bool,
    emitted: bool,
}

impl UnnestOperator {
    pub fn new(expression: &Expr, column: impl Into<String>, ordinality: bool) -> Result<Self> {
        let empty = RecordBatch::try_new_with_options(
            Arc::new(Schema::empty()),
            Vec::new(),
            &RecordBatchOptions::new().with_row_count(Some(1)),
        )?;
        let values = crate::expr_eval::evaluate(expression, &empty)?;
        let (element_type, values) = match values.data_type() {
            DataType::List(field) | DataType::LargeList(field) => {
                if values.len() != 1 {
                    return Err(KaveonError::Execution(
                        "UNNEST expression must produce one array".into(),
                    ));
                }
                let list = values
                    .as_any()
                    .downcast_ref::<arrow::array::GenericListArray<i32>>()
                    .map(|list| list.value(0))
                    .or_else(|| {
                        values
                            .as_any()
                            .downcast_ref::<arrow::array::GenericListArray<i64>>()
                            .map(|list| list.value(0))
                    })
                    .ok_or_else(|| {
                        KaveonError::Execution(
                            "UNNEST ARRAY value has an incompatible list representation".into(),
                        )
                    })?;
                (field.data_type().clone(), list)
            }
            other => {
                return Err(KaveonError::Execution(format!(
                    "UNNEST requires an ARRAY value, found {other}"
                )))
            }
        };
        let column = column.into();
        let mut fields = vec![Field::new(column, element_type, true)];
        if ordinality {
            fields.push(Field::new("ordinality", DataType::Int64, false));
        }
        Ok(Self {
            schema: Arc::new(Schema::new(fields)),
            values,
            ordinality,
            emitted: false,
        })
    }
}

impl BatchOperator for UnnestOperator {
    fn schema(&self) -> &SchemaRef {
        &self.schema
    }

    fn next_batch(&mut self) -> Result<Option<RecordBatch>> {
        if self.emitted {
            return Ok(None);
        }
        self.emitted = true;
        let mut columns = vec![Arc::clone(&self.values)];
        if self.ordinality {
            columns.push(Arc::new(Int64Array::from_iter_values(
                1..=self.values.len() as i64,
            )) as ArrayRef);
        }
        Ok(Some(RecordBatch::try_new(Arc::clone(&self.schema), columns)?))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use arrow::array::{Array, Int64Array};

    #[test]
    fn expands_constant_array_with_ordinality() {
        let expression = Expr::Function {
            name: "ARRAY".into(),
            args: vec![
                Expr::Literal(kaveon_core::ScalarValue::Int64(4)),
                Expr::Literal(kaveon_core::ScalarValue::Int64(9)),
            ],
        };
        let mut operator = UnnestOperator::new(&expression, "value", true).unwrap();
        let batch = operator.next_batch().unwrap().unwrap();
        assert_eq!(batch.num_rows(), 2);
        assert_eq!(batch.schema().field(0).data_type(), &DataType::Int64);
        assert_eq!(batch.column(0).as_any().downcast_ref::<Int64Array>().unwrap().values(), &[4, 9]);
        assert_eq!(batch.column(1).as_any().downcast_ref::<Int64Array>().unwrap().values(), &[1, 2]);
        assert!(operator.next_batch().unwrap().is_none());
    }
}
