use ahash::AHashMap;
use arrow::array::{Array, ArrayRef, AsArray, BooleanArray, UInt64Array};
use arrow::compute::{concat_batches, take};
use arrow::datatypes::{DataType, Field, Schema, SchemaRef};
use arrow::record_batch::RecordBatch;
use kaveon_core::{BatchOperator, KaveonError, MemoryReservation, OperatorMemoryAccount, Result};
use std::sync::Arc;

const HASH_ROW_OVERHEAD_BYTES: u64 = 64;
const OUTPUT_INDEX_BYTES_PER_ROW: u64 = 32;
const OUTPUT_INDEX_GROWTH_ROWS: usize = 1_024;
const OUTPUT_BATCH_ROWS: usize = 8_192;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JoinType {
    Inner,
    Left,
    Right,
    Full,
    Cross,
}

pub struct HashJoin {
    left: Box<dyn BatchOperator>,
    right: Box<dyn BatchOperator>,
    join_type: JoinType,
    keys: Vec<(usize, usize)>,
    schema: SchemaRef,
    memory: Option<OperatorMemoryAccount>,
    emitted: bool,
    state: Option<JoinState>,
    output_memory: Option<MemoryReservation>,
}

struct JoinState {
    right: RecordBatch,
    index: Option<JoinIndex>,
    matched_right: Vec<bool>,
    left: Option<RecordBatch>,
    left_row: usize,
    match_offset: usize,
    unmatched_right_row: usize,
    probe_finished: bool,
    _right_memory: Option<MemoryReservation>,
    _index_memory: Option<MemoryReservation>,
    _matched_memory: Option<MemoryReservation>,
    left_memory: Option<MemoryReservation>,
}

impl HashJoin {
    pub fn try_new(
        left: Box<dyn BatchOperator>,
        right: Box<dyn BatchOperator>,
        join_type: JoinType,
        keys: Vec<(String, String)>,
    ) -> Result<Self> {
        Self::try_new_qualified(left, right, join_type, keys, None, None)
    }

    pub fn try_new_qualified(
        left: Box<dyn BatchOperator>,
        right: Box<dyn BatchOperator>,
        join_type: JoinType,
        keys: Vec<(String, String)>,
        left_qualifier: Option<&str>,
        right_qualifier: Option<&str>,
    ) -> Result<Self> {
        if join_type != JoinType::Cross && keys.is_empty() {
            return Err(exec_err("hash joins require at least one equality key"));
        }
        let left_schema = Arc::clone(left.schema());
        let right_schema = Arc::clone(right.schema());
        let key_indices = keys
            .iter()
            .map(|(left_name, right_name)| {
                let left_index = resolve_column(&left_schema, left_name, "left")?;
                let right_index = resolve_column(&right_schema, right_name, "right")?;
                let left_type = left_schema.field(left_index).data_type();
                let right_type = right_schema.field(right_index).data_type();
                if left_type != right_type {
                    return Err(exec_err(format!(
                        "join key types differ: {left_name} is {left_type}, {right_name} is {right_type}"
                    )));
                }
                Ok((left_index, right_index))
            })
            .collect::<Result<Vec<_>>>()?;
        let fields = left_schema
            .fields()
            .iter()
            .map(|field| qualified_field(field, left_qualifier))
            .chain(
                right_schema
                    .fields()
                    .iter()
                    .map(|field| qualified_field(field, right_qualifier)),
            )
            .collect::<Vec<_>>();
        Ok(Self {
            left,
            right,
            join_type,
            keys: key_indices,
            schema: Arc::new(Schema::new(fields)),
            memory: None,
            emitted: false,
            state: None,
            output_memory: None,
        })
    }

    pub fn try_new_qualified_with_memory(
        left: Box<dyn BatchOperator>,
        right: Box<dyn BatchOperator>,
        join_type: JoinType,
        keys: Vec<(String, String)>,
        left_qualifier: Option<&str>,
        right_qualifier: Option<&str>,
        memory: OperatorMemoryAccount,
    ) -> Result<Self> {
        let mut operator = Self::try_new_qualified(
            left,
            right,
            join_type,
            keys,
            left_qualifier,
            right_qualifier,
        )?;
        operator.memory = Some(memory);
        Ok(operator)
    }
}

