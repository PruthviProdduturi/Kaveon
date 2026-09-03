use arrow::array::{Array, ArrayRef, AsArray, BooleanArray, UInt64Array};
use arrow::compute::{concat_batches, take};
use arrow::datatypes::{DataType, Field, Schema, SchemaRef};
use arrow::record_batch::RecordBatch;
use kaveon_core::{BatchOperator, KaveonError, Result};
use std::collections::HashMap;
use std::sync::Arc;

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
            emitted: false,
        })
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
        let left = collect_input(&mut self.left)?;
        let right = collect_input(&mut self.right)?;
        let mut left_rows = Vec::new();
        let mut right_rows = Vec::new();
        let mut matched_right = vec![false; right.num_rows()];

        if self.join_type == JoinType::Cross {
            for left_row in 0..left.num_rows() {
                for right_row in 0..right.num_rows() {
                    left_rows.push(Some(left_row as u64));
                    right_rows.push(Some(right_row as u64));
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
                        left_rows.push(Some(left_row as u64));
                        right_rows.push(Some(right_row as u64));
                        matched_right[right_row] = true;
                    }
                } else if matches!(self.join_type, JoinType::Left | JoinType::Full) {
                    left_rows.push(Some(left_row as u64));
                    right_rows.push(None);
                }
            }
            if matches!(self.join_type, JoinType::Right | JoinType::Full) {
                for (right_row, matched) in matched_right.into_iter().enumerate() {
                    if !matched {
                        left_rows.push(None);
                        right_rows.push(Some(right_row as u64));
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

fn collect_input(source: &mut Box<dyn BatchOperator>) -> Result<RecordBatch> {
    let schema = Arc::clone(source.schema());
    let mut batches = Vec::new();
    while let Some(batch) = source.next_batch()? {
        batches.push(batch);
    }
    if batches.is_empty() {
        Ok(RecordBatch::new_empty(schema))
    } else {
        Ok(concat_batches(&schema, &batches)?)
    }
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
}
