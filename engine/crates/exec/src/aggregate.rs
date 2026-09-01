use arrow::array::{Array, Float64Array, Int64Array, UInt64Array};
use arrow::datatypes::{DataType, Field, Schema};
use arrow::record_batch::RecordBatch;
use kaveon_core::{KaveonError, Result};
use std::collections::HashMap;
use std::sync::Arc;

#[derive(Debug, Clone)]
pub enum AggOp {
    Sum,
    Count,
    Min,
    Max,
}

pub struct HashAggregate {
    group_cols: Vec<String>,
    agg_col: String,
    agg_op: AggOp,
}

impl HashAggregate {
    pub fn new(group_cols: Vec<String>, agg_col: String, agg_op: AggOp) -> Self {
        Self {
            group_cols,
            agg_col,
            agg_op,
        }
    }

    pub fn execute(&self, batches: &[RecordBatch]) -> Result<RecordBatch> {
        if batches.is_empty() {
            return Err(KaveonError::Execution("no input batches".into()));
        }

        let mut accum: HashMap<Vec<String>, (f64, u64)> = HashMap::new();

        for batch in batches {
            let agg_idx = batch
                .schema()
                .index_of(&self.agg_col)
                .map_err(|e| KaveonError::Execution(e.to_string()))?;
            let agg_arr = batch.column(agg_idx);

            let group_indices: Vec<usize> = self
                .group_cols
                .iter()
                .map(|c| {
                    batch
                        .schema()
                        .index_of(c)
                        .map_err(|e| KaveonError::Execution(e.to_string()))
                })
                .collect::<Result<_>>()?;

            for row in 0..batch.num_rows() {
                let key: Vec<String> = group_indices
                    .iter()
                    .map(|&idx| format!("{:?}", batch.column(idx).slice(row, 1)))
                    .collect();

                let val = extract_f64(agg_arr, row);
                let entry = accum.entry(key).or_insert((0.0, 0));

                match self.agg_op {
                    AggOp::Sum => {
                        entry.0 += val;
                        entry.1 += 1;
                    }
                    AggOp::Count => {
                        entry.1 += 1;
                    }
                    AggOp::Min => {
                        if entry.1 == 0 || val < entry.0 {
                            entry.0 = val;
                        }
                        entry.1 += 1;
                    }
                    AggOp::Max => {
                        if entry.1 == 0 || val > entry.0 {
                            entry.0 = val;
                        }
                        entry.1 += 1;
                    }
                }
            }
        }

        let count = accum.len();
        let mut result_values = Vec::with_capacity(count);
        let mut result_counts = Vec::with_capacity(count);

        for (_key, (val, cnt)) in &accum {
            result_values.push(*val);
            result_counts.push(*cnt);
        }

        let schema = Arc::new(Schema::new(vec![
            Field::new("agg_value", DataType::Float64, false),
            Field::new("agg_count", DataType::UInt64, false),
        ]));

        let batch = RecordBatch::try_new(
            schema,
            vec![
                Arc::new(Float64Array::from(result_values)),
                Arc::new(UInt64Array::from(result_counts)),
            ],
        )?;

        Ok(batch)
    }
}

fn extract_f64(arr: &dyn Array, row: usize) -> f64 {
    if let Some(a) = arr.as_any().downcast_ref::<Float64Array>() {
        a.value(row)
    } else if let Some(a) = arr.as_any().downcast_ref::<Int64Array>() {
        a.value(row) as f64
    } else {
        0.0
    }
}
