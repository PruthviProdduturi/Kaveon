use std::collections::HashMap;

use arrow::array::{Array, ArrayRef, AsArray, Float64Array, Int64Array, RecordBatch, StringArray};
use arrow::datatypes::{DataType, Field, Float64Type, Int64Type, Schema, SchemaRef};
use kaveon_core::{BatchOperator, Expr, KaveonError, Result};
use std::sync::Arc;

use crate::aggregate::AggregateValue;

pub struct WindowOperator {
    source: Box<dyn BatchOperator>,
    window_exprs: Vec<Expr>,
    output_schema: Option<SchemaRef>,
    buffered: Vec<RecordBatch>,
    emitted: bool,
}

impl WindowOperator {
    pub fn new(source: Box<dyn BatchOperator>, window_exprs: Vec<Expr>) -> Self {
        Self {
            source,
            window_exprs,
            output_schema: None,
            buffered: Vec::new(),
            emitted: false,
        }
    }
}

impl BatchOperator for WindowOperator {
    fn schema(&self) -> &SchemaRef {
        if let Some(ref schema) = self.output_schema {
            return schema;
        }
        self.source.schema()
    }

    fn next_batch(&mut self) -> Result<Option<RecordBatch>> {
        if self.emitted {
            return Ok(None);
        }

        while let Some(batch) = self.source.next_batch()? {
            self.buffered.push(batch);
        }

        if self.buffered.is_empty() {
            self.emitted = true;
            return Ok(None);
        }

        let combined = arrow::compute::concat_batches(self.source.schema(), &self.buffered)
            .map_err(|e| KaveonError::Execution(format!("window concat: {e}")))?;
        self.buffered.clear();

        let num_rows = combined.num_rows();
        let mut result_columns: Vec<ArrayRef> = combined.columns().to_vec();
        let mut fields: Vec<Field> = combined
            .schema()
            .fields()
            .iter()
            .map(|f| f.as_ref().clone())
            .collect();

        for (idx, expr) in self.window_exprs.iter().enumerate() {
            let Expr::WindowFunction {
                name,
                args,
                partition_by,
                order_by,
                frame,
            } = expr
            else {
                return Err(KaveonError::Execution(
                    "window_exprs must contain WindowFunction expressions".into(),
                ));
            };

            let partitions = compute_partitions(&combined, partition_by)?;
            let sort_indices = if order_by.is_empty() {
                None
            } else {
                Some(compute_sort_within_partitions(
                    &combined,
                    &partitions,
                    order_by,
                )?)
            };

            let result = evaluate_window_function(
                name,
                args,
                &combined,
                &partitions,
                sort_indices.as_deref(),
                num_rows,
                frame.as_ref(),
            )?;

            let col_name = window_output_name(name, args, idx);
            fields.push(Field::new(&col_name, result.data_type().clone(), true));
            result_columns.push(result);
        }

        let output_schema = Arc::new(Schema::new(fields));
        let result = RecordBatch::try_new(output_schema.clone(), result_columns)
            .map_err(|e| KaveonError::Execution(format!("window result batch: {e}")))?;
        self.output_schema = Some(output_schema);
        self.emitted = true;
        Ok(Some(result))
    }
}

fn window_output_name(name: &str, args: &[Expr], idx: usize) -> String {
    let arg_str = args
        .iter()
        .map(|a| match a {
            Expr::Column(c) => c.clone(),
            Expr::Star => "*".into(),
            _ => "expr".into(),
        })
        .collect::<Vec<_>>()
        .join("_");
    if arg_str.is_empty() {
        format!("{}_{}", name.to_lowercase(), idx)
    } else {
        format!("{}_{}", name.to_lowercase(), arg_str)
    }
}

type PartitionMap = Vec<(Vec<AggregateValue>, Vec<usize>)>;