fn qualified_field(field: &Field, qualifier: Option<&str>) -> Field {
    let name = qualifier
        .map(|qualifier| format!("{qualifier}.{}", field.name()))
        .unwrap_or_else(|| field.name().clone());
    Field::new(name, field.data_type().clone(), true)
}

fn resolve_column(schema: &SchemaRef, name: &str, side: &str) -> Result<usize> {
    if let Ok(index) = schema.index_of(name) {
        return Ok(index);
    }
    let unqualified = name.rsplit('.').next().unwrap_or(name);
    let matches = schema
        .fields()
        .iter()
        .enumerate()
        .filter(|(_, field)| {
            field.name() == unqualified
                || field
                    .name()
                    .strip_suffix(unqualified)
                    .is_some_and(|prefix| prefix.ends_with('.'))
        })
        .map(|(index, _)| index)
        .collect::<Vec<_>>();
    match matches.as_slice() {
        [index] => Ok(*index),
        [] => Err(exec_err(format!("{side} join column '{name}' not found"))),
        _ => Err(exec_err(format!(
            "{side} join column '{name}' is ambiguous"
        ))),
    }
}

impl BatchOperator for HashJoin {
    fn schema(&self) -> &SchemaRef {
        &self.schema
    }

    fn next_batch(&mut self) -> Result<Option<RecordBatch>> {
        self.output_memory = None;
        if self.emitted {
            return Ok(None);
        }
        let result = (|| {
            if self.state.is_none() {
                let (right, right_memory) = collect_input(&mut self.right, self.memory.as_ref())?;
                let index_memory = reserve_join_index(self.memory.as_ref(), &right, &self.keys)?;
                let index = if self.join_type == JoinType::Cross {
                    None
                } else {
                    Some(JoinIndex::build(&right, &self.keys, self.memory.as_ref())?)
                };
                let needs_matches = matches!(self.join_type, JoinType::Right | JoinType::Full);
                let matched_memory = reserve_bytes(
                    self.memory.as_ref(),
                    if needs_matches {
                        right.num_rows() as u64
                    } else {
                        0
                    },
                )?;
                let matched_right = if needs_matches {
                    vec![false; right.num_rows()]
                } else {
                    Vec::new()
                };
                self.state = Some(JoinState {
                    right,
                    index,
                    matched_right,
                    left: None,
                    left_row: 0,
                    match_offset: 0,
                    unmatched_right_row: 0,
                    probe_finished: false,
                    _right_memory: right_memory,
                    _index_memory: index_memory,
                    _matched_memory: matched_memory,
                    left_memory: None,
                });
            }
            let state = self.state.as_mut().expect("join initialized");
            loop {
                check_cancelled(self.memory.as_ref())?;
                if state
                    .left
                    .as_ref()
                    .is_some_and(|batch| state.left_row >= batch.num_rows())
                {
                    state.left = None;
                    state.left_memory = None;
                }
                if state.left.is_none() && !state.probe_finished {
                    match self.left.next_batch()? {
                        Some(batch) => {
                            if batch.schema() != *self.left.schema() {
                                return Err(exec_err("join probe schema changed"));
                            }
                            state.left_memory = reserve_bytes(
                                self.memory.as_ref(),
                                batch.get_array_memory_size() as u64,
                            )?;
                            state.left = Some(batch);
                            state.left_row = 0;
                            state.match_offset = 0;
                        }
                        None => state.probe_finished = true,
                    }
                }
                let empty_left = RecordBatch::new_empty(self.left.schema().clone());
                let left = state.left.as_ref().unwrap_or(&empty_left);
                let mut left_rows = Vec::new();
                let mut right_rows = Vec::new();
                let mut reservations = Vec::new();
                while state.left_row < left.num_rows() && left_rows.len() < OUTPUT_BATCH_ROWS {
                    if state.left_row.is_multiple_of(1024) {
                        check_cancelled(self.memory.as_ref())?;
                    }
                    let matches = state
                        .index
                        .as_ref()
                        .map(|index| index.matches(left, state.left_row, &self.keys))
                        .transpose()?
                        .flatten();
                    let match_count = if self.join_type == JoinType::Cross {
                        state.right.num_rows()
                    } else {
                        matches.map_or(0, <[usize]>::len)
                    };
                    if match_count == 0 {
                        if matches!(self.join_type, JoinType::Left | JoinType::Full) {
                            push_output_pair(
                                &mut left_rows,
                                &mut right_rows,
                                Some(state.left_row as u64),
                                None,
                                self.memory.as_ref(),
                                &mut reservations,
                            )?;
                        }
                        state.left_row += 1;
                        state.match_offset = 0;
                        continue;
                    }
                    while state.match_offset < match_count && left_rows.len() < OUTPUT_BATCH_ROWS {
                        let right_row = if self.join_type == JoinType::Cross {
                            state.match_offset
                        } else {
                            matches.expect("matching rows")[state.match_offset]
                        };
                        push_output_pair(
                            &mut left_rows,
                            &mut right_rows,
                            Some(state.left_row as u64),
                            Some(right_row as u64),
                            self.memory.as_ref(),
                            &mut reservations,
                        )?;
                        if !state.matched_right.is_empty() {
                            state.matched_right[right_row] = true;
                        }
                        state.match_offset += 1;
                    }
                    if state.match_offset == match_count {
                        state.left_row += 1;
                        state.match_offset = 0;
                    }
                }
                if state.probe_finished
                    && matches!(self.join_type, JoinType::Right | JoinType::Full)
                {
                    while state.unmatched_right_row < state.right.num_rows()
                        && left_rows.len() < OUTPUT_BATCH_ROWS
                    {
                        let row = state.unmatched_right_row;
                        if row.is_multiple_of(1024) {
                            check_cancelled(self.memory.as_ref())?;
                        }
                        state.unmatched_right_row += 1;
                        if !state.matched_right[row] {
                            push_output_pair(
                                &mut left_rows,
                                &mut right_rows,
                                None,
                                Some(row as u64),
                                self.memory.as_ref(),
                                &mut reservations,
                            )?;
                        }
                    }
                }
                if left_rows.is_empty() {
                    if state.probe_finished {
                        return Ok(None);
                    }
                    continue;
                }
                let bytes = estimated_output_bytes(left, &left_rows)?
                    .checked_add(estimated_output_bytes(&state.right, &right_rows)?)
                    .ok_or_else(|| exec_err("join output memory estimate overflow"))?;
                self.output_memory = reserve_bytes(self.memory.as_ref(), bytes)?;
                let left_indices = UInt64Array::from(left_rows);
                let right_indices = UInt64Array::from(right_rows);
                let columns = left
                    .columns()
                    .iter()
                    .map(|column| take(column, &left_indices, None))
                    .chain(
                        state
                            .right
                            .columns()
                            .iter()
                            .map(|column| take(column, &right_indices, None)),
                    )
                    .collect::<std::result::Result<Vec<ArrayRef>, _>>()?;
                return Ok(Some(RecordBatch::try_new(self.schema.clone(), columns)?));
            }
        })();
        if !matches!(result, Ok(Some(_))) {
            self.emitted = true;
            self.state = None;
            self.output_memory = None;
        }
        result
    }
}

