use arrow::array::{Array, ArrayRef, AsArray, BooleanArray, UInt64Array};
use arrow::compute::{concat_batches, take};
use arrow::datatypes::{DataType, Field, Schema, SchemaRef};
use arrow::record_batch::RecordBatch;
use kaveon_core::{BatchOperator, KaveonError, MemoryReservation, OperatorMemoryAccount, Result};
use std::collections::HashMap;
use std::sync::Arc;

const HASH_ROW_OVERHEAD_BYTES: u64 = 64;
const OUTPUT_INDEX_BYTES_PER_ROW: u64 = 32;
const OUTPUT_INDEX_GROWTH_ROWS: usize = 1_024;

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
        if self.emitted {
            return Ok(None);
        }
        self.emitted = true;
        let (left, _left_memory) = collect_input(&mut self.left, self.memory.as_ref())?;
        let (right, _right_memory) = collect_input(&mut self.right, self.memory.as_ref())?;
        let _index_memory = reserve_join_index(self.memory.as_ref(), &right, &self.keys)?;
        let _matched_memory = reserve_bytes(
            self.memory.as_ref(),
            u64::try_from(right.num_rows())
                .map_err(|_| exec_err("join build row count exceeds u64"))?,
        )?;
        let mut left_rows = Vec::new();
        let mut right_rows = Vec::new();
        let mut output_reservations = Vec::new();
        let mut matched_right = vec![false; right.num_rows()];

        if self.join_type == JoinType::Cross {
            for left_row in 0..left.num_rows() {
                for right_row in 0..right.num_rows() {
                    push_output_pair(
                        &mut left_rows,
                        &mut right_rows,
                        Some(left_row as u64),
                        Some(right_row as u64),
                        self.memory.as_ref(),
                        &mut output_reservations,
                    )?;
                }
            }
        } else {
            let mut right_index: HashMap<Vec<Key>, Vec<usize>> = HashMap::new();
            for row in 0..right.num_rows() {
                if let Some(key) = row_key(&right, row, &self.keys, false)? {
                    right_index.entry(key).or_default().push(row);
                }
            }
            for left_row in 0..left.num_rows() {
                let matches = row_key(&left, left_row, &self.keys, true)?
                    .and_then(|key| right_index.get(&key));
                if let Some(right_matches) = matches {
                    for &right_row in right_matches {
                        push_output_pair(
                            &mut left_rows,
                            &mut right_rows,
                            Some(left_row as u64),
                            Some(right_row as u64),
                            self.memory.as_ref(),
                            &mut output_reservations,
                        )?;
                        matched_right[right_row] = true;
                    }
                } else if matches!(self.join_type, JoinType::Left | JoinType::Full) {
                    push_output_pair(
                        &mut left_rows,
                        &mut right_rows,
                        Some(left_row as u64),
                        None,
                        self.memory.as_ref(),
                        &mut output_reservations,
                    )?;
                }
            }
            if matches!(self.join_type, JoinType::Right | JoinType::Full) {
                for (right_row, matched) in matched_right.into_iter().enumerate() {
                    if !matched {
                        push_output_pair(
                            &mut left_rows,
                            &mut right_rows,
                            None,
                            Some(right_row as u64),
                            self.memory.as_ref(),
                            &mut output_reservations,
                        )?;
                    }
                }
            }
        }

        let left_indices = UInt64Array::from(left_rows);
        let right_indices = UInt64Array::from(right_rows);
        let columns = left
            .columns()
            .iter()
            .map(|column| take(column, &left_indices, None))
            .chain(
                right
                    .columns()
                    .iter()
                    .map(|column| take(column, &right_indices, None)),
            )
            .collect::<std::result::Result<Vec<ArrayRef>, _>>()?;
        Ok(Some(RecordBatch::try_new(
            Arc::clone(&self.schema),
            columns,
        )?))
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
    } else {
        let concatenated_reservation = reserve_bytes(memory, total_bytes)?;
        let concatenated = concat_batches(&schema, &batches)?;
        drop(batch_reservations);
        Ok((concatenated, concatenated_reservation))
    }
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
                DataType::Float64 => Key::Float64(
                    column
                        .as_primitive::<arrow::datatypes::Float64Type>()
                        .value(row)
                        .to_bits(),
                ),
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
        let result = join.next_batch().unwrap().unwrap();
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