fn compute_partitions(batch: &RecordBatch, partition_by: &[Expr]) -> Result<PartitionMap> {
    if partition_by.is_empty() {
        let all: Vec<usize> = (0..batch.num_rows()).collect();
        return Ok(vec![(Vec::new(), all)]);
    }

    let partition_cols: Vec<ArrayRef> = partition_by
        .iter()
        .map(|e| crate::expr_eval::evaluate(e, batch))
        .collect::<Result<_>>()?;

    let mut groups: HashMap<Vec<AggregateValue>, Vec<usize>> = HashMap::new();
    let mut order: Vec<Vec<AggregateValue>> = Vec::new();

    for row in 0..batch.num_rows() {
        let key: Vec<AggregateValue> = partition_cols
            .iter()
            .map(|col| extract_value(col.as_ref(), row))
            .collect::<Result<_>>()?;
        if !groups.contains_key(&key) {
            order.push(key.clone());
        }
        groups.entry(key).or_default().push(row);
    }

    Ok(order
        .into_iter()
        .map(|k| {
            let rows = groups.remove(&k).unwrap();
            (k, rows)
        })
        .collect())
}

fn compute_sort_within_partitions(
    batch: &RecordBatch,
    partitions: &PartitionMap,
    order_by: &[(Expr, bool)],
) -> Result<Vec<usize>> {
    let sort_cols: Vec<ArrayRef> = order_by
        .iter()
        .map(|(e, _)| crate::expr_eval::evaluate(e, batch))
        .collect::<Result<_>>()?;

    let mut global_order = vec![0usize; batch.num_rows()];

    for (_, rows) in partitions {
        let mut sorted_rows = rows.clone();
        sorted_rows.sort_by(|&a, &b| {
            for (col_idx, (_, asc)) in order_by.iter().enumerate() {
                let col = &sort_cols[col_idx];
                let cmp = compare_array_values(col.as_ref(), a, b);
                let cmp = if *asc { cmp } else { cmp.reverse() };
                if cmp != std::cmp::Ordering::Equal {
                    return cmp;
                }
            }
            std::cmp::Ordering::Equal
        });
        for (rank, &row) in sorted_rows.iter().enumerate() {
            global_order[row] = rank;
        }
    }

    Ok(global_order)
}

