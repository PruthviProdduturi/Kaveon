use std::collections::HashSet;

use arrow::array::{Array, AsArray, RecordBatch};
use arrow::datatypes::{DataType, SchemaRef};
use kaveon_core::{BatchOperator, KaveonError, MemoryReservation, OperatorMemoryAccount, Result};

use crate::aggregate::AggregateValue;

pub struct DistinctOperator {
    source: Box<dyn BatchOperator>,
    seen: HashSet<Vec<AggregateValue>>,
    memory: Option<OperatorMemoryAccount>,
    reservations: Vec<MemoryReservation>,
}

impl DistinctOperator {
    pub fn new(source: Box<dyn BatchOperator>) -> Self {
        Self {
            source,
            seen: HashSet::new(),
            memory: None,
            reservations: Vec::new(),
        }
    }

    pub fn with_memory(mut self, memory: OperatorMemoryAccount) -> Self {
        self.memory = Some(memory);
        self
    }
}

impl BatchOperator for DistinctOperator {
    fn schema(&self) -> &SchemaRef {
        self.source.schema()
    }

    fn next_batch(&mut self) -> Result<Option<RecordBatch>> {
        loop {
            let Some(batch) = self.source.next_batch()? else {
                self.seen = HashSet::new();
                self.reservations.clear();
                return Ok(None);
            };
            let _workspace = self
                .memory
                .as_ref()
                .map(|memory| {
                    memory.reserve(
                        (batch.get_array_memory_size() as u64)
                            .saturating_mul(3)
                            .saturating_add((batch.num_rows() as u64).saturating_mul(32)),
                    )
                })
                .transpose()?;
            let num_cols = batch.num_columns();
            let num_rows = batch.num_rows();

            let mut keep = Vec::new();
            for row in 0..num_rows {
                let key: Vec<AggregateValue> = (0..num_cols)
                    .map(|col| extract_value(batch.column(col), row))
                    .collect::<Result<_>>()?;
                if !self.seen.contains(&key) {
                    if let Some(memory) = &self.memory {
                        let bytes = key.iter().fold(128_u64, |bytes, value| {
                            bytes.saturating_add(64).saturating_add(match value {
                                AggregateValue::Utf8(value) => value.len() as u64,
                                _ => 0,
                            })
                        });
                        self.reservations.push(memory.reserve(bytes)?);
                    }
                    self.seen.insert(key);
                    keep.push(row as u32);
                }
            }

            if keep.is_empty() {
                continue;
            }

            let indices = arrow::array::UInt32Array::from(keep);
            let columns = batch
                .columns()
                .iter()
                .map(|col| {
                    arrow::compute::take(col, &indices, None)
                        .map_err(|e| KaveonError::Execution(format!("distinct take: {e}")))
                })
                .collect::<Result<Vec<_>>>()?;

            return Ok(Some(RecordBatch::try_new(batch.schema(), columns)?));
        }
    }
}

fn extract_value(array: &dyn Array, row: usize) -> Result<AggregateValue> {
    if array.is_null(row) {
        return Ok(AggregateValue::Null);
    }
    match array.data_type() {
        DataType::Boolean => Ok(AggregateValue::Bool(array.as_boolean().value(row))),
        DataType::Int32 => Ok(AggregateValue::Int32(
            array
                .as_primitive::<arrow::datatypes::Int32Type>()
                .value(row),
        )),
        DataType::Int64 => Ok(AggregateValue::Int64(
            array
                .as_primitive::<arrow::datatypes::Int64Type>()
                .value(row),
        )),
        DataType::UInt64 => Ok(AggregateValue::Int64(
            array
                .as_primitive::<arrow::datatypes::UInt64Type>()
                .value(row) as i64,
        )),
        DataType::Float64 => {
            let v = array
                .as_primitive::<arrow::datatypes::Float64Type>()
                .value(row);
            Ok(AggregateValue::Float64Bits(if v == 0.0 {
                0
            } else if v.is_nan() {
                f64::NAN.to_bits()
            } else {
                v.to_bits()
            }))
        }
        DataType::Utf8 => Ok(AggregateValue::Utf8(
            array
                .as_any()
                .downcast_ref::<arrow::array::StringArray>()
                .expect("Utf8 array")
                .value(row)
                .to_owned(),
        )),
        dt => Err(KaveonError::Execution(format!(
            "DISTINCT not supported for type {dt}"
        ))),
    }
}
