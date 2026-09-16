use std::collections::HashSet;

use arrow::array::{Array, AsArray, RecordBatch};
use arrow::datatypes::{DataType, Int32Type, SchemaRef};
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

            // Dictionary columns first collapse to one row per distinct key
            // combination, so the value extraction below touches a handful
            // of rows instead of every row of the batch.
            let candidates =
                distinct_rows_by_code(&batch).unwrap_or_else(|| (0..num_rows as u32).collect());
            let mut keep = Vec::new();
            for row in candidates {
                let row = row as usize;
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

/// The most dictionary cells a batch is coded through; wider combinations
/// fall back to per-row extraction.
const MAX_CODED_CELLS: usize = 1 << 16;

/// When every column is an `Int32`-keyed dictionary, the first row of each
/// distinct key combination in this batch (nulls included), in row order.
fn distinct_rows_by_code(batch: &RecordBatch) -> Option<Vec<u32>> {
    let mut dictionaries = Vec::with_capacity(batch.num_columns());
    let mut cells = 1_usize;
    for column in batch.columns() {
        if !matches!(column.data_type(), DataType::Dictionary(key, _) if key.as_ref() == &DataType::Int32)
        {
            return None;
        }
        let dictionary = column.as_dictionary::<Int32Type>();
        // One extra cell per column stands for its null key.
        cells = cells.checked_mul(dictionary.values().len() + 1)?;
        if cells > MAX_CODED_CELLS {
            return None;
        }
        dictionaries.push(dictionary);
    }
    let mut seen = vec![false; cells];
    let mut rows = Vec::new();
    for row in 0..batch.num_rows() {
        let mut code = 0_usize;
        for dictionary in &dictionaries {
            let width = dictionary.values().len() + 1;
            let keys = dictionary.keys();
            let component = if keys.is_null(row) {
                width - 1
            } else {
                keys.value(row) as usize
            };
            code = code * width + component;
        }
        if !seen[code] {
            seen[code] = true;
            rows.push(row as u32);
        }
    }
    Some(rows)
}

fn extract_value(array: &dyn Array, row: usize) -> Result<AggregateValue> {
    if array.is_null(row) {
        return Ok(AggregateValue::Null);
    }
    match array.data_type() {
        DataType::Dictionary(key_type, _) if key_type.as_ref() == &DataType::Int32 => {
            let dictionary = array.as_dictionary::<Int32Type>();
            extract_value(
                dictionary.values().as_ref(),
                dictionary.keys().value(row) as usize,
            )
        }
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

#[cfg(test)]
mod tests {
    use std::collections::VecDeque;
    use std::sync::Arc;

    use arrow::array::{ArrayRef, DictionaryArray, Int32Array, StringArray};
    use arrow::datatypes::{Field, Schema};

    use super::*;

    struct Input {
        schema: SchemaRef,
        batches: VecDeque<RecordBatch>,
    }

    impl BatchOperator for Input {
        fn schema(&self) -> &SchemaRef {
            &self.schema
        }
        fn next_batch(&mut self) -> Result<Option<RecordBatch>> {
            Ok(self.batches.pop_front())
        }
    }

    fn dictionary(keys: Vec<Option<i32>>, values: &[&str]) -> ArrayRef {
        Arc::new(DictionaryArray::<Int32Type>::new(
            Int32Array::from(keys),
            Arc::new(StringArray::from(values.to_vec())),
        ))
    }

    fn rows(batches: &[RecordBatch]) -> Vec<Vec<Option<String>>> {
        let mut rows = Vec::new();
        for batch in batches {
            let columns: Vec<ArrayRef> = batch
                .columns()
                .iter()
                .map(|column| arrow::compute::cast(column, &DataType::Utf8).unwrap())
                .collect();
            for row in 0..batch.num_rows() {
                rows.push(
                    columns
                        .iter()
                        .map(|column| {
                            let column = column.as_string::<i32>();
                            (!column.is_null(row)).then(|| column.value(row).to_owned())
                        })
                        .collect(),
                );
            }
        }
        rows
    }

    #[test]
    fn dictionary_batches_are_distinct_by_value_across_differing_dictionaries() {
        let dictionary_type =
            DataType::Dictionary(Box::new(DataType::Int32), Box::new(DataType::Utf8));
        let schema = Arc::new(Schema::new(vec![
            Field::new("region", dictionary_type.clone(), true),
            Field::new("platform", dictionary_type, true),
        ]));
        // The same values sit under different keys in the second batch, whose
        // dictionary also carries an entry no row references.
        let first = RecordBatch::try_new(
            Arc::clone(&schema),
            vec![
                dictionary(
                    vec![Some(0), Some(1), Some(0), None, Some(0)],
                    &["Asia", "Europe"],
                ),
                dictionary(
                    vec![Some(0), Some(0), Some(0), Some(1), Some(1)],
                    &["Web", "Mobile"],
                ),
            ],
        )
        .unwrap();
        let second = RecordBatch::try_new(
            Arc::clone(&schema),
            vec![
                dictionary(vec![Some(2), Some(1), None], &["Europe", "Africa", "Asia"]),
                dictionary(vec![Some(1), Some(1), Some(0)], &["Mobile", "Web"]),
            ],
        )
        .unwrap();
        let mut distinct = DistinctOperator::new(Box::new(Input {
            schema,
            batches: VecDeque::from(vec![first, second]),
        }));
        let mut output = Vec::new();
        while let Some(batch) = distinct.next_batch().unwrap() {
            output.push(batch);
        }
        let text = |value: &str| Some(value.to_owned());
        assert_eq!(
            rows(&output),
            vec![
                vec![text("Asia"), text("Web")],
                vec![text("Europe"), text("Web")],
                vec![None, text("Mobile")],
                vec![text("Asia"), text("Mobile")],
                vec![text("Africa"), text("Web")],
            ]
        );
    }

    #[test]
    fn mixed_plain_and_dictionary_columns_take_the_row_path() {
        let schema = Arc::new(Schema::new(vec![
            Field::new(
                "region",
                DataType::Dictionary(Box::new(DataType::Int32), Box::new(DataType::Utf8)),
                true,
            ),
            Field::new("day", DataType::Utf8, true),
        ]));
        let batch = RecordBatch::try_new(
            Arc::clone(&schema),
            vec![
                dictionary(
                    vec![Some(0), Some(0), Some(1), Some(0)],
                    &["Asia", "Europe"],
                ),
                Arc::new(StringArray::from(vec!["d1", "d1", "d1", "d2"])),
            ],
        )
        .unwrap();
        assert!(distinct_rows_by_code(&batch).is_none());
        let mut distinct = DistinctOperator::new(Box::new(Input {
            schema,
            batches: VecDeque::from(vec![batch]),
        }));
        let output = distinct.next_batch().unwrap().unwrap();
        assert_eq!(output.num_rows(), 3);
        assert!(distinct.next_batch().unwrap().is_none());
    }
}