fn evaluate_window_function(
    name: &str,
    args: &[Expr],
    batch: &RecordBatch,
    partitions: &PartitionMap,
    sort_order: Option<&[usize]>,
    num_rows: usize,
    frame: Option<&kaveon_core::WindowFrame>,
) -> Result<ArrayRef> {
    match name.to_uppercase().as_str() {
        "ROW_NUMBER" => {
            let mut result = vec![0i64; num_rows];
            for (_, rows) in partitions {
                let mut sorted = rows.clone();
                if let Some(order) = sort_order {
                    sorted.sort_by_key(|&r| order[r]);
                }
                for (i, &row) in sorted.iter().enumerate() {
                    result[row] = (i + 1) as i64;
                }
            }
            Ok(Arc::new(Int64Array::from(result)))
        }
        "RANK" => {
            let mut result = vec![0i64; num_rows];
            let sort_cols: Vec<ArrayRef> = args
                .iter()
                .chain(std::iter::empty())
                .filter_map(|_| None::<ArrayRef>)
                .collect();
            let _ = sort_cols;
            for (_, rows) in partitions {
                let mut sorted = rows.clone();
                if let Some(order) = sort_order {
                    sorted.sort_by_key(|&r| order[r]);
                }
                if sorted.is_empty() {
                    continue;
                }
                result[sorted[0]] = 1;
                for i in 1..sorted.len() {
                    let same = sort_order
                        .map(|o| o[sorted[i]] == o[sorted[i - 1]])
                        .unwrap_or(false);
                    result[sorted[i]] = if same {
                        result[sorted[i - 1]]
                    } else {
                        (i + 1) as i64
                    };
                }
            }
            Ok(Arc::new(Int64Array::from(result)))
        }
        "DENSE_RANK" => {
            let mut result = vec![0i64; num_rows];
            for (_, rows) in partitions {
                let mut sorted = rows.clone();
                if let Some(order) = sort_order {
                    sorted.sort_by_key(|&r| order[r]);
                }
                if sorted.is_empty() {
                    continue;
                }
                result[sorted[0]] = 1;
                let mut rank = 1i64;
                for i in 1..sorted.len() {
                    let same = sort_order
                        .map(|o| o[sorted[i]] == o[sorted[i - 1]])
                        .unwrap_or(false);
                    if !same {
                        rank += 1;
                    }
                    result[sorted[i]] = rank;
                }
            }
            Ok(Arc::new(Int64Array::from(result)))
        }
        "NTILE" => {
            if args.is_empty() {
                return Err(KaveonError::Execution("NTILE requires one argument".into()));
            }
            let n = match &args[0] {
                Expr::Literal(kaveon_core::predicate::ScalarValue::Int64(v)) => *v as usize,
                _ => {
                    return Err(KaveonError::Execution(
                        "NTILE argument must be an integer literal".into(),
                    ));
                }
            };
            if n == 0 {
                return Err(KaveonError::Execution(
                    "NTILE argument must be positive".into(),
                ));
            }
            let mut result = vec![0i64; num_rows];
            for (_, rows) in partitions {
                let mut sorted = rows.clone();
                if let Some(order) = sort_order {
                    sorted.sort_by_key(|&r| order[r]);
                }
                let count = sorted.len();
                for (i, &row) in sorted.iter().enumerate() {
                    result[row] = (i * n / count + 1) as i64;
                }
            }
            Ok(Arc::new(Int64Array::from(result)))
        }
        "LAG" | "LEAD" => {
            if args.is_empty() {
                return Err(KaveonError::Execution(format!(
                    "{name} requires at least one argument"
                )));
            }
            let col = crate::expr_eval::evaluate(&args[0], batch)?;
            let offset = if args.len() > 1 {
                match &args[1] {
                    Expr::Literal(kaveon_core::predicate::ScalarValue::Int64(v)) => *v as usize,
                    _ => 1,
                }
            } else {
                1
            };
            let is_lag = name.eq_ignore_ascii_case("LAG");

            eval_lag_lead(&col, partitions, sort_order, num_rows, offset, is_lag)
        }
        "FIRST_VALUE" => {
            if args.is_empty() {
                return Err(KaveonError::Execution(
                    "FIRST_VALUE requires one argument".into(),
                ));
            }
            let col = crate::expr_eval::evaluate(&args[0], batch)?;
            eval_first_last_value(&col, partitions, sort_order, num_rows, true)
        }
        "LAST_VALUE" => {
            if args.is_empty() {
                return Err(KaveonError::Execution(
                    "LAST_VALUE requires one argument".into(),
                ));
            }
            let col = crate::expr_eval::evaluate(&args[0], batch)?;
            eval_first_last_value(&col, partitions, sort_order, num_rows, false)
        }
        "SUM" | "COUNT" | "AVG" | "MIN" | "MAX" => {
            eval_aggregate_window(name, args, batch, partitions, sort_order, frame)
        }
        _ => Err(KaveonError::Execution(format!(
            "unsupported window function: {name}"
        ))),
    }
}

