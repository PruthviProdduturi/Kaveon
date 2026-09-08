use std::{cmp::Ordering, sync::Arc};

use arrow::{
    array::{Array, ArrayRef, AsArray, Float64Array, Int64Array, RecordBatch, UInt32Array},
    compute::{SortColumn, SortOptions, concat_batches, lexsort_to_indices, take},
    datatypes::{
        DataType, Field, Float64Type, Int32Type, Int64Type, Schema, SchemaRef, UInt64Type,
    },
};
use kaveon_core::{
    BatchOperator, Expr, KaveonError, Result, WindowFrame, WindowFrameBound, WindowFrameUnits,
};

fn error(message: impl Into<String>) -> KaveonError {
    KaveonError::Execution(message.into())
}

/// Full expression identity keeps different frames/partitions from sharing a result column.
pub(crate) fn window_output_name(expr: &Expr) -> String {
    format!("__kaveon_window_{expr:?}")
}

pub struct WindowOperator {
    source: Box<dyn BatchOperator>,
    expressions: Vec<Expr>,
    schema: SchemaRef,
    emitted: bool,
    memory: Option<kaveon_core::OperatorMemoryAccount>,
}

impl WindowOperator {
    pub fn new(source: Box<dyn BatchOperator>, expressions: Vec<Expr>) -> Result<Self> {
        let empty = RecordBatch::new_empty(source.schema().clone());
        let mut fields = source
            .schema()
            .fields()
            .iter()
            .map(|f| f.as_ref().clone())
            .collect::<Vec<_>>();
        for expr in &expressions {
            let output = evaluate_window(expr, &empty)?;
            fields.push(Field::new(
                window_output_name(expr),
                output.data_type().clone(),
                true,
            ));
        }
        Ok(Self {
            source,
            expressions,
            schema: Arc::new(Schema::new(fields)),
            emitted: false,
            memory: None,
        })
    }

    pub fn with_memory(mut self, memory: kaveon_core::OperatorMemoryAccount) -> Self {
        self.memory = Some(memory);
        self
    }
}

impl BatchOperator for WindowOperator {
    fn schema(&self) -> &SchemaRef {
        &self.schema
    }
    fn next_batch(&mut self) -> Result<Option<RecordBatch>> {
        let expression_memory = self.memory.clone();
        crate::expr_eval::with_expression_memory(expression_memory.as_ref(), || {
            if self.emitted {
                return Ok(None);
            }
            self.emitted = true;
            let mut batches = Vec::new();
            let mut reservations = Vec::new();
            let mut total_bytes = 0_u64;
            let mut total_rows = 0_u64;
            while let Some(batch) = self.source.next_batch()? {
                total_bytes = total_bytes.saturating_add(batch.get_array_memory_size() as u64);
                total_rows = total_rows.saturating_add(batch.num_rows() as u64);
                if let Some(memory) = &self.memory {
                    reservations.push(memory.reserve(batch.get_array_memory_size() as u64)?);
                }
                batches.push(batch);
            }
            self.emitted = true;
            if batches.is_empty() {
                return Ok(None);
            }
            // Keep admission across concatenation, sorting/permutation, frame
            // indices and aggregate temporaries, and all output columns. Frame work
            // remains quadratic in CPU for broad frames but is not retained per row.
            let output_bytes = self
                .schema
                .fields()
                .iter()
                .skip(self.source.schema().fields().len())
                .fold(0_u64, |bytes, field| {
                    bytes.saturating_add(match field.data_type() {
                        DataType::Utf8
                        | DataType::LargeUtf8
                        | DataType::Binary
                        | DataType::LargeBinary => {
                            total_bytes.saturating_add(16).saturating_mul(total_rows)
                        }
                        _ => total_rows.saturating_mul(32),
                    })
                });
            let workspace = total_bytes
                .saturating_mul(8)
                .saturating_add(total_rows.saturating_mul(2048))
                .saturating_add(output_bytes);
            let _workspace = self
                .memory
                .as_ref()
                .map(|memory| memory.reserve(workspace))
                .transpose()?;
            let batch = concat_batches(self.source.schema(), &batches)?;
            drop(batches);
            let mut columns = batch.columns().to_vec();
            for expr in &self.expressions {
                columns.push(evaluate_window(expr, &batch)?);
            }
            Ok(Some(RecordBatch::try_new(self.schema.clone(), columns)?))
        })
    }
}

