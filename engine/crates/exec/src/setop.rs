use std::collections::{HashMap, HashSet};

use arrow::array::{Array, AsArray, RecordBatch};
use arrow::datatypes::{DataType, Int32Type, SchemaRef};
use kaveon_core::{BatchOperator, KaveonError, OperatorMemoryAccount, ReservationSlab, Result};

use crate::aggregate::AggregateValue;

pub enum SetOpMode {
    Intersect,
    Except,
    IntersectAll,
    ExceptAll,
}

pub struct SetOpOperator {
    left: Box<dyn BatchOperator>,
    right: Box<dyn BatchOperator>,
    mode: SetOpMode,
    right_set: Option<HashSet<Vec<AggregateValue>>>,
    right_counts: Option<HashMap<Vec<AggregateValue>, usize>>,
    emitted: HashSet<Vec<AggregateValue>>,
    memory: Option<OperatorMemoryAccount>,
    reservations: ReservationSlab,
}

impl SetOpOperator {
    pub fn new(
        left: Box<dyn BatchOperator>,
        right: Box<dyn BatchOperator>,
        mode: SetOpMode,
    ) -> Self {
        Self {
            left,
            right,
            mode,
            right_set: None,
            right_counts: None,
            emitted: HashSet::new(),
            memory: None,
            reservations: ReservationSlab::default(),
        }
    }

    pub fn with_memory(mut self, memory: OperatorMemoryAccount) -> Self {
        self.memory = Some(memory);
        self
    }

    fn reserve_key(&mut self, key: &[AggregateValue]) -> Result<()> {
        if let Some(memory) = &self.memory {
            let bytes = key.iter().fold(128_u64, |bytes, value| {
                bytes.saturating_add(64).saturating_add(match value {
                    AggregateValue::Utf8(value) => value.len() as u64,
                    _ => 0,
                })
            });
            self.reservations.reserve(memory, bytes)?;
        }
        Ok(())
    }

    fn build_right_set(&mut self) -> Result<()> {
        // Rows compare by value, so a dictionary-encoded side is its value type.
        let logical = |schema: &SchemaRef| {
            schema
                .fields()
                .iter()
                .map(|field| match field.data_type() {
                    DataType::Dictionary(_, values) => values.as_ref().clone(),
                    other => other.clone(),
                })
                .collect::<Vec<_>>()
        };
        if logical(self.left.schema()) != logical(self.right.schema()) {
            return Err(KaveonError::Execution(
                "set operation input column types must match".into(),
            ));
        }
        let mut set = HashSet::new();
        let mut counts = HashMap::new();
        while let Some(batch) = self.right.next_batch()? {
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
            for row in 0..batch.num_rows() {
                let key: Vec<AggregateValue> = (0..num_cols)
                    .map(|col| extract_value(batch.column(col), row))
                    .collect::<Result<_>>()?;
                *counts.entry(key.clone()).or_insert(0) += 1;
                if !set.contains(&key) {
                    self.reserve_key(&key)?;
                    set.insert(key);
                }
            }
        }
        self.right_set = Some(set);
        self.right_counts = Some(counts);
        Ok(())
    }
}

impl BatchOperator for SetOpOperator {
    fn schema(&self) -> &SchemaRef {
        self.left.schema()
    }