fn collect_input(
    source: &mut Box<dyn BatchOperator>,
    memory: Option<&OperatorMemoryAccount>,
) -> Result<(RecordBatch, Option<MemoryReservation>)> {
    let schema = Arc::clone(source.schema());
    let mut batches = Vec::new();
    let mut batch_reservations = Vec::new();
    let mut total_bytes = 0_u64;
    while let Some(batch) = source.next_batch()? {
        check_cancelled(memory)?;
        let bytes = u64::try_from(batch.get_array_memory_size())
            .map_err(|_| exec_err("join input batch memory size exceeds u64"))?;
        total_bytes = total_bytes
            .checked_add(bytes)
            .ok_or_else(|| exec_err("join input memory estimate overflow"))?;
        if let Some(reservation) = reserve_bytes(memory, bytes)? {
            batch_reservations.push(reservation);
        }
        batches.push(batch);
    }
    if batches.is_empty() {
        Ok((RecordBatch::new_empty(schema), reserve_bytes(memory, 0)?))
    } else if batches.len() == 1 {
        Ok((
            batches.pop().expect("one build batch"),
            batch_reservations.pop(),
        ))
    } else {
        let concatenated_reservation = reserve_bytes(memory, total_bytes)?;
        let concatenated = concat_batches(&schema, &batches)?;
        drop(batch_reservations);
        Ok((concatenated, concatenated_reservation))
    }
}

