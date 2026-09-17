use std::collections::HashSet;

use ahash::{AHashMap, AHashSet};
use arrow::array::{Array, ArrayRef, AsArray, RecordBatch};
use arrow::datatypes::{
    DataType, Date32Type, Int8Type, Int16Type, Int32Type, Int64Type, SchemaRef, UInt8Type,
    UInt16Type, UInt32Type, UInt64Type,
};
use kaveon_core::{
    BatchOperator, KaveonError, MemoryReservation, OperatorMemoryAccount, ReservationSlab, Result,
};

use crate::columnar_aggregate::ColumnarGroups;

use crate::aggregate::AggregateValue;

/// A row of up to two columns as fixed-width words plus a null mask: integers
/// and dates as their bits, text through the operator's interner. No
/// allocation per row, one hash per row.
type CompactKey = (u64, u64, u8);
const COMPACT_KEY_BYTES: u64 = 40;

pub struct DistinctOperator {
    source: Box<dyn BatchOperator>,
    seen: HashSet<Vec<AggregateValue>>,
    compact: AHashSet<CompactKey>,
    /// Text values seen so far, each with the word that stands for it in a
    /// compact key; dictionary codes resolve through it per batch, so equal
    /// text compares equal across batches whose dictionaries differ.
    interned: AHashMap<Box<str>, u64>,
    /// The columnar table for every column set the columnar aggregate
    /// carries (up to eight keys of integer, date, boolean, text or
    /// dictionary type): DISTINCT is the grouped aggregate with nothing to
    /// accumulate. `None` before the first batch; `Some(None)` once a
    /// batch showed a shape it does not carry.
    columnar: Option<Option<ColumnarGroups>>,
    growth: Option<MemoryReservation>,
    growth_reserved_at: usize,
    memory: Option<OperatorMemoryAccount>,
    reservations: ReservationSlab,
}

impl DistinctOperator {
    /// The rows of `batch` that are new to this operator, through the
    /// columnar table, or None when the batch has a shape it does not
    /// carry.
    fn keep_by_columnar(&mut self, batch: &RecordBatch) -> Result<Option<Vec<u32>>> {
        if self.columnar.is_none() {
            let types = batch
                .schema()
                .fields()
                .iter()
                .map(|field| field.data_type().clone())
                .collect::<Vec<_>>();
            let carried = !types.is_empty()
                && types.len() <= crate::columnar_aggregate::MAX_KEYS
                && types.iter().all(crate::columnar_aggregate::supports_key);
            self.columnar = Some(if carried {
                ColumnarGroups::new(&types, &[])
            } else {
                None
            });
        }
        let Some(Some(groups)) = self.columnar.as_mut() else {
            return Ok(None);
        };
        let rows = batch.num_rows();
        let slot_bytes = groups.slot_bytes();
        // The batch's worst case — every row a new key, plus its text — is
        // held while it is applied; the actual cost is charged after.
        let worst = if let Some(memory) = &self.memory {
            let doubling = groups.growth_bytes(rows);
            if doubling != 0 && groups.capacity() != self.growth_reserved_at {
                drop(self.growth.take());
                self.growth = Some(memory.reserve(doubling)?);
                self.growth_reserved_at = groups.capacity();
            }
            let key_bytes = batch
                .columns()
                .iter()
                .map(|array| array.get_array_memory_size() as u64)
                .sum::<u64>();
            Some(
                memory.reserve(
                    (rows as u64)
                        .saturating_mul(slot_bytes)
                        .saturating_add(key_bytes),
                )?,
            )
        } else {
            None
        };
        let (kept, new_bytes) = groups.push_batch_new_rows(batch.columns(), rows)?;
        if let Some(memory) = &self.memory {
            self.reservations.reserve(
                memory,
                (kept.len() as u64)
                    .saturating_mul(slot_bytes)
                    .saturating_add(new_bytes),
            )?;
        }
        drop(worst);
        Ok(Some(kept))
    }

    pub fn new(source: Box<dyn BatchOperator>) -> Self {
        Self {
            source,
            seen: HashSet::new(),
            compact: AHashSet::new(),
            interned: AHashMap::new(),
            memory: None,
            columnar: None,
            growth: None,
            growth_reserved_at: 0,
            reservations: ReservationSlab::default(),
        }
    }

    fn intern(&mut self, text: &str) -> Result<u64> {
        if let Some(word) = self.interned.get(text) {
            return Ok(*word);
        }
        if let Some(memory) = &self.memory {
            self.reservations.reserve(memory, 64 + text.len() as u64)?;
        }
        let word = self.interned.len() as u64;
        self.interned.insert(Box::from(text), word);
        Ok(word)
    }