fn eval_lag_lead(
    col: &ArrayRef,
    partitions: &PartitionMap,
    sort_order: Option<&[usize]>,
    num_rows: usize,
    offset: usize,
    is_lag: bool,
) -> Result<ArrayRef> {
    match col.data_type() {
        DataType::Int64 => {
            let arr = col.as_primitive::<Int64Type>();
            let mut result: Vec<Option<i64>> = vec![None; num_rows];
            for (_, rows) in partitions {
                let mut sorted = rows.clone();
                if let Some(order) = sort_order {
                    sorted.sort_by_key(|&r| order[r]);
                }
                for (i, &row) in sorted.iter().enumerate() {
                    let source_idx = if is_lag {
                        if i >= offset {
                            Some(sorted[i - offset])
                        } else {
                            None
                        }
                    } else if i + offset < sorted.len() {
                        Some(sorted[i + offset])
                    } else {
                        None
                    };
                    result[row] = source_idx.and_then(|idx| {
                        if arr.is_null(idx) {
                            None
                        } else {
                            Some(arr.value(idx))
                        }
                    });
                }
            }
            Ok(Arc::new(Int64Array::from(result)))
        }
        DataType::Float64 => {
            let arr = col.as_primitive::<Float64Type>();
            let mut result: Vec<Option<f64>> = vec![None; num_rows];
            for (_, rows) in partitions {
                let mut sorted = rows.clone();
                if let Some(order) = sort_order {
                    sorted.sort_by_key(|&r| order[r]);
                }
                for (i, &row) in sorted.iter().enumerate() {
                    let source_idx = if is_lag {
                        if i >= offset {
                            Some(sorted[i - offset])
                        } else {
                            None
                        }
                    } else if i + offset < sorted.len() {
                        Some(sorted[i + offset])
                    } else {
                        None
                    };
                    result[row] = source_idx.and_then(|idx| {
                        if arr.is_null(idx) {
                            None
                        } else {
                            Some(arr.value(idx))
                        }
                    });
                }
            }
            Ok(Arc::new(Float64Array::from(result)))
        }
        DataType::Utf8 => {
            let arr = col.as_any().downcast_ref::<StringArray>().expect("Utf8");
            let mut result: Vec<Option<String>> = vec![None; num_rows];
            for (_, rows) in partitions {
                let mut sorted = rows.clone();
                if let Some(order) = sort_order {
                    sorted.sort_by_key(|&r| order[r]);
                }
                for (i, &row) in sorted.iter().enumerate() {
                    let source_idx = if is_lag {
                        if i >= offset {
                            Some(sorted[i - offset])
                        } else {
                            None
                        }
                    } else if i + offset < sorted.len() {
                        Some(sorted[i + offset])
                    } else {
                        None
                    };
                    result[row] = source_idx.and_then(|idx| {
                        if arr.is_null(idx) {
                            None
                        } else {
                            Some(arr.value(idx).to_owned())
                        }
                    });
                }
            }
            Ok(Arc::new(StringArray::from(
                result.iter().map(|v| v.as_deref()).collect::<Vec<_>>(),
            )))
        }
        dt => Err(KaveonError::Execution(format!(
            "LAG/LEAD not supported for type {dt}"
        ))),
    }
}

fn eval_first_last_value(
    col: &ArrayRef,
    partitions: &PartitionMap,
    sort_order: Option<&[usize]>,
    num_rows: usize,
    first: bool,
) -> Result<ArrayRef> {
    let indices = arrow::array::UInt32Array::from(
        (0..num_rows)
            .map(|row| {
                for (_, rows) in partitions {
                    if rows.contains(&row) {
                        let mut sorted = rows.clone();
                        if let Some(order) = sort_order {
                            sorted.sort_by_key(|&r| order[r]);
                        }
                        return if first {
                            *sorted.first().unwrap() as u32
                        } else {
                            *sorted.last().unwrap() as u32
                        };
                    }
                }
                row as u32
            })
            .collect::<Vec<_>>(),
    );
    arrow::compute::take(col.as_ref(), &indices, None)
        .map_err(|e| KaveonError::Execution(format!("first/last value: {e}")))
}