    fn next_batch(&mut self) -> Result<Option<RecordBatch>> {
        if self.right_set.is_none() {
            self.build_right_set()?;
        }
        loop {
            let Some(batch) = self.left.next_batch()? else {
                self.right_set = Some(HashSet::new());
                self.right_counts = Some(HashMap::new());
                self.emitted = HashSet::new();
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
                match self.mode {
                    SetOpMode::Intersect | SetOpMode::Except => {
                        let in_right = self.right_set.as_ref().unwrap().contains(&key);
                        let emit = match self.mode {
                            SetOpMode::Intersect => in_right,
                            SetOpMode::Except => !in_right,
                            SetOpMode::IntersectAll | SetOpMode::ExceptAll => unreachable!(),
                        };
                        if emit && !self.emitted.contains(&key) {
                            self.reserve_key(&key)?;
                            self.emitted.insert(key);
                            keep.push(row as u32);
                        }
                    }
                    SetOpMode::IntersectAll => {
                        if let Some(remaining) = self.right_counts.as_mut().unwrap().get_mut(&key)
                            && *remaining > 0
                        {
                            *remaining -= 1;
                            keep.push(row as u32);
                        }
                    }
                    SetOpMode::ExceptAll => {
                        let remaining = self.right_counts.as_mut().unwrap().entry(key).or_insert(0);
                        if *remaining > 0 {
                            *remaining -= 1;
                        } else {
                            keep.push(row as u32);
                        }
                    }
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
                        .map_err(|e| KaveonError::Execution(format!("setop take: {e}")))
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
        DataType::Dictionary(key_type, _) if key_type.as_ref() == &DataType::Int32 => {
            let dictionary = array.as_dictionary::<Int32Type>();
            extract_value(
                dictionary.values().as_ref(),
                dictionary.keys().value(row) as usize,
            )
        }
        DataType::Boolean => Ok(AggregateValue::Bool(array.as_boolean().value(row))),
        DataType::Int8 => Ok(AggregateValue::Int32(
            array
                .as_primitive::<arrow::datatypes::Int8Type>()
                .value(row) as i32,
        )),
        DataType::Int16 => Ok(AggregateValue::Int32(
            array
                .as_primitive::<arrow::datatypes::Int16Type>()
                .value(row) as i32,
        )),
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
        DataType::UInt8 => Ok(AggregateValue::Int32(
            array
                .as_primitive::<arrow::datatypes::UInt8Type>()
                .value(row) as i32,
        )),
        DataType::UInt16 => Ok(AggregateValue::Int32(
            array
                .as_primitive::<arrow::datatypes::UInt16Type>()
                .value(row) as i32,
        )),
        DataType::UInt32 => Ok(AggregateValue::Int64(
            array
                .as_primitive::<arrow::datatypes::UInt32Type>()
                .value(row) as i64,
        )),
        DataType::UInt64 => Ok(AggregateValue::Int64(
            array
                .as_primitive::<arrow::datatypes::UInt64Type>()
                .value(row) as i64,
        )),
        DataType::List(_) | DataType::LargeList(_) | DataType::Map(_, _) | DataType::Struct(_) => {
            Ok(AggregateValue::Utf8(crate::distinct::complex_value_key(
                array, row,
            )?))
        }
        DataType::Date32 => Ok(AggregateValue::Int32(
            array
                .as_primitive::<arrow::datatypes::Date32Type>()
                .value(row),
        )),
        DataType::Date64 => Ok(AggregateValue::Int64(
            array
                .as_primitive::<arrow::datatypes::Date64Type>()
                .value(row),
        )),
        DataType::Timestamp(unit, _) => Ok(AggregateValue::Int64(match unit {
            arrow::datatypes::TimeUnit::Second => array
                .as_primitive::<arrow::datatypes::TimestampSecondType>()
                .value(row),
            arrow::datatypes::TimeUnit::Millisecond => array
                .as_primitive::<arrow::datatypes::TimestampMillisecondType>()
                .value(row),
            arrow::datatypes::TimeUnit::Microsecond => array
                .as_primitive::<arrow::datatypes::TimestampMicrosecondType>()
                .value(row),
            arrow::datatypes::TimeUnit::Nanosecond => array
                .as_primitive::<arrow::datatypes::TimestampNanosecondType>()
                .value(row),
        })),
        DataType::Decimal128(_, scale) => Ok(AggregateValue::Decimal128(
            array
                .as_primitive::<arrow::datatypes::Decimal128Type>()
                .value(row),
            *scale,
        )),
        DataType::Float32 => {
            let value = array
                .as_primitive::<arrow::datatypes::Float32Type>()
                .value(row) as f64;
            Ok(AggregateValue::Float64Bits(if value == 0.0 {
                0
            } else if value.is_nan() {
                f64::NAN.to_bits()
            } else {
                value.to_bits()
            }))
        }
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
                .expect("Utf8")
                .value(row)
                .to_owned(),
        )),
        DataType::LargeUtf8 => Ok(AggregateValue::Utf8(
            array
                .as_any()
                .downcast_ref::<arrow::array::LargeStringArray>()
                .expect("LargeUtf8")
                .value(row)
                .to_owned(),
        )),
        dt => Err(KaveonError::Execution(format!(
            "set operation not supported for type {dt}"
        ))),
    }
}

#[cfg(test)]
mod tests {
    use std::collections::VecDeque;
    use std::sync::Arc;

