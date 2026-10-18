use std::sync::Arc;

use arrow::array::{Array, ArrayRef, Int64Array, StructArray, new_null_array};
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
        if expressions.is_empty() || columns.is_empty() {
            return Err(KaveonError::Execution(
                "UNNEST requires at least one ARRAY expression and output column".into(),
            ));
        }
        let mut values = Vec::with_capacity(columns.len());
        let mut fields = Vec::with_capacity(columns.len() + usize::from(ordinality));
        let mut column_index = 0;
        for expression in expressions {
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
            if let DataType::Struct(struct_fields) = &element_type {
                let row = value
                    .as_any()
                    .downcast_ref::<StructArray>()
                    .ok_or_else(|| {
                        KaveonError::Execution(
                            "UNNEST ROW value has an incompatible struct representation".into(),
                        )
                    })?;
                let width = struct_fields.len();
                if columns.len().saturating_sub(column_index) < width {
                    return Err(KaveonError::Execution(format!(
                        "UNNEST ROW value has {width} fields but only {} output columns were provided",
                        columns.len().saturating_sub(column_index)
                    )));
                }
                for field_index in 0..width {
                    let child = row.column(field_index).clone();
                    let child = if row.null_count() > 0 {
                        let valid = arrow::compute::is_not_null(row)?;
                        let nulls = new_null_array(child.data_type(), child.len());
                        arrow::compute::kernels::zip::zip(&valid, &child, &nulls)?
                    } else {
                        child
                    };
                    fields.push(Field::new(
                        &columns[column_index],
                        struct_fields[field_index].data_type().clone(),
                        true,
                    ));
                    values.push(child);
                    column_index += 1;
                }
            } else {
                let column = columns.get(column_index).ok_or_else(|| {
                    KaveonError::Execution(
                        "UNNEST has more ARRAY expressions than output columns".into(),
                    )
                })?;
                fields.push(Field::new(column, element_type, true));
                values.push(value);
                column_index += 1;
            }
        }
        if column_index != columns.len() {
            return Err(KaveonError::Execution(format!(
                "UNNEST has {} unused output columns",
                columns.len() - column_index
            )));
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

    #[test]
    fn expands_sequence_array_constructor() {
        let expression = Expr::Function {
            name: "SEQUENCE".into(),
            args: vec![
                Expr::Literal(kaveon_core::ScalarValue::Int64(2)),
                Expr::Literal(kaveon_core::ScalarValue::Int64(6)),
            ],
        };
        let mut operator =
            UnnestOperator::new(&[expression], vec!["value".into()], false, "ordinality", 1)
                .unwrap();
        let batch = operator.next_batch().unwrap().unwrap();
        assert_eq!(batch.num_rows(), 5);
        assert_eq!(
            batch
                .column(0)
                .as_any()
                .downcast_ref::<Int64Array>()
                .unwrap()
                .values(),
            &[2, 3, 4, 5, 6]
        );
    }

    #[test]
    fn expands_array_mutators_as_row_producing_inputs() {
        let array = Expr::Function {
            name: "ARRAY".into(),
            args: vec![
                Expr::Literal(kaveon_core::ScalarValue::Int64(1)),
                Expr::Literal(kaveon_core::ScalarValue::Int64(2)),
            ],
        };
        for (name, expected) in [
            ("ARRAY_APPEND", vec![1, 2, 3]),
            ("ARRAY_PREPEND", vec![3, 1, 2]),
            ("ARRAY_REMOVE", vec![1, 2]),
        ] {
            let expression = Expr::Function {
                name: name.into(),
                args: vec![
                    array.clone(),
                    Expr::Literal(kaveon_core::ScalarValue::Int64(3)),
                ],
            };
            let mut operator =
                UnnestOperator::new(&[expression], vec!["value".into()], false, "ord", 1).unwrap();
            let batch = operator.next_batch().unwrap().unwrap();
            assert_eq!(batch.num_rows(), expected.len(), "{name}");
            assert_eq!(
                batch
                    .column(0)
                    .as_any()
                    .downcast_ref::<Int64Array>()
                    .unwrap()
                    .values(),
                expected.as_slice(),
                "{name}"
            );
        }
    }
}