fn eval_aggregate_window(
    name: &str,
    args: &[Expr],
    batch: &RecordBatch,
    partitions: &PartitionMap,
    sort_order: Option<&[usize]>,
    frame: Option<&kaveon_core::WindowFrame>,
) -> Result<ArrayRef> {
    let col = if args.is_empty() || matches!(&args[0], Expr::Star) {
        None
    } else {
        Some(crate::expr_eval::evaluate(&args[0], batch)?)
    };

    let num_rows = batch.num_rows();
    let has_frame = frame.is_some() || sort_order.is_some();

    if has_frame {
        return eval_framed_aggregate(name, &col, partitions, sort_order, num_rows, frame);
    }

    match name.to_uppercase().as_str() {
        "COUNT" => {
            let mut result = vec![0i64; num_rows];
            for (_, rows) in partitions {
                let count = if let Some(ref col) = col {
                    rows.iter().filter(|&&r| !col.is_null(r)).count() as i64
                } else {
                    rows.len() as i64
                };
                for &row in rows {
                    result[row] = count;
                }
            }
            Ok(Arc::new(Int64Array::from(result)))
        }
        "SUM" => {
            let col =
                col.ok_or_else(|| KaveonError::Execution("SUM requires a column argument".into()))?;
            let mut result = vec![0.0f64; num_rows];
            for (_, rows) in partitions {
                let sum = sum_values(&col, rows)?;
                for &row in rows {
                    result[row] = sum;
                }
            }
            Ok(Arc::new(Float64Array::from(result)))
        }
        "AVG" => {
            let col =
                col.ok_or_else(|| KaveonError::Execution("AVG requires a column argument".into()))?;
            let mut result: Vec<Option<f64>> = vec![None; num_rows];
            for (_, rows) in partitions {
                let (sum, count) = sum_count_values(&col, rows)?;
                let avg = if count > 0 {
                    Some(sum / count as f64)
                } else {
                    None
                };
                for &row in rows {
                    result[row] = avg;
                }
            }
            Ok(Arc::new(Float64Array::from(result)))
        }
        "MIN" => {
            let col =
                col.ok_or_else(|| KaveonError::Execution("MIN requires a column argument".into()))?;
            let mut result: Vec<Option<f64>> = vec![None; num_rows];
            for (_, rows) in partitions {
                let min = min_max_values(&col, rows, true)?;
                for &row in rows {
                    result[row] = min;
                }
            }
            Ok(Arc::new(Float64Array::from(result)))
        }
        "MAX" => {
            let col =
                col.ok_or_else(|| KaveonError::Execution("MAX requires a column argument".into()))?;
            let mut result: Vec<Option<f64>> = vec![None; num_rows];
            for (_, rows) in partitions {
                let max = min_max_values(&col, rows, false)?;
                for &row in rows {
                    result[row] = max;
                }
            }
            Ok(Arc::new(Float64Array::from(result)))
        }
        _ => Err(KaveonError::Execution(format!(
            "unsupported aggregate window: {name}"
        ))),
    }
}