    /// One word per row for a column the compact path can carry, or None
    /// when the column's type needs the general path.
    fn column_words(&mut self, array: &ArrayRef) -> Result<Option<Vec<Option<u64>>>> {
        let rows = array.len();
        macro_rules! primitive {
            ($t:ty) => {{
                let values = array.as_primitive::<$t>();
                Some(
                    (0..rows)
                        .map(|row| (!values.is_null(row)).then(|| values.value(row) as i64 as u64))
                        .collect(),
                )
            }};
        }
        Ok(match array.data_type() {
            DataType::Int8 => primitive!(Int8Type),
            DataType::Int16 => primitive!(Int16Type),
            DataType::Int32 => primitive!(Int32Type),
            DataType::Int64 => primitive!(Int64Type),
            DataType::UInt8 => primitive!(UInt8Type),
            DataType::UInt16 => primitive!(UInt16Type),
            DataType::UInt32 => primitive!(UInt32Type),
            DataType::UInt64 => primitive!(UInt64Type),
            DataType::Date32 => primitive!(Date32Type),
            DataType::Boolean => {
                let values = array.as_boolean();
                Some(
                    (0..rows)
                        .map(|row| (!values.is_null(row)).then(|| values.value(row) as u64))
                        .collect(),
                )
            }
            DataType::Utf8 => {
                let values = array.as_string::<i32>();
                let mut words = Vec::with_capacity(rows);
                for row in 0..rows {
                    words.push(if values.is_null(row) {
                        None
                    } else {
                        Some(self.intern(values.value(row))?)
                    });
                }
                Some(words)
            }
            DataType::Dictionary(key_type, values)
                if key_type.as_ref() == &DataType::Int32 && values.as_ref() == &DataType::Utf8 =>
            {
                let dictionary = array.as_dictionary::<Int32Type>();
                let values = dictionary.values().as_string::<i32>();
                let mut by_code = Vec::with_capacity(values.len());
                for index in 0..values.len() {
                    by_code.push(if values.is_null(index) {
                        None
                    } else {
                        Some(self.intern(values.value(index))?)
                    });
                }
                let keys = dictionary.keys();
                Some(
                    (0..rows)
                        .map(|row| {
                            if keys.is_null(row) {
                                None
                            } else {
                                by_code[keys.value(row) as usize]
                            }
                        })
                        .collect(),
                )
            }
            _ => None,
        })
    }