pub(crate) fn estimated_output_bytes(batch: &RecordBatch, indices: &[Option<u64>]) -> Result<u64> {
    batch.columns().iter().try_fold(0_u64, |total, column| {
        // Fixed-width columns need an arithmetic bound, not a pass over every
        // output row. Keep the same conservative validity/alignment allowance.
        let width: Option<u64> = match column.data_type() {
            DataType::Boolean | DataType::Int8 | DataType::UInt8 => Some(2),
            DataType::Int16 | DataType::UInt16 | DataType::Float16 => Some(3),
            DataType::Int32
            | DataType::UInt32
            | DataType::Float32
            | DataType::Date32
            | DataType::Time32(_) => Some(5),
            DataType::Int64
            | DataType::UInt64
            | DataType::Float64
            | DataType::Date64
            | DataType::Time64(_)
            | DataType::Timestamp(_, _)
            | DataType::Duration(_) => Some(9),
            DataType::Decimal128(_, _) => Some(17),
            DataType::Decimal256(_, _) => Some(33),
            DataType::Null => Some(0),
            _ => None,
        };
        if let Some(width) = width {
            return width
                .checked_mul(indices.len() as u64)
                .and_then(|bytes| bytes.checked_add(256))
                .and_then(|bytes| total.checked_add(bytes))
                .ok_or_else(|| exec_err("join output memory estimate overflow"));
        }
        let mut bytes = 256_u64;
        for index in indices {
            let row_bytes = match column.data_type() {
                DataType::Boolean | DataType::Int8 | DataType::UInt8 => 2,
                DataType::Int16 | DataType::UInt16 | DataType::Float16 => 3,
                DataType::Int32
                | DataType::UInt32
                | DataType::Float32
                | DataType::Date32
                | DataType::Time32(_) => 5,
                DataType::Int64
                | DataType::UInt64
                | DataType::Float64
                | DataType::Date64
                | DataType::Time64(_)
                | DataType::Timestamp(_, _)
                | DataType::Duration(_) => 9,
                DataType::Decimal128(_, _) => 17,
                DataType::Decimal256(_, _) => 33,
                DataType::Null => 0,
                DataType::Utf8 => {
                    8 + index.map_or(0, |index| {
                        column.as_string::<i32>().value(index as usize).len() as u64
                    })
                }
                DataType::LargeUtf8 => {
                    16 + index.map_or(0, |index| {
                        column.as_string::<i64>().value(index as usize).len() as u64
                    })
                }
                // Taking one row gives Arrow's own estimate for nested/variable
                // types without allocating a duplicate output array.
                _ => index.map_or(8, |index| {
                    column.slice(index as usize, 1).get_buffer_memory_size() as u64 + 1
                }),
            };
            bytes = bytes
                .checked_add(row_bytes)
                .ok_or_else(|| exec_err("join output memory estimate overflow"))?;
        }
        total
            .checked_add(bytes)
            .ok_or_else(|| exec_err("join output memory estimate overflow"))
    })
}

fn reserve_join_index(
    memory: Option<&OperatorMemoryAccount>,
    right: &RecordBatch,
    keys: &[(usize, usize)],
) -> Result<Option<MemoryReservation>> {
    let key_bytes = keys.iter().try_fold(0_u64, |total, (_, right_index)| {
        let bytes = u64::try_from(right.column(*right_index).get_array_memory_size())
            .map_err(|_| exec_err("join key memory size exceeds u64"))?;
        total
            .checked_add(bytes)
            .ok_or_else(|| exec_err("join key memory estimate overflow"))
    })?;
    let row_overhead = u64::try_from(right.num_rows())
        .map_err(|_| exec_err("join build row count exceeds u64"))?
        .checked_mul(HASH_ROW_OVERHEAD_BYTES)
        .ok_or_else(|| exec_err("join index memory estimate overflow"))?;
    reserve_bytes(memory, key_bytes.saturating_add(row_overhead))
}

