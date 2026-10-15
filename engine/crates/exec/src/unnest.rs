use std::sync::Arc;

use arrow::array::{Array, ArrayRef, Int64Array, new_null_array};
use arrow::compute::concat;
use arrow::datatypes::{DataType, Field, Schema, SchemaRef};
use arrow::record_batch::{RecordBatch, RecordBatchOptions};
use kaveon_core::{BatchOperator, Expr, KaveonError, Result};

/// Expands constant ARRAY expressions into rows, padding shorter arrays with
/// nulls as Trino does for multi-array UNNEST.
///
/// The planner keeps this operator deliberately small: it is a leaf relation,
/// so there is no input batch or lateral binding to resolve. The distributed
/// planner can therefore run it as an ordinary zero-input fragment and joins
/// can consume it through the existing exchange path.
pub struct UnnestOperator {
    schema: SchemaRef,
    values: Vec<ArrayRef>,
    ordinality: bool,
    ordinality_start: i64,
    emitted: bool,
}

impl UnnestOperator {
    pub fn new(
        expressions: &[Expr],
        columns: Vec<String>,
        ordinality: bool,
        ordinality_column: impl Into<String>,
        ordinality_start: i64,
    ) -> Result<Self> {
        let empty = RecordBatch::try_new_with_options(
            Arc::new(Schema::empty()),
            Vec::new(),
            &RecordBatchOptions::new().with_row_count(Some(1)),
        )?;
        if expressions.is_empty() || expressions.len() != columns.len() {
            return Err(KaveonError::Execution(
                "UNNEST requires one output column for each ARRAY expression".into(),
            ));
        }
        let mut values = Vec::with_capacity(expressions.len());
        let mut fields = Vec::with_capacity(columns.len() + usize::from(ordinality));
        for (expression, column) in expressions.iter().zip(columns) {
            let evaluated = crate::expr_eval::evaluate(expression, &empty)?;
            let (element_type, value) = match evaluated.data_type() {
                DataType::List(field) | DataType::LargeList(field) => {
                    if evaluated.len() != 1 {
                        return Err(KaveonError::Execution(
                            "UNNEST expression must produce one array".into(),
                        ));
                    }
                    let value = evaluated
                        .as_any()
                        .downcast_ref::<arrow::array::GenericListArray<i32>>()
                        .map(|list| list.value(0))
                        .or_else(|| {
                            evaluated
                                .as_any()
                                .downcast_ref::<arrow::array::GenericListArray<i64>>()
                                .map(|list| list.value(0))
                        })
                        .ok_or_else(|| {
                            KaveonError::Execution(
                                "UNNEST ARRAY value has an incompatible list representation".into(),
                            )
                        })?;
                    (field.data_type().clone(), value)
                }
                other => {
                    return Err(KaveonError::Execution(format!(
                        "UNNEST requires an ARRAY value, found {other}"
                    )));
                }
            };
            fields.push(Field::new(column, element_type, true));
            values.push(value);
        }
        let row_count = values.iter().map(|value| value.len()).max().unwrap_or(0);
        let values = values
            .into_iter()
            .map(|value| {
                if value.len() == row_count {
                    Ok(value)
                } else {
                    let missing = row_count.saturating_sub(value.len());
                    let nulls = new_null_array(value.data_type(), missing);
                    concat(&[value.as_ref(), nulls.as_ref()]).map_err(Into::into)
                }
            })
            .collect::<Result<Vec<_>>>()?;
        let ordinality_column = ordinality_column.into();
        if ordinality {
            fields.push(Field::new(ordinality_column, DataType::Int64, false));
        }
        Ok(Self {
            schema: Arc::new(Schema::new(fields)),
            values,
            ordinality,
            ordinality_start,
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
        let mut columns = self.values.clone();
        if self.ordinality {
            columns.push(Arc::new(Int64Array::from_iter_values(
                self.ordinality_start
                    ..self.ordinality_start
                        + self.values.first().map_or(0, |value| value.len()) as i64,
            )) as ArrayRef);
        }
        Ok(Some(RecordBatch::try_new(
            Arc::clone(&self.schema),
            columns,
        )?))
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
        let mut operator =
            UnnestOperator::new(&[expression], vec!["value".into()], true, "n", 1).unwrap();
        let batch = operator.next_batch().unwrap().unwrap();
        assert_eq!(batch.num_rows(), 2);
        assert_eq!(batch.schema().field(0).data_type(), &DataType::Int64);
        assert_eq!(batch.schema().field(1).name(), "n");
        assert_eq!(
            batch
                .column(0)
                .as_any()
                .downcast_ref::<Int64Array>()
                .unwrap()
                .values(),
            &[4, 9]
        );
        assert_eq!(
            batch
                .column(1)
                .as_any()
                .downcast_ref::<Int64Array>()
                .unwrap()
                .values(),
            &[1, 2]
        );
        assert!(operator.next_batch().unwrap().is_none());
    }

    #[test]
    fn pads_multiple_arrays_to_the_longest_input() {
        let array = |values: &[i64]| Expr::Function {
            name: "ARRAY".into(),
            args: values
                .iter()
                .copied()
                .map(kaveon_core::ScalarValue::Int64)
                .map(Expr::Literal)
                .collect(),
        };
        let mut operator = UnnestOperator::new(
            &[array(&[1, 2, 3]), array(&[10, 20])],
            vec!["left".into(), "right".into()],
            false,
            "ordinality",
            1,
        )
        .unwrap();
        let batch = operator.next_batch().unwrap().unwrap();
        assert_eq!(batch.num_rows(), 3);
        assert!(batch.column(1).is_null(2));
    }

    #[test]
    fn emits_zero_based_offsets() {
        let expression = Expr::Function {
            name: "ARRAY".into(),
            args: vec![
                Expr::Literal(kaveon_core::ScalarValue::Int64(4)),
                Expr::Literal(kaveon_core::ScalarValue::Int64(9)),
            ],
        };
        let mut operator =
            UnnestOperator::new(&[expression], vec!["value".into()], true, "offset", 0).unwrap();
        let batch = operator.next_batch().unwrap().unwrap();
        assert_eq!(
            batch
                .column(1)
                .as_any()
                .downcast_ref::<Int64Array>()
                .unwrap()
                .values(),
            &[0, 1]
        );
    }
}