fn columns(exprs: &[Expr], batch: &RecordBatch) -> Result<Vec<ArrayRef>> {
    exprs
        .iter()
        .map(|e| crate::expr_eval::evaluate(e, batch))
        .collect()
}

fn compare(col: &ArrayRef, left: usize, right: usize) -> Result<Ordering> {
    // Arrow comparators preserve integer/decimal precision and SQL peer equality.
    if col.is_null(left) || col.is_null(right) {
        return Ok(match (col.is_null(left), col.is_null(right)) {
            (true, true) => Ordering::Equal,
            (true, false) => Ordering::Greater,
            _ => Ordering::Less,
        });
    }
    let comparator = arrow::array::make_comparator(
        col.as_ref(),
        col.as_ref(),
        SortOptions {
            descending: false,
            nulls_first: false,
        },
    )?;
    Ok(comparator(left, right))
}

fn peers(cols: &[ArrayRef], left: usize, right: usize) -> Result<bool> {
    for col in cols {
        if compare(col, left, right)? != Ordering::Equal {
            return Ok(false);
        }
    }
    Ok(true)
}

fn evaluate_window(expr: &Expr, batch: &RecordBatch) -> Result<ArrayRef> {
    let Expr::WindowFunction {
        name,
        args,
        partition_by,
        order_by,
        frame,
    } = expr
    else {
        return Err(error("window operator requires window expressions"));
    };
    let name = name.to_uppercase();
    let partition_cols = columns(partition_by, batch)?;
    let order_cols = columns(
        &order_by.iter().map(|(e, _)| e.clone()).collect::<Vec<_>>(),
        batch,
    )?;
    let mut sorts = partition_cols
        .iter()
        .map(|c| SortColumn {
            values: c.clone(),
            options: Some(SortOptions {
                descending: false,
                nulls_first: false,
            }),
        })
        .collect::<Vec<_>>();
    sorts.extend(
        order_cols
            .iter()
            .zip(order_by)
            .map(|(c, (_, asc))| SortColumn {
                values: c.clone(),
                options: Some(SortOptions {
                    descending: !asc,
                    nulls_first: false,
                }),
            }),
    );
    let sorted = if sorts.is_empty() {
        (0..batch.num_rows()).collect::<Vec<_>>()
    } else {
        lexsort_to_indices(&sorts, None)?
            .values()
            .iter()
            .map(|&n| n as usize)
            .collect()
    };
    let mut partitions: Vec<Vec<usize>> = Vec::new();
    for (index, row) in sorted.into_iter().enumerate() {
        if index % 1024 == 0 {
            crate::expr_eval::check_expression_cancelled()?;
        }
        if let Some(last) = partitions.last_mut()
            && peers(&partition_cols, last[0], row)?
        {
            last.push(row);
        } else {
            partitions.push(vec![row]);
        }
    }
    let default_frame = WindowFrame {
        units: WindowFrameUnits::Range,
        start: WindowFrameBound::UnboundedPreceding,
        end: WindowFrameBound::CurrentRow,
    };
    let frame = frame.as_ref().unwrap_or(&default_frame);
    validate_frame(frame, &order_cols, order_by.len())?;
    let col = args
        .first()
        .filter(|e| !matches!(e, Expr::Star))
        .map(|e| crate::expr_eval::evaluate(e, batch))
        .transpose()?;
    let mut indices = vec![None; batch.num_rows()];
    let mut ints = vec![0i64; batch.num_rows()];
    let mut aggregate_results: Vec<(usize, ArrayRef)> = Vec::new();
    let offset = if name == "LAG" || name == "LEAD" {
        literal_offset(args.get(1), 1, false)?
    } else {
        0
    };
    let tiles = if name == "NTILE" {
        literal_offset(args.first(), 0, true)?
    } else {
        1
    };
    if matches!(
        name.as_str(),
        "LAG" | "LEAD" | "FIRST_VALUE" | "LAST_VALUE" | "SUM" | "AVG" | "MIN" | "MAX"
    ) && col.is_none()
    {
        return Err(error(format!("{name} requires a value argument")));
    }
    for rows in partitions {
        let mut starts = vec![0];
        let mut group = vec![0; rows.len()];
        for i in 1..rows.len() {
            if !peers(&order_cols, rows[i - 1], rows[i])? {
                starts.push(i);
            }
            group[i] = starts.len() - 1;
        }
        starts.push(rows.len());
        for (pos, &row) in rows.iter().enumerate() {
            if pos % 64 == 0 {
                crate::expr_eval::check_expression_cancelled()?;
            }
            match name.as_str() {
                "ROW_NUMBER" => ints[row] = pos as i64 + 1,
                "RANK" => ints[row] = starts[group[pos]] as i64 + 1,
                "DENSE_RANK" => ints[row] = group[pos] as i64 + 1,
                "NTILE" => {
                    let large = rows.len() % tiles;
                    let size = rows.len() / tiles;
                    let boundary = (size + 1) * large;
                    ints[row] = if pos < boundary {
                        pos / (size + 1) + 1
                    } else {
                        large + (pos - boundary) / size + 1
                    } as i64;
                }
                "LAG" | "LEAD" => {
                    let source = if name == "LAG" {
                        pos.checked_sub(offset)
                    } else {
                        pos.checked_add(offset).filter(|&i| i < rows.len())
                    };
                    indices[row] = source.map(|i| rows[i] as u32);
                }
                "COUNT" | "SUM" | "AVG" | "MIN" | "MAX" | "FIRST_VALUE" | "LAST_VALUE" => {
                    let (start, end) =
                        frame_bounds(frame, pos, &rows, &starts, &group, &order_cols, order_by)?;
                    let selected = &rows[start..end];
                    match name.as_str() {
                        "COUNT" => {
                            ints[row] = selected
                                .iter()
                                .filter(|&&i| col.as_ref().is_none_or(|c| !c.is_null(i)))
                                .count() as i64
                        }
                        "FIRST_VALUE" => indices[row] = selected.first().map(|&i| i as u32),
                        "LAST_VALUE" => indices[row] = selected.last().map(|&i| i as u32),
                        _ => aggregate_results
                            .push((row, aggregate(&name, col.as_ref().unwrap(), selected)?)),
                    }
                }
                _ => return Err(error(format!("unsupported window function: {name}"))),
            }
        }
    }
    match name.as_str() {
        "ROW_NUMBER" | "RANK" | "DENSE_RANK" | "NTILE" | "COUNT" => {
            Ok(Arc::new(Int64Array::from(ints)))
        }
        "LAG" | "LEAD" | "FIRST_VALUE" | "LAST_VALUE" => {
            let col = col.as_ref().unwrap();
            let output = take(col.as_ref(), &UInt32Array::from(indices.clone()), None)?;
            if matches!(name.as_str(), "LAG" | "LEAD")
                && let Some(default) = args.get(2)
            {
                let defaults = crate::expr_eval::evaluate(default, batch)?;
                let defaults = arrow::compute::cast(&defaults, col.data_type())?;
                let missing = arrow::array::BooleanArray::from(
                    indices.iter().map(Option::is_none).collect::<Vec<_>>(),
                );
                return Ok(arrow::compute::kernels::zip::zip(
                    &missing, &defaults, &output,
                )?);
            }
            Ok(output)
        }
        "SUM" | "AVG" | "MIN" | "MAX" => {
            if aggregate_results.is_empty() {
                return Ok(arrow::array::new_empty_array(
                    aggregate(&name, col.as_ref().unwrap(), &[])?.data_type(),
                ));
            }
            aggregate_results.sort_by_key(|(row, _)| *row);
            let arrays = aggregate_results
                .iter()
                .map(|(_, a)| a.as_ref())
                .collect::<Vec<_>>();
            Ok(arrow::compute::concat(&arrays)?)
        }
        _ => Err(error(format!("unsupported window function: {name}"))),
    }
}