    use arrow::array::{
        ArrayRef, Date32Array, Decimal128Array, DictionaryArray, Float32Array, Int32Array,
        Int32Builder, LargeStringArray, ListArray, ListBuilder, MapArray, StringArray, StructArray,
        UInt8Array,
    };
    use arrow::buffer::{OffsetBuffer, ScalarBuffer};
    use arrow::datatypes::{DataType, Field, Schema};

    use super::*;

    struct Source {
        schema: SchemaRef,
        batches: VecDeque<RecordBatch>,
    }

    impl BatchOperator for Source {
        fn schema(&self) -> &SchemaRef {
            &self.schema
        }
        fn next_batch(&mut self) -> Result<Option<RecordBatch>> {
            Ok(self.batches.pop_front())
        }
    }

    fn side(column: ArrayRef) -> Box<dyn BatchOperator> {
        let schema = Arc::new(Schema::new(vec![Field::new(
            "country",
            column.data_type().clone(),
            true,
        )]));
        let batch = RecordBatch::try_new(Arc::clone(&schema), vec![column]).unwrap();
        Box::new(Source {
            schema,
            batches: VecDeque::from(vec![batch]),
        })
    }

    fn strings(batches: Vec<RecordBatch>) -> Vec<Option<String>> {
        let mut rows = Vec::new();
        for batch in batches {
            let column = arrow::compute::cast(batch.column(0), &DataType::Utf8).unwrap();
            rows.extend(
                column
                    .as_string::<i32>()
                    .iter()
                    .map(|value| value.map(str::to_owned)),
            );
        }
        rows.sort();
        rows
    }

    #[test]
    fn dictionary_and_plain_sides_intersect_and_except_by_value() {
        let dictionary = || -> ArrayRef {
            Arc::new(DictionaryArray::<Int32Type>::new(
                Int32Array::from(vec![Some(0), Some(1), Some(0), None, Some(2)]),
                Arc::new(StringArray::from(vec!["jp", "in", "kr"])),
            ))
        };
        let plain =
            || -> ArrayRef { Arc::new(StringArray::from(vec![Some("in"), Some("kr"), None])) };
        for (mode, expected) in [
            (
                SetOpMode::Intersect,
                vec![None, Some("in".to_owned()), Some("kr".to_owned())],
            ),
            (SetOpMode::Except, vec![Some("jp".to_owned())]),
        ] {
            let mut operator = SetOpOperator::new(side(dictionary()), side(plain()), mode);
            let mut batches = Vec::new();
            while let Some(batch) = operator.next_batch().unwrap() {
                batches.push(batch);
            }
            assert_eq!(strings(batches), expected);
        }
    }

    #[test]
    fn multiset_modes_preserve_duplicate_counts() {
        let left = || -> ArrayRef {
            Arc::new(StringArray::from(vec![
                Some("a"),
                Some("a"),
                Some("a"),
                Some("b"),
            ]))
        };
        let right =
            || -> ArrayRef { Arc::new(StringArray::from(vec![Some("a"), Some("a"), Some("c")])) };
        for (mode, expected) in [
            (
                SetOpMode::IntersectAll,
                vec![Some("a".to_owned()), Some("a".to_owned())],
            ),
            (
                SetOpMode::ExceptAll,
                vec![Some("a".to_owned()), Some("b".to_owned())],
            ),
        ] {
            let mut operator = SetOpOperator::new(side(left()), side(right()), mode);
            let mut batches = Vec::new();
            while let Some(batch) = operator.next_batch().unwrap() {
                batches.push(batch);
            }
            assert_eq!(strings(batches), expected);
        }
    }

