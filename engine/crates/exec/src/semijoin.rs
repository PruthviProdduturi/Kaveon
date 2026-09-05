use std::collections::HashSet;

use arrow::array::{Array, ArrayRef, AsArray, RecordBatch, StringArray};
use arrow::datatypes::{Float64Type, Int32Type, Int64Type, SchemaRef, UInt64Type};
use kaveon_core::{BatchOperator, Expr, KaveonError, Result};

use crate::aggregate::AggregateValue;

pub struct SemiJoinOperator {
    left: Box<dyn BatchOperator>,
    right: Box<dyn BatchOperator>,
    left_key: Expr,
    right_key: Expr,
    anti: bool,
    right_keys: Option<HashSet<AggregateValue>>,
}

impl SemiJoinOperator {
    pub fn new(
        left: Box<dyn BatchOperator>,
        right: Box<dyn BatchOperator>,
        left_key: Expr,
        right_key: Expr,
        anti: bool,
    ) -> Self {
        Self {
            left,
            right,
            left_key,
            right_key,
            anti,
            right_keys: None,
        }
    }

    fn build_right_keys(&mut self) -> Result<()> {
        let mut keys = HashSet::new();
        while let Some(batch) = self.right.next_batch()? {
            let col = crate::expr_eval::evaluate(&self.right_key, &batch)?;
            for row in 0..batch.num_rows() {
                if !col.is_null(row) {
                    keys.insert(extract_value(col.as_ref(), row)?);
                }
            }
        }
        self.right_keys = Some(keys);
        Ok(())
    }
}

impl BatchOperator for SemiJoinOperator {
    fn schema(&self) -> &SchemaRef {
        self.left.schema()
    }

    fn next_batch(&mut self) -> Result<Option<RecordBatch>> {
        if self.right_keys.is_none() {
            self.build_right_keys()?;
        }
        let keys = self.right_keys.as_ref().unwrap();

        loop {
            let batch = match self.left.next_batch()? {
                Some(b) => b,
                None => return Ok(None),
            };

            let col = crate::expr_eval::evaluate(&self.left_key, &batch)?;
            let mut indices = Vec::new();
            for row in 0..batch.num_rows() {
                if col.is_null(row) {
                    if self.anti {
                        indices.push(row);
                    }
                    continue;
                }
                let val = extract_value(col.as_ref(), row)?;
                let found = keys.contains(&val);
                if (found && !self.anti) || (!found && self.anti) {
                    indices.push(row);
                }
            }

            if indices.is_empty() {
                continue;
            }

            let idx_array = arrow::array::UInt32Array::from(
                indices.iter().map(|&i| i as u32).collect::<Vec<_>>(),
            );
            let columns: Vec<ArrayRef> = batch
                .columns()
                .iter()
                .map(|c| arrow::compute::take(c.as_ref(), &idx_array, None))
                .collect::<std::result::Result<_, _>>()
                .map_err(|e| KaveonError::Execution(format!("semi-join take: {e}")))?;

            let result = RecordBatch::try_new(batch.schema(), columns)
                .map_err(|e| KaveonError::Execution(format!("semi-join batch: {e}")))?;
            return Ok(Some(result));
        }
    }
}

fn extract_value(array: &dyn Array, row: usize) -> Result<AggregateValue> {
    if array.is_null(row) {
        return Ok(AggregateValue::Null);
    }
    match array.data_type() {
        arrow::datatypes::DataType::Boolean => {
            Ok(AggregateValue::Bool(array.as_boolean().value(row)))
        }
        arrow::datatypes::DataType::Int32 => Ok(AggregateValue::Int32(
            array.as_primitive::<Int32Type>().value(row),
        )),
        arrow::datatypes::DataType::Int64 => Ok(AggregateValue::Int64(
            array.as_primitive::<Int64Type>().value(row),
        )),
        arrow::datatypes::DataType::UInt64 => Ok(AggregateValue::Int64(
            array.as_primitive::<UInt64Type>().value(row) as i64,
        )),
        arrow::datatypes::DataType::Float64 => {
            let v = array.as_primitive::<Float64Type>().value(row);
            Ok(AggregateValue::Float64Bits(v.to_bits()))
        }
        arrow::datatypes::DataType::Utf8 => Ok(AggregateValue::Utf8(
            array
                .as_any()
                .downcast_ref::<StringArray>()
                .expect("Utf8")
                .value(row)
                .to_owned(),
        )),
        dt => Err(KaveonError::Execution(format!(
            "unsupported type for semi-join key: {dt}"
        ))),
    }
}