fn literal_offset(expr: Option<&Expr>, default: usize, positive: bool) -> Result<usize> {
    let value = match expr {
        None => default,
        Some(Expr::Literal(kaveon_core::predicate::ScalarValue::Int64(n))) if *n >= 0 => {
            usize::try_from(*n).map_err(|_| error("window offset too large"))?
        }
        _ => return Err(error("window offset must be a nonnegative integer literal")),
    };
    if positive && value == 0 {
        return Err(error("NTILE requires a positive integer"));
    }
    Ok(value)
}

fn validate_frame(frame: &WindowFrame, cols: &[ArrayRef], order_count: usize) -> Result<()> {
    use WindowFrameBound::*;
    if matches!(frame.start, UnboundedFollowing) || matches!(frame.end, UnboundedPreceding) {
        return Err(error("invalid unbounded window frame"));
    }
    let position = |b| match b {
        UnboundedPreceding => i128::MIN,
        Preceding(n) => -(n as i128),
        CurrentRow => 0,
        Following(n) => n as i128,
        UnboundedFollowing => i128::MAX,
    };
    if position(frame.start) > position(frame.end) {
        return Err(error("window frame start follows its end"));
    }
    if matches!(frame.units, WindowFrameUnits::Groups) && order_count == 0 {
        return Err(error("GROUPS frame requires ORDER BY"));
    }
    if matches!(frame.units, WindowFrameUnits::Range)
        && matches!(frame.start, Preceding(_) | Following(_))
        || matches!(frame.units, WindowFrameUnits::Range)
            && matches!(frame.end, Preceding(_) | Following(_))
    {
        if order_count != 1 {
            return Err(error(
                "RANGE with offsets requires exactly one ORDER BY expression",
            ));
        }
        if !matches!(
            cols[0].data_type(),
            DataType::Int32
                | DataType::Int64
                | DataType::UInt64
                | DataType::Float64
                | DataType::Decimal128(_, _)
        ) {
            return Err(error(
                "RANGE offsets support integer, decimal, and Float64 ordering",
            ));
        }
    }
    Ok(())
}