fn eval_framed_aggregate(
    name: &str,
    col: &Option<ArrayRef>,
    partitions: &PartitionMap,
    sort_order: Option<&[usize]>,
    num_rows: usize,
    frame: Option<&kaveon_core::WindowFrame>,
) -> Result<ArrayRef> {
    use kaveon_core::{WindowFrameBound, WindowFrameUnits};

    let default_frame = kaveon_core::WindowFrame {
        units: WindowFrameUnits::Range,
        start: WindowFrameBound::UnboundedPreceding,
        end: WindowFrameBound::CurrentRow,
    };
    let frame = frame.unwrap_or(&default_frame);

    let upper = name.to_uppercase();

    match upper.as_str() {
        "COUNT" => {
            let mut result = vec![0i64; num_rows];
            for (_, rows) in partitions {
                let sorted = sort_partition(rows, sort_order);
                for (pos, &row) in sorted.iter().enumerate() {
                    let (start, end) = frame_bounds(frame, pos, sorted.len());
                    let count = if let Some(c) = col {
                        (start..=end).filter(|&i| !c.is_null(sorted[i])).count() as i64
                    } else {
                        (end - start + 1) as i64
                    };
                    result[row] = count;
                }
            }
            Ok(Arc::new(Int64Array::from(result)))
        }
        "SUM" => {
            let col = col
                .as_ref()
                .ok_or_else(|| KaveonError::Execution("SUM requires a column".into()))?;
            let mut result = vec![0.0f64; num_rows];
            for (_, rows) in partitions {
                let sorted = sort_partition(rows, sort_order);
                for (pos, &row) in sorted.iter().enumerate() {
                    let (start, end) = frame_bounds(frame, pos, sorted.len());
                    let window_rows: Vec<usize> = (start..=end).map(|i| sorted[i]).collect();
                    result[row] = sum_values(col, &window_rows)?;
                }
            }
            Ok(Arc::new(Float64Array::from(result)))
        }
        "AVG" => {
            let col = col
                .as_ref()
                .ok_or_else(|| KaveonError::Execution("AVG requires a column".into()))?;
            let mut result: Vec<Option<f64>> = vec![None; num_rows];
            for (_, rows) in partitions {
                let sorted = sort_partition(rows, sort_order);
                for (pos, &row) in sorted.iter().enumerate() {
                    let (start, end) = frame_bounds(frame, pos, sorted.len());
                    let window_rows: Vec<usize> = (start..=end).map(|i| sorted[i]).collect();
                    let (sum, count) = sum_count_values(col, &window_rows)?;
                    result[row] = if count > 0 {
                        Some(sum / count as f64)
                    } else {
                        None
                    };
                }
            }
            Ok(Arc::new(Float64Array::from(result)))
        }
        "MIN" => {
            let col = col
                .as_ref()
                .ok_or_else(|| KaveonError::Execution("MIN requires a column".into()))?;
            let mut result: Vec<Option<f64>> = vec![None; num_rows];
            for (_, rows) in partitions {
                let sorted = sort_partition(rows, sort_order);
                for (pos, &row) in sorted.iter().enumerate() {
                    let (start, end) = frame_bounds(frame, pos, sorted.len());
                    let window_rows: Vec<usize> = (start..=end).map(|i| sorted[i]).collect();
                    result[row] = min_max_values(col, &window_rows, true)?;
                }
            }
            Ok(Arc::new(Float64Array::from(result)))
        }
        "MAX" => {
            let col = col
                .as_ref()
                .ok_or_else(|| KaveonError::Execution("MAX requires a column".into()))?;
            let mut result: Vec<Option<f64>> = vec![None; num_rows];
            for (_, rows) in partitions {
                let sorted = sort_partition(rows, sort_order);
                for (pos, &row) in sorted.iter().enumerate() {
                    let (start, end) = frame_bounds(frame, pos, sorted.len());
                    let window_rows: Vec<usize> = (start..=end).map(|i| sorted[i]).collect();
                    result[row] = min_max_values(col, &window_rows, false)?;
                }
            }
            Ok(Arc::new(Float64Array::from(result)))
        }
        _ => Err(KaveonError::Execution(format!(
            "unsupported framed aggregate window: {name}"
        ))),
    }
}

fn sort_partition(rows: &[usize], sort_order: Option<&[usize]>) -> Vec<usize> {
    let mut sorted = rows.to_vec();
    if let Some(order) = sort_order {
        sorted.sort_by_key(|&r| order[r]);
    }
    sorted
}

fn frame_bounds(
    frame: &kaveon_core::WindowFrame,
    pos: usize,
    partition_len: usize,
) -> (usize, usize) {
    use kaveon_core::WindowFrameBound;

    let start = match frame.start {
        WindowFrameBound::UnboundedPreceding => 0,
        WindowFrameBound::Preceding(n) => pos.saturating_sub(n as usize),
        WindowFrameBound::CurrentRow => pos,
        WindowFrameBound::Following(n) => (pos + n as usize).min(partition_len - 1),
        WindowFrameBound::UnboundedFollowing => partition_len - 1,
    };
    let end = match frame.end {
        WindowFrameBound::UnboundedPreceding => 0,
        WindowFrameBound::Preceding(n) => pos.saturating_sub(n as usize),
        WindowFrameBound::CurrentRow => pos,
        WindowFrameBound::Following(n) => (pos + n as usize).min(partition_len - 1),
        WindowFrameBound::UnboundedFollowing => partition_len - 1,
    };
    (start, end)
}

