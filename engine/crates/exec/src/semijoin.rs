use std::collections::HashSet;

use arrow::array::{Array, ArrayRef, AsArray, RecordBatch, StringArray};
use arrow::datatypes::{Float64Type, Int32Type, Int64Type, SchemaRef, UInt64Type};
use kaveon_core::{
    BatchOperator, Expr, KaveonError, MemoryReservation, OperatorMemoryAccount, Result,
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
    reservations: Vec<MemoryReservation>,
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
        let left_type =
            crate::expr_eval::evaluate(&left_key, &RecordBatch::new_empty(left.schema().clone()))?
                .data_type()
                .clone();
        let right_type = crate::expr_eval::evaluate(
            &right_key,
            &RecordBatch::new_empty(right.schema().clone()),
        )?
        .data_type()
        .clone();
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
            reservations: Vec::new(),
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
                            self.reservations.push(memory.reserve(bytes)?);
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
                let mut indices = Vec::new();
                for row in 0..batch.num_rows() {
                    if col.is_null(row) {
                        if self.anti && keys.is_empty() && !self.right_has_null {
                            indices.push(row);
                        }
                        continue;
                    }
                    let val = extract_value(col.as_ref(), row)?;
                    let found = keys.contains(&val);
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

fn extract_value(array: &dyn Array, row: usize) -> Result<Key> {
    match array.data_type() {
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
