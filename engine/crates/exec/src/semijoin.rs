use std::collections::HashSet;

use arrow::array::{Array, ArrayRef, AsArray, RecordBatch, StringArray};
use arrow::datatypes::{Float64Type, Int32Type, Int64Type, SchemaRef, UInt64Type};
use kaveon_core::{
    BatchOperator, Expr, KaveonError, OperatorMemoryAccount, ReservationSlab, Result,
};

#[derive(Debug, PartialEq, Eq, Hash)]
enum Key {
    Bool(bool),
    Number(i128, i8),
    Float(u64),
    Text(String),
}

pub struct SemiJoinOperator {
    left: Box<dyn BatchOperator>,
    right: Box<dyn BatchOperator>,
    left_key: Expr,
    right_key: Expr,
    anti: bool,
    right_keys: Option<HashSet<Key>>,
    right_has_null: bool,
    memory: Option<OperatorMemoryAccount>,
    reservations: ReservationSlab,
}

impl SemiJoinOperator {
    pub fn new(
        left: Box<dyn BatchOperator>,
        right: Box<dyn BatchOperator>,
        mut left_key: Expr,
        right_key: Expr,
        anti: bool,
    ) -> Result<Self> {
        let mut right_key = if matches!(right_key, Expr::Literal(_)) {
            right_key
        } else {
            if right.schema().fields().len() != 1 {
                return Err(KaveonError::Execution(
                    "IN subquery must return exactly one column".into(),
                ));
            }
            Expr::Column(right.schema().field(0).name().clone())
        };
        // Keys compare by value; a dictionary-encoded side is its value type.
        let left_type = logical_type(
            crate::expr_eval::evaluate(&left_key, &RecordBatch::new_empty(left.schema().clone()))?
                .data_type(),
        );
        let right_type = logical_type(
            crate::expr_eval::evaluate(
                &right_key,
                &RecordBatch::new_empty(right.schema().clone()),
            )?
            .data_type(),
        );
        if left_type.is_numeric()
            && right_type.is_numeric()
            && (left_type == arrow::datatypes::DataType::Float64
                || right_type == arrow::datatypes::DataType::Float64)
        {
            left_key = Expr::Cast {
                expr: Box::new(left_key),
                data_type: kaveon_core::CastTarget::Float64,
            };
            right_key = Expr::Cast {
                expr: Box::new(right_key),
                data_type: kaveon_core::CastTarget::Float64,
            };
        } else if left_type != right_type
            && !(left_type.is_numeric() && right_type.is_numeric())
            && left_type != arrow::datatypes::DataType::Null
            && right_type != arrow::datatypes::DataType::Null
        {
            return Err(KaveonError::Execution(format!(
                "incompatible IN key types: {left_type} and {right_type}"
            )));
        }
        Ok(Self {
            left,
            right,
            left_key,
            right_key,
            anti,
            right_keys: None,
            right_has_null: false,
            memory: None,
            reservations: ReservationSlab::default(),
        })
    }

    pub fn with_memory(mut self, memory: OperatorMemoryAccount) -> Self {
        self.memory = Some(memory);
        self
    }