fn frame_bounds(
    frame: &WindowFrame,
    pos: usize,
    rows: &[usize],
    starts: &[usize],
    groups: &[usize],
    cols: &[ArrayRef],
    order: &[(Expr, bool)],
) -> Result<(usize, usize)> {
    use WindowFrameBound::*;
    let bound = |b: WindowFrameBound, end: bool| -> Result<usize> {
        if matches!(b, UnboundedPreceding) {
            return Ok(0);
        }
        if matches!(b, UnboundedFollowing) {
            return Ok(rows.len());
        }
        if matches!(
            frame.units,
            WindowFrameUnits::Rows | WindowFrameUnits::Groups
        ) {
            let base = if matches!(frame.units, WindowFrameUnits::Rows) {
                pos
            } else {
                groups[pos]
            } as i128;
            let target =
                base + match b {
                    Preceding(n) => -(n as i128),
                    Following(n) => n as i128,
                    _ => 0,
                } + i128::from(end);
            let count = if matches!(frame.units, WindowFrameUnits::Rows) {
                rows.len()
            } else {
                starts.len() - 1
            };
            let index = target.clamp(0, count as i128) as usize;
            return Ok(if matches!(frame.units, WindowFrameUnits::Rows) {
                index
            } else {
                starts[index]
            });
        }
        if matches!(b, CurrentRow) || cols.first().is_some_and(|c| c.is_null(rows[pos])) {
            return Ok(starts[groups[pos] + usize::from(end)]);
        }
        let amount = match b {
            Preceding(n) => -(n as i128),
            Following(n) => n as i128,
            _ => unreachable!(),
        };
        let ascending = order[0].1;
        for (i, &row) in rows.iter().enumerate() {
            // NULLS LAST independently of direction; a finite boundary never includes NULL.
            if cols[0].is_null(row) {
                return Ok(i);
            }
            let mut cmp = compare_offset(
                &cols[0],
                row,
                rows[pos],
                if ascending { amount } else { -amount },
            )?;
            if !ascending {
                cmp = cmp.reverse();
            }
            if cmp == Ordering::Greater || (!end && cmp == Ordering::Equal) {
                return Ok(i);
            }
        }
        Ok(rows.len())
    };
    let start = bound(frame.start, false)?;
    let end = bound(frame.end, true)?;
    Ok((start.min(end), end))
}