    #[test]
    fn typed_temporal_decimal_and_large_text_keys_are_value_comparable() {
        let decimal = |values: Vec<Option<i128>>| -> ArrayRef {
            Arc::new(
                Decimal128Array::from(values)
                    .with_precision_and_scale(12, 2)
                    .unwrap(),
            )
        };
        let mut decimal_op = SetOpOperator::new(
            side(decimal(vec![Some(100), Some(250), Some(999)])),
            side(decimal(vec![Some(250), Some(300)])),
            SetOpMode::Intersect,
        );
        let decimal_batch = decimal_op.next_batch().unwrap().unwrap();
        let decimal_values = decimal_batch
            .column(0)
            .as_primitive::<arrow::datatypes::Decimal128Type>()
            .values()
            .to_vec();
        assert_eq!(decimal_values, vec![250]);

        let mut date_op = SetOpOperator::new(
            side(Arc::new(Date32Array::from(vec![
                Some(20_000),
                Some(20_001),
            ]))),
            side(Arc::new(Date32Array::from(vec![
                Some(20_001),
                Some(20_002),
            ]))),
            SetOpMode::Except,
        );
        let date_batch = date_op.next_batch().unwrap().unwrap();
        assert_eq!(
            date_batch
                .column(0)
                .as_primitive::<arrow::datatypes::Date32Type>()
                .values(),
            &[20_000]
        );

        let mut large_text_op = SetOpOperator::new(
            side(Arc::new(LargeStringArray::from(vec![
                Some("alpha"),
                Some("beta"),
            ]))),
            side(Arc::new(LargeStringArray::from(vec![Some("beta")]))),
            SetOpMode::Intersect,
        );
        let large_text_batch = large_text_op.next_batch().unwrap().unwrap();
        assert_eq!(
            large_text_batch
                .column(0)
                .as_any()
                .downcast_ref::<LargeStringArray>()
                .unwrap()
                .value(0),
            "beta"
        );

        let mut narrow_numeric_op = SetOpOperator::new(
            side(Arc::new(Float32Array::from(vec![Some(1.5), Some(2.5)]))),
            side(Arc::new(Float32Array::from(vec![Some(2.5)]))),
            SetOpMode::Intersect,
        );
        let narrow_numeric_batch = narrow_numeric_op.next_batch().unwrap().unwrap();
        assert_eq!(
            narrow_numeric_batch
                .column(0)
                .as_primitive::<arrow::datatypes::Float32Type>()
                .value(0),
            2.5
        );

        let mut narrow_integer_op = SetOpOperator::new(
            side(Arc::new(UInt8Array::from(vec![1_u8, 2_u8]))),
            side(Arc::new(UInt8Array::from(vec![2_u8]))),
            SetOpMode::Intersect,
        );
        let narrow_integer_batch = narrow_integer_op.next_batch().unwrap().unwrap();
        assert_eq!(
            narrow_integer_batch
                .column(0)
                .as_primitive::<arrow::datatypes::UInt8Type>()
                .value(0),
            2
        );

        let lists = |rows: &[&[i32]]| -> ArrayRef {
            let mut builder = ListBuilder::new(Int32Builder::new());
            for row in rows {
                for value in *row {
                    builder.values().append_value(*value);
                }
                builder.append(true);
            }
            Arc::new(builder.finish())
        };
        let mut list_op = SetOpOperator::new(
            side(lists(&[&[1, 2], &[3]])),
            side(lists(&[&[3], &[4]])),
            SetOpMode::Intersect,
        );
        let list_batch = list_op.next_batch().unwrap().unwrap();
        let list = list_batch
            .column(0)
            .as_any()
            .downcast_ref::<ListArray>()
            .unwrap();
        assert_eq!(
            list.value(0)
                .as_primitive::<arrow::datatypes::Int32Type>()
                .values(),
            &[3]
        );

        let map = |keys: &[&str], values: &[i32]| -> ArrayRef {
            let fields: arrow::datatypes::Fields = vec![
                Arc::new(Field::new("key", DataType::Utf8, false)),
                Arc::new(Field::new("value", DataType::Int32, true)),
            ]
            .into();
            let entries = StructArray::new(
                fields.clone(),
                vec![
                    Arc::new(StringArray::from(keys.to_vec())) as ArrayRef,
                    Arc::new(Int32Array::from(values.to_vec())) as ArrayRef,
                ],
                None,
            );
            Arc::new(
                MapArray::try_new(
                    Arc::new(Field::new("entries", DataType::Struct(fields), false)),
                    OffsetBuffer::new(ScalarBuffer::from(vec![0_i32, keys.len() as i32])),
                    entries,
                    None,
                    false,
                )
                .unwrap(),
            )
        };
        let mut map_op = SetOpOperator::new(
            side(map(&["a", "b"], &[1, 2])),
            side(map(&["b", "a"], &[2, 1])),
            SetOpMode::Intersect,
        );
        assert!(map_op.next_batch().unwrap().is_some());
    }
}