fn sum_values(col: &ArrayRef, rows: &[usize]) -> Result<f64> {
    let mut sum = 0.0;
    for &row in rows {
        if col.is_null(row) {
            continue;
        }
        sum += to_f64(col, row)?;
    }
    Ok(sum)
}

fn sum_count_values(col: &ArrayRef, rows: &[usize]) -> Result<(f64, usize)> {
    let mut sum = 0.0;
    let mut count = 0;
    for &row in rows {
        if col.is_null(row) {
            continue;
        }
        sum += to_f64(col, row)?;
        count += 1;
    }
    Ok((sum, count))
}

fn min_max_values(col: &ArrayRef, rows: &[usize], is_min: bool) -> Result<Option<f64>> {
    let mut result: Option<f64> = None;
    for &row in rows {
        if col.is_null(row) {
            continue;
        }
        let v = to_f64(col, row)?;
        result = Some(match result {
            None => v,
            Some(current) => {
                if is_min {
                    current.min(v)
                } else {
                    current.max(v)
                }
            }
        });
    }
    Ok(result)
}

fn to_f64(col: &ArrayRef, row: usize) -> Result<f64> {
    match col.data_type() {
        DataType::Int32 => Ok(col.as_primitive::<arrow::datatypes::Int32Type>().value(row) as f64),
        DataType::Int64 => Ok(col.as_primitive::<Int64Type>().value(row) as f64),
        DataType::UInt64 => Ok(col
            .as_primitive::<arrow::datatypes::UInt64Type>()
            .value(row) as f64),
        DataType::Float64 => Ok(col.as_primitive::<Float64Type>().value(row)),
        DataType::Decimal128(_, scale) => {
            let arr = col
                .as_any()
                .downcast_ref::<arrow::array::Decimal128Array>()
                .expect("Decimal128");
            Ok(arr.value(row) as f64 / 10f64.powi(*scale as i32))
        }
        dt => Err(KaveonError::Execution(format!(
            "cannot convert {dt} to numeric for window aggregate"
        ))),
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
            array.as_primitive::<Int64Type>().value(row),
        )),
        DataType::UInt64 => Ok(AggregateValue::Int64(
            array
                .as_primitive::<arrow::datatypes::UInt64Type>()
                .value(row) as i64,
        )),
        DataType::Float64 => {
            let v = array.as_primitive::<Float64Type>().value(row);
            Ok(AggregateValue::Float64Bits(v.to_bits()))
        }
        DataType::Utf8 => Ok(AggregateValue::Utf8(
            array
                .as_any()
                .downcast_ref::<StringArray>()
                .expect("Utf8")
                .value(row)
                .to_owned(),
        )),
        dt => Err(KaveonError::Execution(format!(
            "unsupported type for partitioning: {dt}"
        ))),
    }
}

fn compare_array_values(array: &dyn Array, a: usize, b: usize) -> std::cmp::Ordering {
    if array.is_null(a) && array.is_null(b) {
        return std::cmp::Ordering::Equal;
    }
    if array.is_null(a) {
        return std::cmp::Ordering::Greater;
    }
    if array.is_null(b) {
        return std::cmp::Ordering::Less;
    }
    match array.data_type() {
        DataType::Int32 => {
            let arr = array.as_primitive::<arrow::datatypes::Int32Type>();
            arr.value(a).cmp(&arr.value(b))
        }
        DataType::Int64 => {
            let arr = array.as_primitive::<Int64Type>();
            arr.value(a).cmp(&arr.value(b))
        }
        DataType::UInt64 => {
            let arr = array.as_primitive::<arrow::datatypes::UInt64Type>();
            arr.value(a).cmp(&arr.value(b))
        }
        DataType::Float64 => {
            let arr = array.as_primitive::<Float64Type>();
            arr.value(a).total_cmp(&arr.value(b))
        }
        DataType::Utf8 => {
            let arr = array.as_any().downcast_ref::<StringArray>().expect("Utf8");
            arr.value(a).cmp(arr.value(b))
        }
        _ => std::cmp::Ordering::Equal,
    }
}