fn compare_offset(col: &ArrayRef, row: usize, current: usize, offset: i128) -> Result<Ordering> {
    let pair = match col.data_type() {
        DataType::Int32 => (
            col.as_primitive::<Int32Type>().value(row) as i128,
            col.as_primitive::<Int32Type>().value(current) as i128,
            offset,
        ),
        DataType::Int64 => (
            col.as_primitive::<Int64Type>().value(row) as i128,
            col.as_primitive::<Int64Type>().value(current) as i128,
            offset,
        ),
        DataType::UInt64 => (
            col.as_primitive::<UInt64Type>().value(row) as i128,
            col.as_primitive::<UInt64Type>().value(current) as i128,
            offset,
        ),
        DataType::Decimal128(_, scale) => {
            if *scale < 0 {
                return Err(error(
                    "RANGE offsets with negative decimal scale are unsupported",
                ));
            }
            let factor = 10i128
                .checked_pow(*scale as u32)
                .ok_or_else(|| error("decimal RANGE offset overflow"))?;
            let col = col.as_primitive::<arrow::datatypes::Decimal128Type>();
            (
                col.value(row),
                col.value(current),
                offset
                    .checked_mul(factor)
                    .ok_or_else(|| error("decimal RANGE offset overflow"))?,
            )
        }
        DataType::Float64 => {
            let col = col.as_primitive::<Float64Type>();
            let left = col.value(row);
            let right = col.value(current) + offset as f64;
            return Ok(if left.is_nan() {
                if right.is_nan() {
                    Ordering::Equal
                } else {
                    Ordering::Greater
                }
            } else if right.is_nan() {
                Ordering::Less
            } else {
                left.partial_cmp(&right).unwrap()
            });
        }
        _ => return Err(error("unsupported RANGE offset type")),
    };
    Ok(match pair.1.checked_add(pair.2) {
        Some(target) => pair.0.cmp(&target),
        None if pair.2 > 0 => Ordering::Less,
        None => Ordering::Greater,
    })
}

fn aggregate(name: &str, col: &ArrayRef, rows: &[usize]) -> Result<ArrayRef> {
    let rows = rows
        .iter()
        .copied()
        .filter(|&r| !col.is_null(r))
        .collect::<Vec<_>>();
    if name == "MIN" || name == "MAX" {
        let mut selected = rows.first().copied();
        for &row in &rows {
            if let Some(best) = selected
                && compare(col, row, best)?
                    == if name == "MIN" {
                        Ordering::Less
                    } else {
                        Ordering::Greater
                    }
            {
                selected = Some(row);
            }
        }
        return Ok(take(
            col.as_ref(),
            &UInt32Array::from(vec![selected.map(|r| r as u32)]),
            None,
        )?);
    }
    if name == "SUM" && matches!(col.data_type(), DataType::Int32 | DataType::Int64) {
        let mut sum = 0i128;
        for &row in &rows {
            sum += if matches!(col.data_type(), DataType::Int32) {
                col.as_primitive::<Int32Type>().value(row) as i128
            } else {
                col.as_primitive::<Int64Type>().value(row) as i128
            };
        }
        let sum = i64::try_from(sum).map_err(|_| error("window SUM integer overflow"))?;
        return Ok(Arc::new(Int64Array::from(vec![
            (!rows.is_empty()).then_some(sum),
        ])));
    }
    if name == "SUM"
        && let DataType::Decimal128(_, scale) = col.data_type()
    {
        let values = col.as_primitive::<arrow::datatypes::Decimal128Type>();
        let mut sum = 0i128;
        for &row in &rows {
            sum = sum
                .checked_add(values.value(row))
                .ok_or_else(|| error("window decimal SUM overflow"))?;
        }
        let array = arrow::array::Decimal128Array::from(vec![(!rows.is_empty()).then_some(sum)])
            .with_precision_and_scale(38, *scale)?;
        array.validate_decimal_precision(38)?;
        return Ok(Arc::new(array));
    }
    if !matches!(
        col.data_type(),
        DataType::Int32 | DataType::Int64 | DataType::UInt64 | DataType::Float64
    ) {
        return Err(error(format!(
            "{name} window does not support {}",
            col.data_type()
        )));
    }
    if name == "SUM" && matches!(col.data_type(), DataType::UInt64) {
        return Err(error("UInt64 window SUM requires an explicit cast"));
    }
    let cast = arrow::compute::cast(col, &DataType::Float64)?;
    let values = cast.as_primitive::<Float64Type>();
    let sum: f64 = rows.iter().map(|&r| values.value(r)).sum();
    Ok(Arc::new(Float64Array::from(vec![
        (!rows.is_empty()).then_some(if name == "AVG" {
            sum / rows.len() as f64
        } else {
            sum
        }),
    ])))
}