    fn build_right_keys(&mut self) -> Result<()> {
        let mut keys = HashSet::new();
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
            let col = crate::expr_eval::evaluate(&self.right_key, &batch)?;
            for row in 0..batch.num_rows() {
                if row % 1024 == 0 {
                    crate::expr_eval::check_expression_cancelled()?;
                }
                if !col.is_null(row) {
                    let key = extract_value(col.as_ref(), row)?;
                    if !keys.contains(&key) {
                        if let Some(memory) = &self.memory {
                            let bytes = 128_u64.saturating_add(match &key {
                                Key::Text(value) => value.len() as u64,
                                _ => 0,
                            });
                            self.reservations.reserve(memory, bytes)?;
                        }
                        keys.insert(key);
                    }
                } else {
                    self.right_has_null = true;
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
        let expression_memory = self.memory.clone();
        crate::expr_eval::with_expression_memory(expression_memory.as_ref(), || {
            if self.right_keys.is_none() {
                self.build_right_keys()?;
            }
            let keys = self.right_keys.as_ref().unwrap();

            loop {
                let batch = match self.left.next_batch()? {
                    Some(b) => b,
                    None => {
                        self.right_keys = Some(HashSet::new());
                        self.reservations.clear();
                        return Ok(None);
                    }
                };

                let _workspace = self
                    .memory
                    .as_ref()
                    .map(|memory| {
                        memory.reserve(
                            (batch.get_array_memory_size() as u64)
                                .saturating_mul(4)
                                .saturating_add((batch.num_rows() as u64).saturating_mul(32)),
                        )
                    })
                    .transpose()?;

                let col = crate::expr_eval::evaluate(&self.left_key, &batch)?;
                // A dictionary column is probed once per distinct value, then
                // every row reads its value's verdict.
                let (col, verdicts, dictionary_keys) = match col.data_type() {
                    arrow::datatypes::DataType::Dictionary(key_type, _)
                        if key_type.as_ref() == &arrow::datatypes::DataType::Int32 =>
                    {
                        let dictionary = col.as_dictionary::<Int32Type>();
                        let values = dictionary.values().clone();
                        let verdicts = (0..values.len())
                            .map(|index| {
                                if values.is_null(index) {
                                    Ok(false)
                                } else {
                                    Ok(keys.contains(&extract_value(values.as_ref(), index)?))
                                }
                            })
                            .collect::<Result<Vec<bool>>>()?;
                        (values, Some(verdicts), Some(dictionary.keys().clone()))
                    }
                    _ => (col, None, None),
                };
                let mut indices = Vec::new();
                for row in 0..batch.num_rows() {
                    let is_null = match &dictionary_keys {
                        Some(dictionary_keys) => {
                            dictionary_keys.is_null(row)
                                || col.is_null(dictionary_keys.value(row) as usize)
                        }
                        None => col.is_null(row),
                    };
                    if is_null {
                        if self.anti && keys.is_empty() && !self.right_has_null {
                            indices.push(row);
                        }
                        continue;
                    }
                    let found = match (&verdicts, &dictionary_keys) {
                        (Some(verdicts), Some(dictionary_keys)) => {
                            verdicts[dictionary_keys.value(row) as usize]
                        }
                        _ => keys.contains(&extract_value(col.as_ref(), row)?),
                    };
                    if (found && !self.anti) || (!found && self.anti && !self.right_has_null) {
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
        })
    }
}

fn logical_type(data_type: &arrow::datatypes::DataType) -> arrow::datatypes::DataType {
    match data_type {
        arrow::datatypes::DataType::Dictionary(_, values) => values.as_ref().clone(),
        other => other.clone(),
    }
}

fn extract_value(array: &dyn Array, row: usize) -> Result<Key> {
    match array.data_type() {
        arrow::datatypes::DataType::Dictionary(key_type, _)
            if key_type.as_ref() == &arrow::datatypes::DataType::Int32 =>
        {
            let dictionary = array.as_dictionary::<Int32Type>();
            extract_value(
                dictionary.values().as_ref(),
                dictionary.keys().value(row) as usize,
            )
        }
        arrow::datatypes::DataType::Boolean => Ok(Key::Bool(array.as_boolean().value(row))),
        arrow::datatypes::DataType::Int32 => Ok(Key::Number(
            array.as_primitive::<Int32Type>().value(row) as i128,
            0,
        )),
        arrow::datatypes::DataType::Int64 => Ok(Key::Number(
            array.as_primitive::<Int64Type>().value(row) as i128,
            0,
        )),
        arrow::datatypes::DataType::UInt64 => Ok(Key::Number(
            array.as_primitive::<UInt64Type>().value(row) as i128,
            0,
        )),
        arrow::datatypes::DataType::Decimal128(_, scale) => {
            let mut value = array
                .as_primitive::<arrow::datatypes::Decimal128Type>()
                .value(row);
            let mut scale = *scale;
            while scale > 0 && value % 10 == 0 {
                value /= 10;
                scale -= 1;
            }
            if scale < 0 {
                value = value
                    .checked_mul(
                        10i128
                            .checked_pow((-scale) as u32)
                            .ok_or_else(|| KaveonError::Execution("decimal key overflow".into()))?,
                    )
                    .ok_or_else(|| KaveonError::Execution("decimal key overflow".into()))?;
                scale = 0;
            }
            Ok(Key::Number(value, scale))
        }
        arrow::datatypes::DataType::Float64 => {
            let v = array.as_primitive::<Float64Type>().value(row);
            Ok(Key::Float(if v == 0.0 {
                0
            } else if v.is_nan() {
                f64::NAN.to_bits()
            } else {
                v.to_bits()
            }))
        }
        arrow::datatypes::DataType::Utf8 => Ok(Key::Text(
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

#[cfg(test)]
mod tests {
    use super::*;
    use arrow::{
        array::Int64Array,
        datatypes::{DataType, Field, Schema},
    };
    use std::sync::Arc;
    struct Source {
        schema: SchemaRef,
        batches: std::vec::IntoIter<RecordBatch>,
    }
    impl BatchOperator for Source {
        fn schema(&self) -> &SchemaRef {
            &self.schema
        }
        fn next_batch(&mut self) -> Result<Option<RecordBatch>> {
            Ok(self.batches.next())
        }
    }
    fn source(values: &[Option<i64>]) -> Box<dyn BatchOperator> {
        let schema = Arc::new(Schema::new(vec![Field::new("x", DataType::Int64, true)]));
        let batches = values
            .chunks(2)
            .map(|v| {
                RecordBatch::try_new(schema.clone(), vec![Arc::new(Int64Array::from(v.to_vec()))])
                    .unwrap()
            })
            .collect::<Vec<_>>()
            .into_iter();
        Box::new(Source { schema, batches })
    }
    fn run(
        left: &[Option<i64>],
        right: &[Option<i64>],
        anti: bool,
        exists: bool,
    ) -> Vec<Option<i64>> {
        let key = if exists {
            Expr::Literal(kaveon_core::predicate::ScalarValue::Int64(1))
        } else {
            Expr::Column("x".into())
        };
        let mut op =
            SemiJoinOperator::new(source(left), source(right), key.clone(), key, anti).unwrap();
        let mut values = Vec::new();
        while let Some(batch) = op.next_batch().unwrap() {
            values.extend(batch.column(0).as_primitive::<Int64Type>().iter());
        }
        values
    }
    #[test]
    fn dictionary_left_keys_probe_by_value() {
        use arrow::array::{DictionaryArray, Int32Array, StringArray};
        let dictionary_type =
            DataType::Dictionary(Box::new(DataType::Int32), Box::new(DataType::Utf8));
        let left_schema = Arc::new(Schema::new(vec![Field::new(
            "country",
            dictionary_type,
            true,
        )]));
        let left = RecordBatch::try_new(
            left_schema.clone(),
            vec![Arc::new(DictionaryArray::<Int32Type>::new(
                Int32Array::from(vec![Some(0), Some(1), None, Some(2), Some(1)]),
                Arc::new(StringArray::from(vec![Some("Japan"), None, Some("Kenya")])),
            ))],
        )
        .unwrap();
        let right_schema = Arc::new(Schema::new(vec![Field::new(
            "country",
            DataType::Utf8,
            true,
        )]));
        let right = RecordBatch::try_new(
            right_schema.clone(),
            vec![Arc::new(StringArray::from(vec!["Kenya", "Brazil"]))],
        )
        .unwrap();
        let run = |anti: bool| {
            let mut op = SemiJoinOperator::new(
                Box::new(Source {
                    schema: left_schema.clone(),
                    batches: vec![left.clone()].into_iter(),
                }),
                Box::new(Source {
                    schema: right_schema.clone(),
                    batches: vec![right.clone()].into_iter(),
                }),
                Expr::Column("country".into()),
                Expr::Column("country".into()),
                anti,
            )
            .unwrap();
            let mut rows = Vec::new();
            while let Some(batch) = op.next_batch().unwrap() {
                let column = arrow::compute::cast(batch.column(0), &DataType::Utf8).unwrap();
                rows.extend(
                    column
                        .as_string::<i32>()
                        .iter()
                        .map(|value| value.map(str::to_owned)),
                );
            }
            rows
        };
        assert_eq!(run(false), vec![Some("Kenya".to_owned())]);
        assert_eq!(run(true), vec![Some("Japan".to_owned())]);
    }

    #[test]
    fn not_in_null_truth_table_across_batches() {
        let left = [Some(1), Some(2), None, Some(2)];
        assert_eq!(
            run(&left, &[Some(1), None], true, false),
            Vec::<Option<i64>>::new()
        );
        assert_eq!(run(&left, &[Some(1)], true, false), vec![Some(2), Some(2)]);
        assert_eq!(run(&left, &[], true, false), left);
        assert_eq!(
            run(&left, &[Some(2), Some(2), None], false, false),
            vec![Some(2), Some(2)]
        );
    }
    #[test]
    fn exists_checks_cardinality_and_preserves_outer_duplicates_nulls() {
        let left = [Some(1), Some(2), None, Some(2)];
        assert_eq!(run(&left, &[None], false, true), left);
        assert_eq!(
            run(&left, &[Some(99), None], true, true),
            Vec::<Option<i64>>::new()
        );
        assert_eq!(run(&left, &[], true, true), left);
        assert_eq!(run(&left, &[], false, true), Vec::<Option<i64>>::new());
    }
    #[test]
    fn numeric_keys_are_exact_across_widths_and_zero_signs() {
        assert_eq!(
            extract_value(&arrow::array::Int32Array::from(vec![42]), 0).unwrap(),
            extract_value(&Int64Array::from(vec![42]), 0).unwrap()
        );
        assert_ne!(
            extract_value(&arrow::array::UInt64Array::from(vec![u64::MAX]), 0).unwrap(),
            extract_value(&Int64Array::from(vec![-1]), 0).unwrap()
        );
        assert_eq!(
            extract_value(&arrow::array::Float64Array::from(vec![-0.0]), 0).unwrap(),
            extract_value(&arrow::array::Float64Array::from(vec![0.0]), 0).unwrap()
        );
    }
}
