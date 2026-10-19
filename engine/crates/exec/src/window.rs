use std::{
    cmp::Ordering,
    collections::{HashMap, HashSet},
    sync::Arc,
};

use arrow::{
    array::{Array, ArrayRef, AsArray, Float64Array, Int64Array, RecordBatch, UInt32Array},
    compute::{SortColumn, SortOptions, cast, concat_batches, lexsort_to_indices, take},
    datatypes::{
        DataType, Field, Float32Type, Float64Type, Int8Type, Int16Type, Int32Type, Int64Type,
        Schema, SchemaRef, TimeUnit, UInt64Type,
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

/// Immutable sort inputs shared by window expressions with the same
/// partition/order layout.  Keeping the evaluated key arrays alongside the
/// permutation avoids reevaluating those expressions once per function.
struct WindowSortContext {
    sorted: Arc<Vec<usize>>,
    partition_cols: Vec<ArrayRef>,
    order_cols: Vec<ArrayRef>,
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
            // Multiple window expressions commonly share PARTITION BY and
            // ORDER BY while differing only in frame or function. Reuse the
            // expensive lexsort permutation for that exact layout; each
            // expression still evaluates its own frame and output values.
            let mut sorted_cache = HashMap::<String, Arc<WindowSortContext>>::new();
            let sorted_expressions = self
                .expressions
                .iter()
                .map(|expr| {
                    let sort_key = window_sort_key(expr);
                    let context = if let Some(context) = sorted_cache.get(&sort_key) {
                        Arc::clone(context)
                    } else {
                        let context = Arc::new(window_sort_context(expr, &batch)?);
                        sorted_cache.insert(sort_key, Arc::clone(&context));
                        context
                    };
                    Ok((expr, context))
                })
                .collect::<Result<Vec<_>>>()?;
            let evaluated = if sorted_expressions.len() > 1 {
                // A global/unpartitioned window still needs one shared sort, but
                // independent expressions can evaluate their frames concurrently.
                // Keep the result vector in declaration order so output schemas and
                // deterministic serialization remain unchanged.
                std::thread::scope(|scope| {
                    sorted_expressions
                        .iter()
                        .map(|(expr, context)| {
                            scope.spawn(|| {
                                crate::expr_eval::with_expression_memory(
                                    expression_memory.as_ref(),
                                    || evaluate_window_with_context(expr, &batch, context),
                                )
                            })
                        })
                        .collect::<Vec<_>>()
                        .into_iter()
                        .map(|worker| {
                            worker
                                .join()
                                .map_err(|_| error("window expression worker panicked"))?
                        })
                        .collect::<Result<Vec<_>>>()
                })?
            } else {
                sorted_expressions
                    .iter()
                    .map(|(expr, context)| evaluate_window_with_context(expr, &batch, context))
                    .collect::<Result<Vec<_>>>()?
            };
            for output in evaluated {
                columns.push(output);
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

fn window_sort_context(expr: &Expr, batch: &RecordBatch) -> Result<WindowSortContext> {
    let Expr::WindowFunction {
        partition_by,
        order_by,
        ..
    } = expr
    else {
        return Err(error("window operator requires window expressions"));
    };
    let partition_cols = normalize_dictionary_columns(columns(partition_by, batch)?)?;
    let order_cols = normalize_dictionary_columns(columns(
        &order_by
            .iter()
            .map(|(e, _, _)| e.clone())
            .collect::<Vec<_>>(),
        batch,
    )?)?;
    let sorted = Arc::new(window_sort_indices_from_columns(
        &partition_cols,
        &order_cols,
        order_by,
        batch.num_rows(),
    )?);
    Ok(WindowSortContext {
        sorted,
        partition_cols,
        order_cols,
    })
}

fn normalize_dictionary_columns(columns: Vec<ArrayRef>) -> Result<Vec<ArrayRef>> {
    columns
        .into_iter()
        .map(|column| match column.data_type() {
            // Dictionary codes are local to each batch/file.  Window peer
            // detection must compare their logical values, and normalization
            // happens once per evaluated column rather than once per row.
            DataType::Dictionary(_, value_type) => cast(column.as_ref(), value_type.as_ref())
                .map_err(|failure| error(failure.to_string())),
            _ => Ok(column),
        })
        .collect()
}

fn peers(cols: &[ArrayRef], left: usize, right: usize) -> Result<bool> {
    for col in cols {
        if compare(col, left, right)? != Ordering::Equal {
            return Ok(false);
        }
    }
    Ok(true)
}

fn peers_with_comparators<F>(comparators: &[F], left: usize, right: usize) -> bool
where
    F: Fn(usize, usize) -> Ordering,
{
    comparators
        .iter()
        .all(|comparator| comparator(left, right) == Ordering::Equal)
}

fn window_sort_key(expr: &Expr) -> String {
    let Expr::WindowFunction {
        partition_by,
        order_by,
        ..
    } = expr
    else {
        return format!("{expr:?}");
    };
    format!("{partition_by:?}|{order_by:?}")
}

fn window_sort_indices_from_columns(
    partition_cols: &[ArrayRef],
    order_cols: &[ArrayRef],
    order_by: &[(Expr, bool, bool)],
    row_count: usize,
) -> Result<Vec<usize>> {
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
            .map(|(c, (_, asc, nulls_first))| SortColumn {
                values: c.clone(),
                options: Some(SortOptions {
                    descending: !asc,
                    nulls_first: *nulls_first,
                }),
            }),
    );
    if sorts.is_empty() {
        Ok((0..row_count).collect())
    } else {
        Ok(lexsort_to_indices(&sorts, None)?
            .values()
            .iter()
            .map(|&n| n as usize)
            .collect())
    }
}

fn evaluate_window(expr: &Expr, batch: &RecordBatch) -> Result<ArrayRef> {
    evaluate_window_with_sorted(expr, batch, None)
}

fn evaluate_window_with_sorted(
    expr: &Expr,
    batch: &RecordBatch,
    sorted_override: Option<&[usize]>,
) -> Result<ArrayRef> {
    let context = if let Some(sorted) = sorted_override {
        let Expr::WindowFunction {
            partition_by,
            order_by,
            ..
        } = expr
        else {
            return Err(error("window operator requires window expressions"));
        };
        WindowSortContext {
            sorted: Arc::new(sorted.to_vec()),
            partition_cols: normalize_dictionary_columns(columns(partition_by, batch)?)?,
            order_cols: normalize_dictionary_columns(columns(
                &order_by
                    .iter()
                    .map(|(e, _, _)| e.clone())
                    .collect::<Vec<_>>(),
                batch,
            )?)?,
        }
    } else {
        window_sort_context(expr, batch)?
    };
    evaluate_window_with_context(expr, batch, &context)
}

fn evaluate_window_with_context(
    expr: &Expr,
    batch: &RecordBatch,
    context: &WindowSortContext,
) -> Result<ArrayRef> {
    let Expr::WindowFunction {
        name,
        args,
        order_by,
        frame,
        ..
    } = expr
    else {
        return Err(error("window operator requires window expressions"));
    };
    let name = name.to_uppercase();
    let ignore_nulls = name.ends_with("_IGNORE_NULLS");
    let name = name
        .strip_suffix("_IGNORE_NULLS")
        .unwrap_or(&name)
        .to_owned();
    let partition_cols = &context.partition_cols;
    let partition_comparators = partition_cols
        .iter()
        .map(|column| {
            arrow::array::make_comparator(
                column.as_ref(),
                column.as_ref(),
                SortOptions {
                    descending: false,
                    nulls_first: false,
                },
            )
            .map_err(|failure| error(failure.to_string()))
        })
        .collect::<Result<Vec<_>>>()?;
    let order_cols = &context.order_cols;
    let sorted = context.sorted.as_ref().clone();
    let mut partitions: Vec<Vec<usize>> = Vec::new();
    for (index, row) in sorted.into_iter().enumerate() {
        if index % 1024 == 0 {
            crate::expr_eval::check_expression_cancelled()?;
        }
        if let Some(last) = partitions.last_mut()
            && peers_with_comparators(&partition_comparators, last[0], row)
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
    validate_frame(frame, order_cols, order_by.len())?;
    let col = args
        .first()
        .filter(|e| !matches!(e, Expr::Star))
        .map(|e| crate::expr_eval::evaluate(e, batch))
        .transpose()?;
    let distinct = name.ends_with("_DISTINCT");
    let aggregate_name = name.strip_suffix("_DISTINCT").unwrap_or(&name);
    let col = col
        .map(|column| normalize_dictionary_columns(vec![column]))
        .transpose()?
        .and_then(|mut columns| columns.pop());
    let mut indices = vec![None; batch.num_rows()];
    let mut ints = vec![0i64; batch.num_rows()];
    let mut floats = vec![0.0f64; batch.num_rows()];
    let mut aggregate_results: Vec<(usize, ArrayRef)> = Vec::new();
    let offset = if name == "LAG" || name == "LEAD" {
        literal_offset(args.get(1), 1)?
    } else {
        0
    };
    let tiles = if name == "NTILE" {
        positive_literal_offset(args.first(), 0, "NTILE")?
    } else {
        1
    };
    let nth = if name == "NTH_VALUE" {
        positive_literal_offset(args.get(1), 1, "NTH_VALUE")?
    } else {
        1
    };
    if matches!(
        name.as_str(),
        "LAG" | "LEAD" | "FIRST_VALUE" | "LAST_VALUE" | "NTH_VALUE" | "SUM" | "AVG" | "MIN" | "MAX"
    ) && col.is_none()
    {
        return Err(error(format!("{name} requires a value argument")));
    }
    for rows in partitions {
        // A partition without an ORDER BY gives every row the same frame.
        // Cache distinct frame membership (and any repeated bounded frame)
        // instead of rebuilding the hash set once per output row.
        let mut distinct_frames: HashMap<(usize, usize), Vec<usize>> = HashMap::new();
        let mut starts = vec![0];
        let mut group = vec![0; rows.len()];
        for i in 1..rows.len() {
            if !peers(order_cols, rows[i - 1], rows[i])? {
                starts.push(i);
            }
            group[i] = starts.len() - 1;
        }
        starts.push(rows.len());
        for (pos, &row) in rows.iter().enumerate() {
            if pos % 64 == 0 {
                crate::expr_eval::check_expression_cancelled()?;
            }
            match aggregate_name {
                "ROW_NUMBER" => ints[row] = pos as i64 + 1,
                "RANK" => ints[row] = starts[group[pos]] as i64 + 1,
                "DENSE_RANK" => ints[row] = group[pos] as i64 + 1,
                "PERCENT_RANK" => {
                    floats[row] = if rows.len() <= 1 {
                        0.0
                    } else {
                        starts[group[pos]] as f64 / (rows.len() - 1) as f64
                    };
                }
                "CUME_DIST" => {
                    let peer_end = starts[group[pos] + 1];
                    floats[row] = peer_end as f64 / rows.len() as f64;
                }
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
                    let source = if ignore_nulls {
                        let mut candidate = Some(pos);
                        let mut remaining = offset;
                        while remaining > 0 {
                            let Some(current) = candidate else {
                                break;
                            };
                            candidate = if name == "LAG" {
                                current.checked_sub(1)
                            } else {
                                current.checked_add(1).filter(|&i| i < rows.len())
                            };
                            let Some(next) = candidate else {
                                break;
                            };
                            if col
                                .as_ref()
                                .is_some_and(|column| !column.is_null(rows[next]))
                            {
                                remaining -= 1;
                            }
                        }
                        if remaining == 0 {
                            candidate.map(|index| rows[index] as u32)
                        } else {
                            None
                        }
                    } else {
                        let source = if name == "LAG" {
                            pos.checked_sub(offset)
                        } else {
                            pos.checked_add(offset).filter(|&i| i < rows.len())
                        };
                        source.map(|index| rows[index] as u32)
                    };
                    indices[row] = source;
                }
                "COUNT" | "SUM" | "AVG" | "MIN" | "MAX" | "FIRST_VALUE" | "LAST_VALUE"
                | "NTH_VALUE" => {
                    let (start, end) =
                        frame_bounds(frame, pos, &rows, &starts, &group, order_cols, order_by)?;
                    let selected = if distinct {
                        let key = (start, end);
                        if let std::collections::hash_map::Entry::Vacant(e) =
                            distinct_frames.entry(key)
                        {
                            let unique = distinct_window_rows(col.as_ref(), &rows[start..end])?;
                            e.insert(unique);
                        }
                        distinct_frames
                            .get(&key)
                            .expect("distinct window frame was inserted")
                            .clone()
                    } else {
                        rows[start..end].to_vec()
                    };
                    let selected = if ignore_nulls {
                        let column = col
                            .as_ref()
                            .ok_or_else(|| error(format!("{name} requires a value argument")))?;
                        selected
                            .into_iter()
                            .filter(|&row| !column.is_null(row))
                            .collect::<Vec<_>>()
                    } else {
                        selected
                    };
                    match aggregate_name {
                        "COUNT" => {
                            ints[row] = selected
                                .iter()
                                .filter(|&&i| col.as_ref().is_none_or(|c| !c.is_null(i)))
                                .count() as i64
                        }
                        "FIRST_VALUE" => indices[row] = selected.first().map(|&i| i as u32),
                        "LAST_VALUE" => indices[row] = selected.last().map(|&i| i as u32),
                        "NTH_VALUE" => indices[row] = selected.get(nth - 1).map(|&i| i as u32),
                        _ => aggregate_results.push((
                            row,
                            aggregate(aggregate_name, col.as_ref().unwrap(), &selected)?,
                        )),
                    }
                }
                _ => return Err(error(format!("unsupported window function: {name}"))),
            }
        }
    }
    match aggregate_name {
        "ROW_NUMBER" | "RANK" | "DENSE_RANK" | "NTILE" | "COUNT" => {
            Ok(Arc::new(Int64Array::from(ints)))
        }
        "PERCENT_RANK" | "CUME_DIST" => Ok(Arc::new(Float64Array::from(floats))),
        "LAG" | "LEAD" | "FIRST_VALUE" | "LAST_VALUE" | "NTH_VALUE" => {
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

/// Return one row for each distinct non-null value in a window frame. Window
/// DISTINCT aggregates ignore nulls, and a dictionary column has already been
/// normalized to its logical value representation before reaching this helper.
fn distinct_window_rows(col: Option<&ArrayRef>, rows: &[usize]) -> Result<Vec<usize>> {
    let Some(col) = col else {
        return Err(error("DISTINCT window aggregate requires a value argument"));
    };
    let mut seen = HashSet::with_capacity(rows.len());
    let mut unique = Vec::with_capacity(rows.len());
    for &row in rows {
        if col.is_null(row) {
            continue;
        }
        let key = crate::distinct::extract_value(col.as_ref(), row)?;
        if seen.insert(key) {
            unique.push(row);
        }
    }
    Ok(unique)
}

fn literal_offset(expr: Option<&Expr>, default: usize) -> Result<usize> {
    let value = match expr {
        None => default,
        Some(Expr::Literal(kaveon_core::predicate::ScalarValue::Int64(n))) if *n >= 0 => {
            usize::try_from(*n).map_err(|_| error("window offset too large"))?
        }
        _ => return Err(error("window offset must be a nonnegative integer literal")),
    };
    Ok(value)
}

fn positive_literal_offset(expr: Option<&Expr>, default: usize, function: &str) -> Result<usize> {
    let value = match expr {
        None => default,
        Some(Expr::Literal(kaveon_core::predicate::ScalarValue::Int64(n))) if *n > 0 => {
            usize::try_from(*n).map_err(|_| error(format!("{function} offset is too large")))?
        }
        _ => return Err(error(format!("{function} requires a positive integer"))),
    };
    Ok(value)
}

fn validate_frame(frame: &WindowFrame, cols: &[ArrayRef], order_count: usize) -> Result<()> {
    use WindowFrameBound::*;
    if matches!(frame.start, UnboundedFollowing) || matches!(frame.end, UnboundedPreceding) {
        return Err(error("invalid unbounded window frame"));
    }
    let position = |b| match b {
        UnboundedPreceding => i128::MIN,
        Preceding(n) | PrecedingDays(n) | PrecedingMonths(n) | PrecedingSeconds(n)
        | PrecedingNanos(n) => -(n as i128),
        CurrentRow => 0,
        Following(n) | FollowingDays(n) | FollowingMonths(n) | FollowingSeconds(n)
        | FollowingNanos(n) => n as i128,
        UnboundedFollowing => i128::MAX,
    };
    if position(frame.start) > position(frame.end) {
        return Err(error("window frame start follows its end"));
    }
    if matches!(frame.units, WindowFrameUnits::Groups) && order_count == 0 {
        return Err(error("GROUPS frame requires ORDER BY"));
    }
    if matches!(frame.units, WindowFrameUnits::Range)
        && matches!(
            frame.start,
            Preceding(_)
                | Following(_)
                | PrecedingDays(_)
                | FollowingDays(_)
                | PrecedingMonths(_)
                | FollowingMonths(_)
                | PrecedingSeconds(_)
                | FollowingSeconds(_)
                | PrecedingNanos(_)
                | FollowingNanos(_)
        )
        || matches!(frame.units, WindowFrameUnits::Range)
            && matches!(
                frame.end,
                Preceding(_)
                    | Following(_)
                    | PrecedingDays(_)
                    | FollowingDays(_)
                    | PrecedingMonths(_)
                    | FollowingMonths(_)
                    | PrecedingSeconds(_)
                    | FollowingSeconds(_)
                    | PrecedingNanos(_)
                    | FollowingNanos(_)
            )
    {
        if order_count != 1 {
            return Err(error(
                "RANGE with offsets requires exactly one ORDER BY expression",
            ));
        }
        let has_numeric = matches!(frame.start, Preceding(_) | Following(_))
            || matches!(frame.end, Preceding(_) | Following(_));
        let has_days = matches!(frame.start, PrecedingDays(_) | FollowingDays(_))
            || matches!(frame.end, PrecedingDays(_) | FollowingDays(_));
        let has_months = matches!(frame.start, PrecedingMonths(_) | FollowingMonths(_))
            || matches!(frame.end, PrecedingMonths(_) | FollowingMonths(_));
        let has_seconds = matches!(frame.start, PrecedingSeconds(_) | FollowingSeconds(_))
            || matches!(frame.end, PrecedingSeconds(_) | FollowingSeconds(_));
        let has_nanos = matches!(frame.start, PrecedingNanos(_) | FollowingNanos(_))
            || matches!(frame.end, PrecedingNanos(_) | FollowingNanos(_));
        let numeric_order = matches!(
            cols[0].data_type(),
            DataType::Int8
                | DataType::Int16
                | DataType::Int32
                | DataType::UInt32
                | DataType::Int64
                | DataType::UInt64
                | DataType::Float32
                | DataType::Float64
                | DataType::Decimal128(_, _)
        );
        let timestamp_order = matches!(cols[0].data_type(), DataType::Timestamp(_, _));
        if (has_numeric && !numeric_order)
            || (has_days && !matches!(cols[0].data_type(), DataType::Date32))
            || (has_months && !matches!(cols[0].data_type(), DataType::Date32))
            || ((has_seconds || has_nanos) && !timestamp_order)
        {
            return Err(error(
                "RANGE offsets support numeric ordering; INTERVAL DAY/MONTH/YEAR require DATE ordering; fixed-unit SECOND intervals require TIMESTAMP ordering",
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
    order: &[(Expr, bool, bool)],
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
                    Preceding(n) | PrecedingDays(n) | PrecedingMonths(n) | PrecedingSeconds(n)
                    | PrecedingNanos(n) => -(n as i128),
                    Following(n) | FollowingDays(n) | FollowingMonths(n) | FollowingSeconds(n)
                    | FollowingNanos(n) => n as i128,
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
        let calendar_months = match b {
            PrecedingMonths(n) => Some(-(n as i128)),
            FollowingMonths(n) => Some(n as i128),
            _ => None,
        };
        let amount = match b {
            Preceding(n) | Following(n) => {
                if let Preceding(_) = b {
                    -(n as i128)
                } else {
                    n as i128
                }
            }
            PrecedingDays(n) => -(n as i128),
            FollowingDays(n) => n as i128,
            PrecedingMonths(_) | FollowingMonths(_) => 0,
            PrecedingSeconds(n) => -timestamp_seconds_in_units(&cols[0], n)?,
            FollowingSeconds(n) => timestamp_seconds_in_units(&cols[0], n)?,
            PrecedingNanos(n) => -timestamp_nanos_in_units(&cols[0], n)?,
            FollowingNanos(n) => timestamp_nanos_in_units(&cols[0], n)?,
            _ => unreachable!(),
        };
        let ascending = order[0].1;
        for (i, &row) in rows.iter().enumerate() {
            // NULLS LAST independently of direction; a finite boundary never includes NULL.
            if cols[0].is_null(row) {
                // With NULLS FIRST, null rows precede every finite value. They
                // must be skipped while finding a finite RANGE boundary; a
                // null current row was handled above as its peer group.
                if order[0].2 {
                    continue;
                }
                return Ok(i);
            }
            let mut cmp = if let Some(months) = calendar_months {
                compare_calendar_offset(&cols[0], row, rows[pos], months)?
            } else {
                compare_offset(
                    &cols[0],
                    row,
                    rows[pos],
                    if ascending { amount } else { -amount },
                )?
            };
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

fn compare_calendar_offset(
    col: &ArrayRef,
    row: usize,
    current: usize,
    months: i128,
) -> Result<Ordering> {
    let col = col
        .as_any()
        .downcast_ref::<arrow::array::Date32Array>()
        .ok_or_else(|| error("calendar RANGE offsets require DATE ordering"))?;
    let target = add_calendar_months(col.value(current), months)?;
    Ok(i128::from(col.value(row)).cmp(&target))
}

fn add_calendar_months(days: i32, months: i128) -> Result<i128> {
    let (year, month, day) = civil_from_days(i128::from(days));
    let total = year
        .checked_mul(12)
        .and_then(|value| value.checked_add(month - 1))
        .and_then(|value| value.checked_add(months))
        .ok_or_else(|| error("calendar RANGE interval overflows date arithmetic"))?;
    let target_year = total.div_euclid(12);
    let target_month = total.rem_euclid(12) + 1;
    let target_day = day.min(days_in_month(target_year, target_month));
    Ok(days_from_civil(target_year, target_month, target_day))
}

fn is_leap_year(year: i128) -> bool {
    year.rem_euclid(4) == 0 && (year.rem_euclid(100) != 0 || year.rem_euclid(400) == 0)
}

fn days_in_month(year: i128, month: i128) -> i128 {
    match month {
        2 => 28 + i128::from(is_leap_year(year)),
        4 | 6 | 9 | 11 => 30,
        _ => 31,
    }
}

fn civil_from_days(days: i128) -> (i128, i128, i128) {
    let z = days + 719_468;
    let era = (if z >= 0 { z } else { z - 146_096 }).div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096).div_euclid(365);
    let year = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2).div_euclid(153);
    let day = doy - (153 * mp + 2).div_euclid(5) + 1;
    let month = mp + if mp < 10 { 3 } else { -9 };
    (year + i128::from(month <= 2), month, day)
}

fn days_from_civil(year: i128, month: i128, day: i128) -> i128 {
    let year = year - i128::from(month <= 2);
    let era = (if year >= 0 { year } else { year - 399 }).div_euclid(400);
    let yoe = year - era * 400;
    let mp = month + if month > 2 { -3 } else { 9 };
    let doy = (153 * mp + 2).div_euclid(5) + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

fn timestamp_seconds_in_units(col: &ArrayRef, seconds: u64) -> Result<i128> {
    timestamp_nanos_in_units(
        col,
        seconds
            .checked_mul(1_000_000_000)
            .ok_or_else(|| error("timestamp RANGE interval overflows nanoseconds"))?,
    )
}

fn timestamp_nanos_in_units(col: &ArrayRef, nanos: u64) -> Result<i128> {
    let DataType::Timestamp(unit, _) = col.data_type() else {
        return Err(error("INTERVAL SECOND RANGE requires TIMESTAMP ordering"));
    };
    let divisor = match unit {
        TimeUnit::Second => 1_000_000_000,
        TimeUnit::Millisecond => 1_000_000,
        TimeUnit::Microsecond => 1_000,
        TimeUnit::Nanosecond => 1,
    };
    Ok(i128::from(nanos / divisor))
}

fn compare_offset(col: &ArrayRef, row: usize, current: usize, offset: i128) -> Result<Ordering> {
    let pair = match col.data_type() {
        DataType::Int8 => (
            col.as_primitive::<Int8Type>().value(row) as i128,
            col.as_primitive::<Int8Type>().value(current) as i128,
            offset,
        ),
        DataType::Int16 => (
            col.as_primitive::<Int16Type>().value(row) as i128,
            col.as_primitive::<Int16Type>().value(current) as i128,
            offset,
        ),
        DataType::Int32 => (
            col.as_primitive::<Int32Type>().value(row) as i128,
            col.as_primitive::<Int32Type>().value(current) as i128,
            offset,
        ),
        DataType::UInt32 => (
            col.as_primitive::<arrow::datatypes::UInt32Type>()
                .value(row) as i128,
            col.as_primitive::<arrow::datatypes::UInt32Type>()
                .value(current) as i128,
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
        DataType::Date32 => (
            col.as_primitive::<arrow::datatypes::Date32Type>()
                .value(row) as i128,
            col.as_primitive::<arrow::datatypes::Date32Type>()
                .value(current) as i128,
            offset,
        ),
        DataType::Timestamp(unit, _) => match unit {
            TimeUnit::Second => {
                let col = col.as_primitive::<arrow::datatypes::TimestampSecondType>();
                (col.value(row) as i128, col.value(current) as i128, offset)
            }
            TimeUnit::Millisecond => {
                let col = col.as_primitive::<arrow::datatypes::TimestampMillisecondType>();
                (col.value(row) as i128, col.value(current) as i128, offset)
            }
            TimeUnit::Microsecond => {
                let col = col.as_primitive::<arrow::datatypes::TimestampMicrosecondType>();
                (col.value(row) as i128, col.value(current) as i128, offset)
            }
            TimeUnit::Nanosecond => {
                let col = col.as_primitive::<arrow::datatypes::TimestampNanosecondType>();
                (col.value(row) as i128, col.value(current) as i128, offset)
            }
        },
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
        DataType::Float32 => {
            let col = col.as_primitive::<Float32Type>();
            let left = col.value(row) as f64;
            let right = col.value(current) as f64 + offset as f64;
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
    // Trino widens the signed integral SUM family (TINYINT through BIGINT)
    // to BIGINT.  Keep that widening exact instead of routing narrow Arrow
    // integers through the floating-point fallback.  Window aggregates use
    // the same evaluator for local and exchanged partitions, so this also
    // keeps a narrow cast stable across worker boundaries.
    if name == "SUM"
        && matches!(
            col.data_type(),
            DataType::Int8 | DataType::Int16 | DataType::Int32 | DataType::Int64
        )
    {
        let mut sum = 0i128;
        for &row in &rows {
            sum += match col.data_type() {
                DataType::Int8 => col.as_primitive::<Int8Type>().value(row) as i128,
                DataType::Int16 => col.as_primitive::<Int16Type>().value(row) as i128,
                DataType::Int32 => col.as_primitive::<Int32Type>().value(row) as i128,
                DataType::Int64 => col.as_primitive::<Int64Type>().value(row) as i128,
                _ => unreachable!("integral window SUM type was checked above"),
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
        DataType::Int32
            | DataType::Int64
            | DataType::UInt64
            | DataType::Float32
            | DataType::Float64
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
    use arrow::array::{
        Date32Array, DictionaryArray, Float32Array, Int8Array, Int16Array, Int32Array, StringArray,
        TimestampMillisecondArray, TimestampSecondArray, UInt32Array,
    };
    use arrow::datatypes::Int32Type;
    use kaveon_core::predicate::ScalarValue;
    fn batch(values: Vec<Option<i64>>) -> RecordBatch {
        RecordBatch::try_new(
            Arc::new(Schema::new(vec![Field::new("x", DataType::Int64, true)])),
            vec![Arc::new(Int64Array::from(values))],
        )
        .unwrap()
    }
    fn float_batch(values: Vec<Option<f32>>) -> RecordBatch {
        RecordBatch::try_new(
            Arc::new(Schema::new(vec![Field::new("x", DataType::Float32, true)])),
            vec![Arc::new(Float32Array::from(values))],
        )
        .unwrap()
    }
    fn uint32_batch(values: Vec<Option<u32>>) -> RecordBatch {
        RecordBatch::try_new(
            Arc::new(Schema::new(vec![Field::new("x", DataType::UInt32, true)])),
            vec![Arc::new(UInt32Array::from(values))],
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
            order_by: vec![(Expr::Column("x".into()), true, false)],
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
    fn distinct_window_count_ignores_duplicates_and_nulls() {
        let input = batch(vec![Some(1), Some(1), Some(2), None, Some(2)]);
        let expr = Expr::WindowFunction {
            name: "COUNT_DISTINCT".into(),
            args: vec![Expr::Column("x".into())],
            partition_by: vec![],
            order_by: vec![],
            frame: Some(WindowFrame {
                units: WindowFrameUnits::Rows,
                start: WindowFrameBound::UnboundedPreceding,
                end: WindowFrameBound::UnboundedFollowing,
            }),
        };
        assert_eq!(ints(expr, &input), vec![Some(2); 5]);
    }

    #[test]
    fn distinct_window_numeric_aggregates_ignore_duplicates_and_nulls() {
        let input = batch(vec![Some(1), Some(1), Some(2), None]);
        let whole_partition = frame(
            WindowFrameUnits::Rows,
            WindowFrameBound::UnboundedPreceding,
            WindowFrameBound::UnboundedFollowing,
        );
        let sum = evaluate_window(&expression("SUM_DISTINCT", whole_partition), &input)
            .unwrap()
            .as_primitive::<Int64Type>()
            .iter()
            .collect::<Vec<_>>();
        assert_eq!(sum, vec![Some(3); 4]);
        for (name, expected) in [("MIN_DISTINCT", 1), ("MAX_DISTINCT", 2)] {
            let values = evaluate_window(&expression(name, whole_partition), &input)
                .unwrap()
                .as_primitive::<Int64Type>()
                .iter()
                .collect::<Vec<_>>();
            assert_eq!(values, vec![Some(expected); 4], "{name}");
        }
        let values = evaluate_window(&expression("AVG_DISTINCT", whole_partition), &input)
            .unwrap()
            .as_primitive::<Float64Type>()
            .iter()
            .collect::<Vec<_>>();
        assert_eq!(values, vec![Some(1.5); 4]);
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
        let percent_rank = evaluate_window(&expression("PERCENT_RANK", None), &input)
            .unwrap()
            .as_primitive::<Float64Type>()
            .iter()
            .collect::<Vec<_>>();
        assert_eq!(
            percent_rank,
            vec![
                Some(0.0),
                Some(0.0),
                Some(0.4),
                Some(0.6),
                Some(0.8),
                Some(1.0)
            ]
        );
        let cume_dist = evaluate_window(&expression("CUME_DIST", None), &input)
            .unwrap()
            .as_primitive::<Float64Type>()
            .iter()
            .collect::<Vec<_>>();
        assert_eq!(
            cume_dist,
            vec![
                Some(2.0 / 6.0),
                Some(2.0 / 6.0),
                Some(3.0 / 6.0),
                Some(4.0 / 6.0),
                Some(5.0 / 6.0),
                Some(1.0)
            ]
        );
        let mut nth_value = expression("NTH_VALUE", frame(Rows, UnboundedPreceding, CurrentRow));
        if let Expr::WindowFunction { args, .. } = &mut nth_value {
            args.push(Expr::Literal(ScalarValue::Int64(2)));
        }
        assert_eq!(
            ints(nth_value, &input),
            vec![None, Some(1), Some(1), Some(1), Some(1), Some(1)]
        );
        let nullable = batch(vec![None, Some(1), None, Some(2)]);
        assert_eq!(
            ints(
                expression(
                    "FIRST_VALUE_IGNORE_NULLS",
                    frame(Rows, UnboundedPreceding, CurrentRow)
                ),
                &nullable,
            ),
            vec![Some(1), Some(1), Some(1), Some(1)]
        );
        assert_eq!(
            ints(expression("LAG_IGNORE_NULLS", None), &nullable),
            vec![Some(2), None, Some(2), Some(1)]
        );
        let mut lag_ignore = expression("LAG_IGNORE_NULLS", None);
        if let Expr::WindowFunction { args, .. } = &mut lag_ignore {
            args.push(Expr::Literal(ScalarValue::Int64(2)));
        }
        assert_eq!(
            ints(lag_ignore, &nullable),
            vec![Some(1), None, Some(1), None]
        );
        assert_eq!(
            ints(expression("LEAD_IGNORE_NULLS", None), &nullable),
            vec![None, Some(2), None, None]
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
    fn range_offsets_support_float32_ordering() {
        use WindowFrameBound::*;
        use WindowFrameUnits::*;
        let input = float_batch(vec![Some(1.0), Some(1.5), Some(2.0), None]);
        let expr = expression("COUNT", frame(Range, Preceding(1), CurrentRow));
        assert_eq!(ints(expr, &input), vec![Some(1), Some(2), Some(3), Some(1)]);
    }

    #[test]
    fn window_sum_widens_tinyint_and_real_without_float_input_loss() {
        let tinyint = RecordBatch::try_new(
            Arc::new(Schema::new(vec![Field::new("x", DataType::Int8, true)])),
            vec![Arc::new(Int8Array::from(vec![
                Some(2),
                Some(-1),
                Some(3),
                None,
            ]))],
        )
        .unwrap();
        assert_eq!(
            evaluate_window(&expression("SUM", None), &tinyint)
                .unwrap()
                .as_primitive::<Int64Type>()
                .iter()
                .collect::<Vec<_>>(),
            vec![Some(1), Some(-1), Some(4), Some(4)]
        );

        let smallint = RecordBatch::try_new(
            Arc::new(Schema::new(vec![Field::new("x", DataType::Int16, true)])),
            vec![Arc::new(Int16Array::from(vec![
                Some(300),
                Some(-100),
                None,
            ]))],
        )
        .unwrap();
        assert_eq!(
            evaluate_window(&expression("SUM", None), &smallint)
                .unwrap()
                .as_primitive::<Int64Type>()
                .iter()
                .collect::<Vec<_>>(),
            vec![Some(200), Some(-100), Some(200)]
        );

        let real = float_batch(vec![Some(1.5), Some(2.5), None]);
        assert_eq!(
            evaluate_window(&expression("SUM", None), &real)
                .unwrap()
                .as_primitive::<Float64Type>()
                .iter()
                .collect::<Vec<_>>(),
            vec![Some(1.5), Some(4.0), Some(4.0)]
        );
    }

    #[test]
    fn range_offsets_support_uint32_ordering() {
        use WindowFrameBound::*;
        use WindowFrameUnits::*;
        let input = uint32_batch(vec![Some(1), Some(2), Some(3), None]);
        let expr = expression("COUNT", frame(Range, Preceding(1), CurrentRow));
        assert_eq!(ints(expr, &input), vec![Some(1), Some(2), Some(2), Some(1)]);
    }

    #[test]
    fn range_offsets_support_int8_extremes_and_null_peers() {
        use WindowFrameBound::*;
        use WindowFrameUnits::*;
        let input = RecordBatch::try_new(
            Arc::new(Schema::new(vec![Field::new("x", DataType::Int8, true)])),
            vec![Arc::new(Int8Array::from(vec![
                Some(i8::MIN),
                Some(i8::MIN + 1),
                Some(0),
                Some(i8::MAX - 1),
                Some(i8::MAX),
                Some(i8::MAX),
                None,
                None,
            ]))],
        )
        .unwrap();
        for (ascending, start, end, expected) in [
            (true, Preceding(1), CurrentRow, vec![1, 2, 1, 1, 3, 3, 2, 2]),
            (true, CurrentRow, Following(1), vec![2, 1, 1, 3, 2, 2, 2, 2]),
            (
                false,
                Preceding(1),
                CurrentRow,
                vec![2, 1, 1, 3, 2, 2, 2, 2],
            ),
            (
                false,
                CurrentRow,
                Following(1),
                vec![1, 2, 1, 1, 3, 3, 2, 2],
            ),
        ] {
            for nulls_first in [true, false] {
                let mut expr = expression("COUNT", frame(Range, start, end));
                if let Expr::WindowFunction { order_by, .. } = &mut expr {
                    order_by[0].1 = ascending;
                    order_by[0].2 = nulls_first;
                }
                assert_eq!(
                    ints(expr, &input),
                    expected.iter().copied().map(Some).collect::<Vec<_>>()
                );
            }
        }
    }

    #[test]
    fn range_offsets_support_int16_extremes_and_null_peers() {
        use WindowFrameBound::*;
        use WindowFrameUnits::*;
        let input = RecordBatch::try_new(
            Arc::new(Schema::new(vec![Field::new("x", DataType::Int16, true)])),
            vec![Arc::new(Int16Array::from(vec![
                Some(i16::MIN),
                Some(i16::MIN + 1),
                Some(0),
                Some(i16::MAX - 1),
                Some(i16::MAX),
                Some(i16::MAX),
                None,
                None,
            ]))],
        )
        .unwrap();
        for (ascending, start, end, expected) in [
            (true, Preceding(1), CurrentRow, vec![1, 2, 1, 1, 3, 3, 2, 2]),
            (true, CurrentRow, Following(1), vec![2, 1, 1, 3, 2, 2, 2, 2]),
            (
                false,
                Preceding(1),
                CurrentRow,
                vec![2, 1, 1, 3, 2, 2, 2, 2],
            ),
            (
                false,
                CurrentRow,
                Following(1),
                vec![1, 2, 1, 1, 3, 3, 2, 2],
            ),
        ] {
            for nulls_first in [true, false] {
                let mut expr = expression("COUNT", frame(Range, start, end));
                if let Expr::WindowFunction { order_by, .. } = &mut expr {
                    order_by[0].1 = ascending;
                    order_by[0].2 = nulls_first;
                }
                assert_eq!(
                    ints(expr, &input),
                    expected.iter().copied().map(Some).collect::<Vec<_>>()
                );
            }
        }
    }

    #[test]
    fn range_offsets_with_nulls_first_skip_null_boundaries() {
        use WindowFrameBound::*;
        use WindowFrameUnits::*;
        let input = batch(vec![Some(1), Some(2), None]);
        let mut expr = expression("COUNT", frame(Range, Preceding(1), CurrentRow));
        if let Expr::WindowFunction { order_by, .. } = &mut expr {
            order_by[0].2 = true;
        }
        // The finite RANGE boundary for 1 and 2 must not terminate at the
        // leading NULLS FIRST row. A NULL current row remains its peer group.
        assert_eq!(ints(expr, &input), vec![Some(1), Some(2), Some(1)]);
    }

    #[test]
    fn explicit_nulls_first_changes_window_order() {
        let input = batch(vec![Some(1), None, Some(2)]);
        let mut expr = expression("RANK", None);
        if let Expr::WindowFunction { order_by, .. } = &mut expr {
            order_by[0].2 = true;
        }
        assert_eq!(ints(expr, &input), vec![Some(2), Some(1), Some(3)]);
    }

    #[test]
    fn date_range_interval_day_offsets_use_calendar_day_values() {
        let dates = Date32Array::from(vec![10, 11, 13, 14]);
        let values = Int64Array::from(vec![1, 1, 1, 1]);
        let batch = RecordBatch::try_from_iter([
            ("d", Arc::new(dates) as ArrayRef),
            ("v", Arc::new(values) as ArrayRef),
        ])
        .unwrap();
        let expr = Expr::WindowFunction {
            name: "SUM".into(),
            args: vec![Expr::Column("v".into())],
            partition_by: vec![],
            order_by: vec![(Expr::Column("d".into()), true, false)],
            frame: Some(WindowFrame {
                units: WindowFrameUnits::Range,
                start: WindowFrameBound::PrecedingDays(2),
                end: WindowFrameBound::FollowingDays(1),
            }),
        };
        let output = evaluate_window(&expr, &batch).unwrap();
        assert_eq!(
            output.as_primitive::<Int64Type>().values().as_ref(),
            &[2, 2, 3, 2]
        );
    }

    #[test]
    fn timestamp_range_interval_second_offsets_use_physical_seconds() {
        let timestamps = TimestampSecondArray::from(vec![1000, 1001, 1003, 1004]);
        let values = Int64Array::from(vec![1, 1, 1, 1]);
        let batch = RecordBatch::try_from_iter([
            ("ts", Arc::new(timestamps) as ArrayRef),
            ("v", Arc::new(values) as ArrayRef),
        ])
        .unwrap();
        let expr = Expr::WindowFunction {
            name: "SUM".into(),
            args: vec![Expr::Column("v".into())],
            partition_by: vec![],
            order_by: vec![(Expr::Column("ts".into()), true, false)],
            frame: Some(WindowFrame {
                units: WindowFrameUnits::Range,
                start: WindowFrameBound::PrecedingSeconds(2),
                end: WindowFrameBound::FollowingSeconds(1),
            }),
        };
        let output = evaluate_window(&expr, &batch).unwrap();
        assert_eq!(
            output.as_primitive::<Int64Type>().values().as_ref(),
            &[2, 2, 3, 2]
        );
    }

    #[test]
    fn timestamp_range_fractional_second_offsets_scale_to_milliseconds() {
        let timestamps = TimestampMillisecondArray::from(vec![1000, 2000, 2250, 3000]);
        let values = Int64Array::from(vec![1, 1, 1, 1]);
        let batch = RecordBatch::try_from_iter([
            ("ts", Arc::new(timestamps) as ArrayRef),
            ("v", Arc::new(values) as ArrayRef),
        ])
        .unwrap();
        let expr = Expr::WindowFunction {
            name: "SUM".into(),
            args: vec![Expr::Column("v".into())],
            partition_by: vec![],
            order_by: vec![(Expr::Column("ts".into()), true, false)],
            frame: Some(WindowFrame {
                units: WindowFrameUnits::Range,
                start: WindowFrameBound::PrecedingNanos(1_250_000_000),
                end: WindowFrameBound::FollowingNanos(500_000_000),
            }),
        };
        let output = evaluate_window(&expr, &batch).unwrap();
        assert_eq!(
            output.as_primitive::<Int64Type>().values().as_ref(),
            &[1, 3, 3, 3]
        );
    }

    #[test]
    fn date_range_interval_month_clamps_calendar_end_days() {
        let dates = Date32Array::from(vec![18_292, 18_321, 18_352]);
        let values = Int64Array::from(vec![1, 1, 1]);
        let batch = RecordBatch::try_from_iter([
            ("d", Arc::new(dates) as ArrayRef),
            ("v", Arc::new(values) as ArrayRef),
        ])
        .unwrap();
        let expr = Expr::WindowFunction {
            name: "SUM".into(),
            args: vec![Expr::Column("v".into())],
            partition_by: vec![],
            order_by: vec![(Expr::Column("d".into()), true, false)],
            frame: Some(WindowFrame {
                units: WindowFrameUnits::Range,
                start: WindowFrameBound::PrecedingMonths(1),
                end: WindowFrameBound::CurrentRow,
            }),
        };
        let output = evaluate_window(&expr, &batch).unwrap();
        assert_eq!(
            output.as_primitive::<Int64Type>().values().as_ref(),
            &[1, 2, 2]
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

    #[test]
    fn partitioned_window_compares_dictionary_values_not_local_codes() {
        let key = DictionaryArray::<Int32Type>::try_new(
            Int32Array::from(vec![0, 1, 0, 1]),
            Arc::new(StringArray::from(vec!["east", "west"])),
        )
        .unwrap();
        let input = RecordBatch::try_new(
            Arc::new(Schema::new(vec![
                Field::new("region", key.data_type().clone(), false),
                Field::new("amount", DataType::Int64, false),
            ])),
            vec![
                Arc::new(key),
                Arc::new(Int64Array::from(vec![1, 10, 2, 20])),
            ],
        )
        .unwrap();
        let expr = Expr::WindowFunction {
            name: "SUM".into(),
            args: vec![Expr::Column("amount".into())],
            partition_by: vec![Expr::Column("region".into())],
            order_by: vec![],
            frame: None,
        };
        assert_eq!(
            evaluate_window(&expr, &input)
                .unwrap()
                .as_primitive::<Int64Type>()
                .iter()
                .collect::<Vec<_>>(),
            vec![Some(3), Some(30), Some(3), Some(30)]
        );
    }

    #[test]
    fn partitioned_window_keeps_null_keys_together_with_ordered_frames() {
        let input = RecordBatch::try_new(
            Arc::new(Schema::new(vec![
                Field::new("group", DataType::Utf8, true),
                Field::new("x", DataType::Int64, false),
            ])),
            vec![
                Arc::new(StringArray::from(vec![None, Some("a"), None, Some("a")])),
                Arc::new(Int64Array::from(vec![2, 1, 1, 3])),
            ],
        )
        .unwrap();
        let expr = Expr::WindowFunction {
            name: "ROW_NUMBER".into(),
            args: vec![],
            partition_by: vec![Expr::Column("group".into())],
            order_by: vec![(Expr::Column("x".into()), true, false)],
            frame: None,
        };
        assert_eq!(
            evaluate_window(&expr, &input)
                .unwrap()
                .as_primitive::<Int64Type>()
                .iter()
                .collect::<Vec<_>>(),
            vec![Some(2), Some(1), Some(1), Some(2)]
        );
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