#[cfg(test)]
mod tests {
    use super::*;
    use kaveon_core::predicate::ScalarValue;
    fn batch(values: Vec<Option<i64>>) -> RecordBatch {
        RecordBatch::try_new(
            Arc::new(Schema::new(vec![Field::new("x", DataType::Int64, true)])),
            vec![Arc::new(Int64Array::from(values))],
        )
        .unwrap()
    }
    fn expression(name: &str, frame: Option<WindowFrame>) -> Expr {
        Expr::WindowFunction {
            name: name.into(),
            args: vec![if name == "COUNT" {
                Expr::Star
            } else {
                Expr::Column("x".into())
            }],
            partition_by: vec![],
            order_by: vec![(Expr::Column("x".into()), true)],
            frame,
        }
    }
    fn ints(expr: Expr, input: &RecordBatch) -> Vec<Option<i64>> {
        evaluate_window(&expr, input)
            .unwrap()
            .as_primitive::<Int64Type>()
            .iter()
            .collect()
    }
    fn frame(
        units: WindowFrameUnits,
        start: WindowFrameBound,
        end: WindowFrameBound,
    ) -> Option<WindowFrame> {
        Some(WindowFrame { units, start, end })
    }
    #[test]
    fn range_default_and_groups_preserve_null_and_duplicate_peers() {
        use WindowFrameBound::*;
        use WindowFrameUnits::*;
        let input = batch(vec![Some(1), Some(1), Some(2), Some(4), None]);
        assert_eq!(
            ints(expression("COUNT", None), &input),
            vec![Some(2), Some(2), Some(3), Some(4), Some(5)]
        );
        assert_eq!(
            ints(
                expression("COUNT", frame(Groups, Preceding(1), CurrentRow)),
                &input
            ),
            vec![Some(2), Some(2), Some(3), Some(2), Some(2)]
        );
        assert_eq!(
            ints(
                expression("COUNT", frame(Range, Preceding(1), CurrentRow)),
                &input
            ),
            vec![Some(2), Some(2), Some(3), Some(1), Some(1)]
        );
        assert_eq!(
            ints(
                expression("COUNT", frame(Range, CurrentRow, CurrentRow)),
                &input
            ),
            vec![Some(2), Some(2), Some(1), Some(1), Some(1)]
        );
    }
    #[test]
    fn empty_frames_and_all_null_sum_follow_sql() {
        use WindowFrameBound::*;
        use WindowFrameUnits::*;
        let input = batch(vec![Some(1), Some(2), None]);
        let following = frame(Rows, Following(1), Following(1));
        assert_eq!(
            ints(expression("COUNT", following), &input),
            vec![Some(1), Some(1), Some(0)]
        );
        assert_eq!(
            ints(expression("SUM", following), &input),
            vec![Some(2), None, None]
        );
        let preceding = frame(Rows, Preceding(2), Preceding(1));
        assert_eq!(
            ints(expression("COUNT", preceding), &input),
            vec![Some(0), Some(1), Some(2)]
        );
        assert_eq!(
            ints(
                expression(
                    "COUNT",
                    frame(Rows, Following(u64::MAX), Following(u64::MAX))
                ),
                &input
            ),
            vec![Some(0); 3]
        );
    }
    #[test]
    fn ranks_tiles_and_value_functions() {
        use WindowFrameBound::*;
        use WindowFrameUnits::*;
        let input = batch(vec![Some(1), Some(1), Some(2), Some(3), Some(4), Some(5)]);
        assert_eq!(
            ints(expression("RANK", None), &input),
            vec![Some(1), Some(1), Some(3), Some(4), Some(5), Some(6)]
        );
        assert_eq!(
            ints(expression("DENSE_RANK", None), &input),
            vec![Some(1), Some(1), Some(2), Some(3), Some(4), Some(5)]
        );
        let mut tile = expression("NTILE", None);
        if let Expr::WindowFunction { args, .. } = &mut tile {
            *args = vec![Expr::Literal(ScalarValue::Int64(4))];
        }
        assert_eq!(
            ints(tile, &input),
            vec![Some(1), Some(1), Some(2), Some(2), Some(3), Some(4)]
        );
        assert_eq!(
            ints(
                expression("LAST_VALUE", frame(Rows, CurrentRow, CurrentRow)),
                &input
            ),
            vec![Some(1), Some(1), Some(2), Some(3), Some(4), Some(5)]
        );
        let mut lag = expression("LAG", None);
        if let Expr::WindowFunction { args, .. } = &mut lag {
            args.extend([
                Expr::Literal(ScalarValue::Int64(1)),
                Expr::Literal(ScalarValue::Int64(99)),
            ]);
        }
        assert_eq!(
            ints(lag, &input),
            vec![Some(99), Some(1), Some(1), Some(2), Some(3), Some(4)]
        );
    }
    #[test]
    fn range_descending_and_large_integer_precision() {
        use WindowFrameBound::*;
        use WindowFrameUnits::*;
        let input = batch(vec![
            Some(9007199254740992),
            Some(9007199254740993),
            Some(9007199254740994),
            None,
        ]);
        let mut expr = expression("COUNT", frame(Range, CurrentRow, Following(1)));
        if let Expr::WindowFunction { order_by, .. } = &mut expr {
            order_by[0].1 = false;
        }
        assert_eq!(ints(expr, &input), vec![Some(1), Some(2), Some(2), Some(1)]);
        assert_eq!(
            ints(expression("MIN", None), &input),
            vec![Some(9007199254740992); 4]
        );
    }
    #[test]
    fn invalid_frames_and_offsets_fail_even_on_empty_input() {
        use WindowFrameBound::*;
        use WindowFrameUnits::*;
        let input = batch(vec![]);
        assert!(
            evaluate_window(
                &expression("COUNT", frame(Rows, Following(1), CurrentRow)),
                &input
            )
            .is_err()
        );
        let mut lag = expression("LAG", None);
        if let Expr::WindowFunction { args, .. } = &mut lag {
            args.push(Expr::Literal(ScalarValue::Int64(-1)));
        }
        assert!(evaluate_window(&lag, &input).is_err());
    }
    struct Source {
        schema: SchemaRef,
        batch: Option<RecordBatch>,
    }
    impl BatchOperator for Source {
        fn schema(&self) -> &SchemaRef {
            &self.schema
        }
        fn next_batch(&mut self) -> Result<Option<RecordBatch>> {
            Ok(self.batch.take())
        }
    }
    #[test]
    fn projection_binds_distinct_frames_before_execution() {
        use WindowFrameBound::*;
        use WindowFrameUnits::*;
        let input = batch(vec![Some(1), Some(1), Some(2)]);
        let first = expression("COUNT", None);
        let second = expression("COUNT", frame(Rows, CurrentRow, CurrentRow));
        let source = Box::new(Source {
            schema: input.schema(),
            batch: Some(input),
        });
        let window = WindowOperator::new(source, vec![first.clone(), second.clone()]).unwrap();
        let mut project =
            crate::project::ProjectOperator::new(Box::new(window), vec![first, second]).unwrap();
        assert_eq!(project.schema().fields().len(), 2);
        let result = project.next_batch().unwrap().unwrap();
        assert_eq!(
            result
                .column(0)
                .as_primitive::<Int64Type>()
                .values()
                .as_ref(),
            &[2, 2, 3]
        );
        assert_eq!(
            result
                .column(1)
                .as_primitive::<Int64Type>()
                .values()
                .as_ref(),
            &[1, 1, 1]
        );
    }
}