    /// The rows of `batch` that are new to this operator, through compact
    /// keys, or None when the batch has a shape the compact path does not
    /// carry (more than two columns, or a type it cannot pack).
    fn keep_by_compact_keys(&mut self, batch: &RecordBatch) -> Result<Option<Vec<u32>>> {
        if batch.num_columns() == 0 || batch.num_columns() > 2 {
            return Ok(None);
        }
        let Some(first) = self.column_words(batch.column(0))? else {
            return Ok(None);
        };
        let second = match batch.num_columns() {
            2 => match self.column_words(batch.column(1))? {
                Some(words) => Some(words),
                None => return Ok(None),
            },
            _ => None,
        };
        let mut keep = Vec::new();
        for row in 0..batch.num_rows() {
            let (a, a_null) = first[row].map_or((0, 1), |word| (word, 0));
            let (b, b_null) = second
                .as_ref()
                .map_or((0, 0), |words| words[row].map_or((0, 2), |word| (word, 0)));
            let key = (a, b, a_null | b_null);
            if let Some(memory) = &self.memory
                && self.compact.capacity() >= 1 << 16
                && self.compact.len() == self.compact.capacity()
            {
                // The set doubles; the old table lives until the copy is done.
                self.reservations
                    .reserve(memory, (self.compact.capacity() as u64).saturating_mul(24))?;
            }
            if self.compact.insert(key) {
                if let Some(memory) = &self.memory {
                    self.reservations.reserve(memory, COMPACT_KEY_BYTES)?;
                }
                keep.push(row as u32);
            }
        }
        Ok(Some(keep))
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
                self.compact = AHashSet::new();
                self.interned = AHashMap::new();
                self.columnar = None;
                self.growth = None;
                self.growth_reserved_at = 0;
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

            // Every column set the columnar table carries deduplicates
            // there; a batch of at most two packable columns of other types
            // through compact keys; otherwise dictionary columns first
            // collapse to one row per distinct key combination, so the
            // value extraction below touches a handful of rows instead of
            // every row.
            let kept = match self.keep_by_columnar(&batch)? {
                Some(kept) => Some(kept),
                None => self.keep_by_compact_keys(&batch)?,
            };
            let (mut keep, candidates) = match kept {
                Some(keep) => (keep, Vec::new()),
                None => (
                    Vec::new(),
                    distinct_rows_by_code(&batch).unwrap_or_else(|| (0..num_rows as u32).collect()),
                ),
            };
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
                        self.reservations.reserve(memory, bytes)?;
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
    fn three_columns_of_mixed_types_deduplicate_through_the_columnar_table() {
        // Text, integer and date — more columns than the compact path
        // packs — across batches whose duplicates straddle the boundary:
        // one row per distinct triple, in first-seen order, nulls distinct
        // from values; and a column type the table does not carry falls
        // back to the compact path with the same answer.
        let schema = Arc::new(Schema::new(vec![
            Field::new("phone", DataType::Utf8, true),
            Field::new("user", DataType::Int64, false),
            Field::new("day", DataType::Date32, false),
        ]));
        let batch = |phones: Vec<Option<&str>>, users: Vec<i64>, days: Vec<i32>| {
            RecordBatch::try_new(
                schema.clone(),
                vec![
                    Arc::new(StringArray::from(phones)),
                    Arc::new(arrow::array::Int64Array::from(users)),
                    Arc::new(arrow::array::Date32Array::from(days)),
                ],
            )
            .unwrap()
        };
        let first = batch(
            vec![Some("a"), Some("a"), None, Some("b")],
            vec![1, 1, 1, 2],
            vec![10, 10, 10, 11],
        );
        let second = batch(
            vec![Some("a"), None, Some("b"), None],
            vec![1, 1, 2, 1],
            vec![10, 10, 12, 10],
        );
        let mut distinct = DistinctOperator::new(Box::new(Input {
            schema: schema.clone(),
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
                vec![text("a"), text("1"), text("1970-01-11")],
                vec![None, text("1"), text("1970-01-11")],
                vec![text("b"), text("2"), text("1970-01-12")],
                vec![text("b"), text("2"), text("1970-01-13")],
            ]
        );
    }

    #[test]
    fn a_large_distinct_reserves_in_slabs_not_per_key() {
        // 400 000 distinct (text, integer) keys: the budget is charged in
        // 64 KiB slabs, so the pool sees hundreds of reservations, not one
        // per key, and everything is released at the end.
        let rows = 400_000;
        let schema = Arc::new(Schema::new(vec![
            Field::new("phrase", DataType::Utf8, true),
            Field::new("user", DataType::Int64, false),
        ]));
        let batches = (0..rows / 8192 + 1)
            .map(|chunk| {
                let start = chunk * 8192;
                let end = rows.min(start + 8192);
                RecordBatch::try_new(
                    schema.clone(),
                    vec![
                        Arc::new(StringArray::from_iter(
                            (start..end).map(|n| Some(format!("phrase {}", n % 50_000))),
                        )),
                        Arc::new(arrow::array::Int64Array::from_iter_values(
                            (start..end).map(|n| n as i64),
                        )),
                    ],
                )
                .unwrap()
            })
            .collect::<VecDeque<_>>();
        let pool = kaveon_core::QueryMemoryPool::new("distinct-slabs", 256 * 1024 * 1024).unwrap();
        let account = pool.operator("distinct").unwrap();
        let mut distinct =
            DistinctOperator::new(Box::new(Input { schema, batches })).with_memory(account.clone());
        let mut kept = 0;
        while let Some(batch) = distinct.next_batch().unwrap() {
            kept += batch.num_rows();
        }
        assert_eq!(kept, rows);
        let calls = account.snapshot().reservation_calls;
        assert!(calls < 2_000, "{calls} reservation calls for {rows} keys");
        assert_eq!(pool.snapshot().current_bytes, 0);
    }

    #[test]
    fn compact_keys_distinguish_nulls_and_zero_across_batches() {
        use arrow::array::Int64Array;
        let schema = Arc::new(Schema::new(vec![
            Field::new("user", DataType::Int64, true),
            Field::new("day", DataType::Int32, true),
        ]));
        let batch = |users: Vec<Option<i64>>, days: Vec<Option<i32>>| {
            RecordBatch::try_new(
                Arc::clone(&schema),
                vec![
                    Arc::new(Int64Array::from(users)),
                    Arc::new(Int32Array::from(days)),
                ],
            )
            .unwrap()
        };
        let first = batch(
            vec![Some(0), None, Some(0), Some(-1), None],
            vec![Some(0), Some(0), None, Some(0), None],
        );
        let second = batch(vec![Some(0), None, Some(7)], vec![Some(0), Some(0), None]);
        let mut distinct = DistinctOperator::new(Box::new(Input {
            schema: Arc::clone(&schema),
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
                vec![text("0"), text("0")],
                vec![None, text("0")],
                vec![text("0"), None],
                vec![text("-1"), text("0")],
                vec![None, None],
                vec![text("7"), None],
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