fn reserve_bytes(
    memory: Option<&OperatorMemoryAccount>,
    bytes: u64,
) -> Result<Option<MemoryReservation>> {
    memory.map(|account| account.reserve(bytes)).transpose()
}

fn push_output_pair(
    left_rows: &mut Vec<Option<u64>>,
    right_rows: &mut Vec<Option<u64>>,
    left: Option<u64>,
    right: Option<u64>,
    memory: Option<&OperatorMemoryAccount>,
    reservations: &mut Vec<MemoryReservation>,
) -> Result<()> {
    if left_rows.len().is_multiple_of(1024) {
        check_cancelled(memory)?;
    }
    if left_rows.len() == left_rows.capacity() {
        let growth = left_rows.capacity().max(OUTPUT_INDEX_GROWTH_ROWS);
        let bytes = u64::try_from(growth)
            .map_err(|_| exec_err("join output capacity exceeds u64"))?
            .checked_mul(OUTPUT_INDEX_BYTES_PER_ROW)
            .ok_or_else(|| exec_err("join output memory estimate overflow"))?;
        if let Some(reservation) = reserve_bytes(memory, bytes)? {
            reservations.push(reservation);
        }
        left_rows.reserve_exact(growth);
        right_rows.reserve_exact(growth);
    }
    left_rows.push(left);
    right_rows.push(right);
    Ok(())
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
enum Key {
    Bool(bool),
    Int32(i32),
    Int64(i64),
    UInt64(u64),
    Float64(u64),
    Utf8(String),
}

/// Common integer joins avoid allocating a Vec<Key> for every probe row.
/// Other key layouts use the existing exact typed composite representation.
enum JoinIndex {
    Int64 {
        first_rows: AHashMap<i64, usize>,
        duplicate_rows: AHashMap<i64, Vec<usize>>,
    },
    Composite(AHashMap<Vec<Key>, Vec<usize>>),
}

impl JoinIndex {
    fn build(
        right: &RecordBatch,
        keys: &[(usize, usize)],
        memory: Option<&OperatorMemoryAccount>,
    ) -> Result<Self> {
        if let [(.., index)] = keys
            && right.column(*index).data_type() == &DataType::Int64
        {
            let values = right
                .column(*index)
                .as_primitive::<arrow::datatypes::Int64Type>();
            let mut first_rows = AHashMap::new();
            let mut duplicate_rows: AHashMap<i64, Vec<usize>> = AHashMap::new();
            for row in 0..values.len() {
                if row.is_multiple_of(1024) {
                    check_cancelled(memory)?;
                }
                if values.is_valid(row) {
                    let key = values.value(row);
                    if let Some(first_row) = first_rows.get(&key).copied() {
                        duplicate_rows
                            .entry(key)
                            .or_insert_with(|| vec![first_row])
                            .push(row);
                    } else {
                        first_rows.insert(key, row);
                    }
                }
            }
            return Ok(Self::Int64 {
                first_rows,
                duplicate_rows,
            });
        }
        let mut map: AHashMap<Vec<Key>, Vec<usize>> = AHashMap::new();
        for row in 0..right.num_rows() {
            if row.is_multiple_of(1024) {
                check_cancelled(memory)?;
            }
            if let Some(key) = row_key(right, row, keys, false)? {
                map.entry(key).or_default().push(row);
            }
        }
        Ok(Self::Composite(map))
    }

    fn matches(
        &self,
        left: &RecordBatch,
        row: usize,
        keys: &[(usize, usize)],
    ) -> Result<Option<&[usize]>> {
        Ok(match self {
            Self::Int64 {
                first_rows,
                duplicate_rows,
            } => {
                let values = left
                    .column(keys[0].0)
                    .as_primitive::<arrow::datatypes::Int64Type>();
                if values.is_null(row) {
                    None
                } else {
                    let key = values.value(row);
                    if duplicate_rows.is_empty() {
                        first_rows.get(&key).map(std::slice::from_ref)
                    } else {
                        duplicate_rows
                            .get(&key)
                            .map(Vec::as_slice)
                            .or_else(|| first_rows.get(&key).map(std::slice::from_ref))
                    }
                }
            }
            Self::Composite(map) => row_key(left, row, keys, true)?
                .and_then(|key| map.get(&key))
                .map(Vec::as_slice),
        })
    }
}

fn check_cancelled(memory: Option<&OperatorMemoryAccount>) -> Result<()> {
    if let Some(memory) = memory {
        memory.check_cancelled()?;
    }
    Ok(())
}

fn row_key(
    batch: &RecordBatch,
    row: usize,
    keys: &[(usize, usize)],
    left: bool,
) -> Result<Option<Vec<Key>>> {
    keys.iter()
        .map(|(left_index, right_index)| {
            let column = batch.column(if left { *left_index } else { *right_index });
            if column.is_null(row) {
                return Ok(None);
            }
            Ok(Some(match column.data_type() {
                DataType::Boolean => Key::Bool(
                    column
                        .as_any()
                        .downcast_ref::<BooleanArray>()
                        .expect("type checked")
                        .value(row),
                ),
                DataType::Int32 => Key::Int32(
                    column
                        .as_primitive::<arrow::datatypes::Int32Type>()
                        .value(row),
                ),
                DataType::Int64 => Key::Int64(
                    column
                        .as_primitive::<arrow::datatypes::Int64Type>()
                        .value(row),
                ),
                DataType::UInt64 => Key::UInt64(
                    column
                        .as_primitive::<arrow::datatypes::UInt64Type>()
                        .value(row),
                ),
                DataType::Float64 => {
                    let value = column
                        .as_primitive::<arrow::datatypes::Float64Type>()
                        .value(row);
                    Key::Float64(if value == 0.0 {
                        0
                    } else if value.is_nan() {
                        f64::NAN.to_bits()
                    } else {
                        value.to_bits()
                    })
                }
                DataType::Utf8 => Key::Utf8(column.as_string::<i32>().value(row).to_owned()),
                data_type => {
                    return Err(exec_err(format!("unsupported join key type {data_type}")));
                }
            }))
        })
        .collect::<Result<Option<Vec<_>>>>()
}

fn exec_err(message: impl Into<String>) -> KaveonError {
    KaveonError::Execution(message.into())
}

#[cfg(test)]
mod tests {
    use super::*;
    use arrow::array::{Int64Array, StringArray};
    use std::collections::VecDeque;

    struct Input {
        schema: SchemaRef,
        batches: VecDeque<RecordBatch>,
    }
    impl Input {
        fn new(batch: RecordBatch) -> Self {
            Self {
                schema: batch.schema(),
                batches: VecDeque::from([batch]),
            }
        }
    }
    impl BatchOperator for Input {
        fn schema(&self) -> &SchemaRef {
            &self.schema
        }
        fn next_batch(&mut self) -> Result<Option<RecordBatch>> {
            Ok(self.batches.pop_front())
        }
    }
    fn input(ids: Vec<Option<i64>>, names: Vec<&str>, name: &str) -> Input {
        let schema = Arc::new(Schema::new(vec![
            Field::new("id", DataType::Int64, true),
            Field::new(name, DataType::Utf8, false),
        ]));
        Input::new(
            RecordBatch::try_new(
                schema,
                vec![
                    Arc::new(Int64Array::from(ids)),
                    Arc::new(StringArray::from(names)),
                ],
            )
            .unwrap(),
        )
    }

    #[test]
    fn integer_fast_path_matches_composite_path_with_nulls_duplicates_and_outer_rows() {
        let left_ids = (0..79)
            .map(|i| {
                if i % 11 == 0 {
                    None
                } else {
                    Some((i * 7 % 23) - 10)
                }
            })
            .collect::<Vec<_>>();
        let right_ids = (0..61)
            .map(|i| {
                if i % 9 == 0 {
                    None
                } else {
                    Some((i * 5 % 19) - 8)
                }
            })
            .collect::<Vec<_>>();
        for mode in [
            JoinType::Inner,
            JoinType::Left,
            JoinType::Right,
            JoinType::Full,
        ] {
            let run = |keys| {
                let left = Box::new(input(
                    left_ids.clone(),
                    vec!["l"; left_ids.len()],
                    "left_name",
                ));
                let right = Box::new(input(
                    right_ids.clone(),
                    vec!["r"; right_ids.len()],
                    "right_name",
                ));
                let mut join = HashJoin::try_new(left, right, mode, keys).unwrap();
                let batches = kaveon_core::collect_batches(&mut join).unwrap();
                concat_batches(join.schema(), &batches).unwrap()
            };
            assert_eq!(
                run(vec![("id".into(), "id".into())]),
                run(vec![("id".into(), "id".into()), ("id".into(), "id".into())])
            );
        }
    }

    #[test]
    fn inner_join_preserves_duplicates_and_excludes_null_keys() {
        let mut join = HashJoin::try_new(
            Box::new(input(
                vec![Some(1), Some(2), None],
                vec!["a", "b", "n"],
                "left_name",
            )),
            Box::new(input(
                vec![Some(1), Some(1), None],
                vec!["x", "y", "n"],
                "right_name",
            )),
            JoinType::Inner,
            vec![("id".into(), "id".into())],
        )
        .unwrap();
        assert_eq!(join.next_batch().unwrap().unwrap().num_rows(), 2);
        assert!(join.next_batch().unwrap().is_none());
    }

    #[test]
    fn full_join_emits_both_unmatched_sides() {
        let mut join = HashJoin::try_new(
            Box::new(input(vec![Some(1), Some(2)], vec!["a", "b"], "left_name")),
            Box::new(input(vec![Some(1), Some(3)], vec!["x", "z"], "right_name")),
            JoinType::Full,
            vec![("id".into(), "id".into())],
        )
        .unwrap();
        let batches = kaveon_core::collect_batches(&mut join).unwrap();
        let result = concat_batches(join.schema(), &batches).unwrap();
        assert_eq!(result.num_rows(), 3);
        assert_eq!(result.column(0).null_count(), 1);
        assert_eq!(result.column(2).null_count(), 1);
    }

    #[test]
    fn cross_join_returns_cartesian_product() {
        let mut join = HashJoin::try_new(
            Box::new(input(vec![Some(1), Some(2)], vec!["a", "b"], "left_name")),
            Box::new(input(
                vec![Some(3), Some(4), Some(5)],
                vec!["x", "y", "z"],
                "right_name",
            )),
            JoinType::Cross,
            Vec::new(),
        )
        .unwrap();
        assert_eq!(join.next_batch().unwrap().unwrap().num_rows(), 6);
    }

    #[test]
    fn streaming_join_preserves_fanout_and_outer_tail_across_output_boundaries() {
        for mode in [JoinType::Full, JoinType::Cross] {
            let mut right_ids = vec![Some(1); 9_000];
            right_ids.extend([None, Some(3)]);
            let mut join = HashJoin::try_new(
                Box::new(input(
                    vec![Some(1), Some(1), None, Some(2)],
                    vec!["l"; 4],
                    "l",
                )),
                Box::new(input(right_ids, vec!["r"; 9_002], "r")),
                mode,
                if mode == JoinType::Cross {
                    vec![]
                } else {
                    vec![("id".into(), "id".into())]
                },
            )
            .unwrap();
            let batches = kaveon_core::collect_batches(&mut join).unwrap();
            assert!(batches.len() >= 3);
            assert!(
                batches
                    .iter()
                    .all(|batch| batch.num_rows() <= OUTPUT_BATCH_ROWS)
            );
            let rows = batches.iter().map(RecordBatch::num_rows).sum::<usize>();
            assert_eq!(
                rows,
                if mode == JoinType::Full {
                    18_004
                } else {
                    36_008
                }
            );
            if mode == JoinType::Full {
                assert_eq!(
                    batches
                        .iter()
                        .map(|batch| batch.column(1).null_count())
                        .sum::<usize>(),
                    2
                );
                assert_eq!(
                    batches
                        .iter()
                        .map(|batch| batch.column(3).null_count())
                        .sum::<usize>(),
                    2
                );
            }
        }
    }

    #[test]
    fn streaming_join_does_not_collect_probe_and_releases_memory_on_eof() {
        struct Generated {
            batch: RecordBatch,
            remaining: usize,
            reads: std::rc::Rc<std::cell::Cell<usize>>,
        }
        impl BatchOperator for Generated {
            fn schema(&self) -> &SchemaRef {
                self.batch.schema_ref()
            }
            fn next_batch(&mut self) -> Result<Option<RecordBatch>> {
                if self.remaining == 0 {
                    return Ok(None);
                }
                self.remaining -= 1;
                self.reads.set(self.reads.get() + 1);
                Ok(Some(self.batch.clone()))
            }
        }
        let batch = input(vec![Some(1); 512], vec!["l"; 512], "l")
            .batches
            .pop_front()
            .unwrap();
        let reads = std::rc::Rc::new(std::cell::Cell::new(0));
        let pool = kaveon_core::QueryMemoryPool::new("streaming-join", 256 * 1024).unwrap();
        let mut join = HashJoin::try_new_qualified_with_memory(
            Box::new(Generated {
                batch,
                remaining: 300,
                reads: reads.clone(),
            }),
            Box::new(input(vec![Some(1)], vec!["r"], "r")),
            JoinType::Inner,
            vec![("id".into(), "id".into())],
            None,
            None,
            pool.operator("join").unwrap(),
        )
        .unwrap();
        let first = join.next_batch().unwrap().unwrap();
        assert_eq!(reads.get(), 1);
        let mut count = first.num_rows();
        drop(first);
        while let Some(batch) = join.next_batch().unwrap() {
            count += batch.num_rows();
        }
        assert_eq!(count, 300 * 512);
        assert_eq!(pool.snapshot().current_bytes, 0);
        assert!(pool.snapshot().peak_bytes <= 256 * 1024);
    }

    #[test]
    fn streaming_join_cancellation_releases_retained_build_and_output() {
        let pool =
            kaveon_core::QueryMemoryPool::new("cancel-streaming-join", 4 * 1024 * 1024).unwrap();
        let canceled = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let signal = canceled.clone();
        pool.set_cancellation_probe(move || signal.load(std::sync::atomic::Ordering::Acquire))
            .unwrap();
        let mut join = HashJoin::try_new_qualified_with_memory(
            Box::new(input(vec![Some(1); 2], vec!["l"; 2], "l")),
            Box::new(input(vec![Some(1); 9_000], vec!["r"; 9_000], "r")),
            JoinType::Inner,
            vec![("id".into(), "id".into())],
            None,
            None,
            pool.operator("join").unwrap(),
        )
        .unwrap();
        assert_eq!(
            join.next_batch().unwrap().unwrap().num_rows(),
            OUTPUT_BATCH_ROWS
        );
        assert!(pool.snapshot().current_bytes > 0);
        canceled.store(true, std::sync::atomic::Ordering::Release);
        assert!(join.next_batch().is_err());
        assert_eq!(pool.snapshot().current_bytes, 0);
        assert!(join.next_batch().unwrap().is_none());
    }

    #[test]
    fn memory_aware_join_fails_before_unbounded_output_growth() {
        let pool = kaveon_core::QueryMemoryPool::new("bounded-join", 512).unwrap();
        let account = pool.operator("hash-join").unwrap();
        let mut join = HashJoin::try_new_qualified_with_memory(
            Box::new(input(vec![Some(1), Some(1)], vec!["a", "b"], "left_name")),
            Box::new(input(vec![Some(1), Some(1)], vec!["x", "y"], "right_name")),
            JoinType::Inner,
            vec![("id".into(), "id".into())],
            None,
            None,
            account,
        )
        .unwrap();

        let error = join.next_batch().unwrap_err().to_string();
        assert!(error.contains("query 'bounded-join' operator 'hash-join'"));
        drop(join);
        assert_eq!(pool.snapshot().current_bytes, 0);
        assert!(pool.snapshot().peak_bytes <= pool.snapshot().limit_bytes);
    }
}
