use arrow::array::{
    Array, ArrayRef, AsArray, BooleanArray, BooleanBuilder, Date32Array, Date64Array,
    Decimal128Array,
    Decimal128Builder,
    Float32Array, Float64Array, Float64Builder,
    Int16Array, Int32Array, Int32Builder, Int32DictionaryArray, Int64Array, Int64Builder,
    Int8Array, LargeListArray,
    ListArray, GenericListArray, OffsetSizeTrait,
    ListBuilder, MapArray,
    NullBuilder, PrimitiveArray, PrimitiveBuilder,
    StringArray,
    StringBuilder, StructArray,
    Time32MillisecondArray, Time32SecondArray, Time64MicrosecondArray, Time64NanosecondArray,
    TimestampMicrosecondArray, TimestampMillisecondArray, TimestampNanosecondArray,
    TimestampSecondArray, UInt16Array, UInt32Array, UInt64Array, UInt8Array,
};
use arrow::buffer::{OffsetBuffer, ScalarBuffer};
use arrow::compute;
use arrow::datatypes::{
    ArrowPrimitiveType, DataType, Date32Type, Date64Type, Float32Type, Float64Type, Int32Type,
    Int64Type,
    Time32MillisecondType, Time32SecondType, Time64MicrosecondType, Time64NanosecondType,
    TimestampMicrosecondType,
    TimestampMillisecondType, TimestampNanosecondType, TimestampSecondType, TimeUnit, UInt16Type,
    UInt32Type, UInt64Type, UInt8Type,
    Int16Type, Int8Type,
};
use arrow::record_batch::RecordBatch;
use kaveon_core::predicate::ScalarValue;
use kaveon_core::{BinaryOp, CastTarget, DateField, Expr, KaveonError, Result};
use std::sync::Arc;

struct ExpressionBudget {
    account: kaveon_core::OperatorMemoryAccount,
    reservations: Vec<kaveon_core::MemoryReservation>,
}

thread_local! {
    static EXPRESSION_MEMORY: std::cell::RefCell<Option<ExpressionBudget>> = const { std::cell::RefCell::new(None) };
}

/// Synchronous expression evaluation inherits its caller's query budget,
/// including recursively nested function calls. The prior scope is restored
/// even when evaluation returns an error or unwinds.
pub fn with_expression_memory<T>(
    memory: Option<&kaveon_core::OperatorMemoryAccount>,
    action: impl FnOnce() -> Result<T>,
) -> Result<T> {
    struct Restore(Option<ExpressionBudget>);
    impl Drop for Restore {
        fn drop(&mut self) {
            EXPRESSION_MEMORY.with(|slot| {
                slot.replace(self.0.take());
            });
        }
    }
    let Some(memory) = memory else {
        return action();
    };
    let _restore = Restore(EXPRESSION_MEMORY.with(|slot| {
        slot.replace(Some(ExpressionBudget {
            account: memory.clone(),
            reservations: Vec::new(),
        }))
    }));
    action()
}

pub fn check_expression_cancelled() -> Result<()> {
    EXPRESSION_MEMORY.with(|slot| {
        slot.borrow()
            .as_ref()
            .map(|budget| budget.account.check_cancelled())
            .unwrap_or(Ok(()))
    })
}

/// Compiled regular expressions by pattern, shared by every batch and
/// query in the process: a pattern is compiled once, not once per batch.
/// Bounded; a burst of distinct patterns clears it rather than growing it.
fn compiled_regex(pattern: &str) -> Result<regex::Regex> {
    compiled_regex_for("REGEXP_REPLACE", pattern)
}

fn compiled_regex_for(function: &str, pattern: &str) -> Result<regex::Regex> {
    use std::collections::HashMap;
    use std::sync::{Mutex, OnceLock};
    const MAX_CACHED_PATTERNS: usize = 256;
    static CACHE: OnceLock<Mutex<HashMap<String, regex::Regex>>> = OnceLock::new();
    let cache = CACHE.get_or_init(|| Mutex::new(HashMap::new()));
    if let Some(regex) = cache
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .get(pattern)
    {
        return Ok(regex.clone());
    }
    let regex = regex::Regex::new(pattern).map_err(|error| {
        KaveonError::Execution(format!(
            "{function} pattern {pattern:?} is invalid: {error}"
        ))
    })?;
    let mut cache = cache
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    if cache.len() >= MAX_CACHED_PATTERNS {
        cache.clear();
    }
    cache.insert(pattern.to_owned(), regex.clone());
    Ok(regex)
}

fn reserve_string_expansion(bytes: u64, rows: usize) -> Result<()> {
    const MAX_EXPANSION_BYTES: u64 = 64 * 1024 * 1024;
    if bytes > MAX_EXPANSION_BYTES {
        return Err(KaveonError::Execution(format!(
            "string expression output exceeds {MAX_EXPANSION_BYTES} bytes per batch"
        )));
    }
    let workspace = bytes
        .checked_mul(2)
        .and_then(|bytes| bytes.checked_add((rows as u64).saturating_mul(16)))
        .ok_or_else(|| {
            KaveonError::Execution("string expression memory estimate overflow".into())
        })?;
    EXPRESSION_MEMORY.with(|slot| {
        if let Some(budget) = slot.borrow_mut().as_mut() {
            budget.reservations.push(budget.account.reserve(workspace)?);
        }
        Ok(())
    })
}

pub fn evaluate(expr: &Expr, batch: &RecordBatch) -> Result<ArrayRef> {
    match expr {
        Expr::Column(name) => resolve_column(name, batch),
        Expr::Literal(value) => literal_to_array(value, batch.num_rows()),
        Expr::BinaryOp { left, op, right } => {
            // A column against a literal is the common filter shape; compare
            // it as a scalar so the literal is never expanded to a batch-long
            // array, and through the dictionary when the column has one.
            if let Some(result) = compare_column_with_literal(left, *op, right, batch)? {
                return Ok(result);
            }
            // Arithmetic against a literal takes the scalar kernel: the
            // literal is never expanded to a batch-long array. Ninety
            // `SUM(width + k)` projections in one statement are ninety
            // fewer allocations per batch.
            if let Some(result) = arithmetic_with_literal(left, *op, right, batch)? {
                return Ok(result);
            }
            let left_arr = evaluate(left, batch)?;
            let right_arr = evaluate(right, batch)?;
            eval_binary_op(&left_arr, *op, &right_arr)
        }
        Expr::IsNull(inner) => {
            let arr = evaluate(inner, batch)?;
            Ok(Arc::new(compute::is_null(&arr)?))
        }
        Expr::IsNotNull(inner) => {
            let arr = evaluate(inner, batch)?;
            Ok(Arc::new(compute::is_not_null(&arr)?))
        }
        Expr::Not(inner) => {
            let arr = evaluate(inner, batch)?;
            let bool_arr = as_boolean(&arr)?;
            Ok(Arc::new(compute::not(bool_arr)?))
        }
        Expr::And(left, right) => {
            let l = evaluate(left, batch)?;
            let r = evaluate(right, batch)?;
            Ok(Arc::new(compute::and(as_boolean(&l)?, as_boolean(&r)?)?))
        }
        Expr::Or(left, right) => {
            let l = evaluate(left, batch)?;
            let r = evaluate(right, batch)?;
            Ok(Arc::new(compute::or(as_boolean(&l)?, as_boolean(&r)?)?))
        }
        Expr::Case {
            operand,
            when_then,
            else_expr,
        } => eval_case(operand.as_deref(), when_then, else_expr.as_deref(), batch),
        Expr::Like {
            expr,
            pattern,
            negated,
            case_insensitive,
        } => eval_like(expr, pattern, *negated, *case_insensitive, batch),
        Expr::Between {
            expr,
            low,
            high,
            negated,
        } => eval_between(expr, low, high, *negated, batch),
        Expr::InList {
            expr,
            list,
            negated,
        } => eval_in_list(expr, list, *negated, batch),
        Expr::Cast { expr, data_type } => eval_cast(expr, *data_type, batch),
        Expr::Function { name, args } => {
            if let Some(result) = eval_function_through_dictionary(name, args, batch)? {
                return Ok(result);
            }
            let evaluated_args: Vec<ArrayRef> = args
                .iter()
                .map(|a| {
                    evaluate(a, batch)
                        .and_then(|array| decode_dictionary(&array))
                        .and_then(|array| normalize_large_list(&array))
                })
                .collect::<Result<_>>()?;
            eval_scalar_function(name, &evaluated_args, batch.num_rows())
        }
        Expr::Alias { expr, .. } => evaluate(expr, batch),
        Expr::Extract { field, expr } => eval_extract(*field, expr, batch),
        Expr::WindowFunction { .. } => {
            resolve_column(&crate::window::window_output_name(expr), batch)
        }
        Expr::Star => Err(KaveonError::Execution(
            "star (*) cannot be evaluated as an expression".into(),
        )),
    }
}

fn normalize_large_list(array: &ArrayRef) -> Result<ArrayRef> {
    let DataType::LargeList(field) = array.data_type() else {
        return Ok(Arc::clone(array));
    };
    Ok(compute::cast(array, &DataType::List(field.clone()))?)
}

pub fn evaluate_predicate(expr: &Expr, batch: &RecordBatch) -> Result<BooleanArray> {
    let arr = evaluate(expr, batch)?;
    as_boolean(&arr).cloned()
}

/// The columns `expr` reads, as written, in evaluation order; `*` is not
/// a column.
pub fn column_references(expr: &Expr, into: &mut Vec<String>) {
    match expr {
        Expr::Column(name) => {
            if name != "*" {
                into.push(name.clone());
            }
        }
        Expr::Literal(_) | Expr::Star => {}
        Expr::Alias { expr, .. }
        | Expr::Not(expr)
        | Expr::IsNull(expr)
        | Expr::IsNotNull(expr)
        | Expr::Cast { expr, .. }
        | Expr::Extract { expr, .. } => column_references(expr, into),
        Expr::BinaryOp { left, right, .. } | Expr::And(left, right) | Expr::Or(left, right) => {
            column_references(left, into);
            column_references(right, into);
        }
        Expr::Function { args, .. } => {
            for arg in args {
                column_references(arg, into);
            }
        }
        Expr::WindowFunction {
            args,
            partition_by,
            order_by,
            ..
        } => {
            for expr in args
                .iter()
                .chain(partition_by)
                .chain(order_by.iter().map(|(expr, _, _)| expr))
            {
                column_references(expr, into);
            }
        }
        Expr::Case {
            operand,
            when_then,
            else_expr,
        } => {
            for expr in operand.iter().chain(else_expr) {
                column_references(expr, into);
            }
            for (when, then) in when_then {
                column_references(when, into);
                column_references(then, into);
            }
        }
        Expr::Like { expr, pattern, .. } => {
            column_references(expr, into);
            column_references(pattern, into);
        }
        Expr::Between {
            expr, low, high, ..
        } => {
            column_references(expr, into);
            column_references(low, into);
            column_references(high, into);
        }
        Expr::InList { expr, list, .. } => {
            column_references(expr, into);
            for item in list {
                column_references(item, into);
            }
        }
    }
}

/// The index of `name` in `schema`: the field named exactly `name`, else
/// the one field whose bare name is `name`'s bare name. A join qualifies
/// its output as `relation.column`, so a bare `c_name` reaches
/// `customer.c_name` and a qualified `t.x` reaches a scan's bare `x`.
pub fn resolve_column_index(schema: &arrow::datatypes::Schema, name: &str) -> Result<usize> {
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
        [] => Err(KaveonError::Execution(format!(
            "column '{name}' not found in batch"
        ))),
        _ => Err(KaveonError::Execution(format!(
            "column '{name}' is ambiguous in batch"
        ))),
    }
}

fn resolve_column(name: &str, batch: &RecordBatch) -> Result<ArrayRef> {
    let schema = batch.schema();
    let idx = resolve_column_index(&schema, name)?;
    Ok(Arc::clone(batch.column(idx)))
}

/// `column <op> literal` (or the mirror) with a same-typed literal: one
/// comparison of the column against a scalar, or of a dictionary's values
/// against the scalar followed by a gather through its keys. Returns None
/// for every other shape, which takes the general path.
fn compare_column_with_literal(
    left: &Expr,
    op: BinaryOp,
    right: &Expr,
    batch: &RecordBatch,
) -> Result<Option<ArrayRef>> {
    use arrow::array::{AsArray, Datum, Scalar};
    use arrow::compute::kernels::cmp::{eq, gt, gt_eq, lt, lt_eq, neq};
    let (column, literal, op) = match (left, right) {
        (Expr::Column(name), Expr::Literal(value)) => (name, value, op),
        (Expr::Literal(value), Expr::Column(name)) => (
            name,
            value,
            match op {
                BinaryOp::Lt => BinaryOp::Gt,
                BinaryOp::Le => BinaryOp::Ge,
                BinaryOp::Gt => BinaryOp::Lt,
                BinaryOp::Ge => BinaryOp::Le,
                other => other,
            },
        ),
        _ => return Ok(None),
    };
    if !matches!(
        op,
        BinaryOp::Eq | BinaryOp::Ne | BinaryOp::Lt | BinaryOp::Le | BinaryOp::Gt | BinaryOp::Ge
    ) {
        return Ok(None);
    }
    let array = resolve_column(column, batch)?;
    let value_type = match array.data_type() {
        DataType::Dictionary(key_type, values) if key_type.as_ref() == &DataType::Int32 => {
            values.as_ref()
        }
        DataType::Dictionary(_, _) => return Ok(None),
        other => other,
    };
    let literal = literal.coerced_for(value_type);
    let scalar: ArrayRef = match (&literal, value_type) {
        (ScalarValue::Utf8(v), DataType::Utf8) => Arc::new(StringArray::from(vec![v.as_str()])),
        (ScalarValue::Int64(v), DataType::Int64) => Arc::new(Int64Array::from(vec![*v])),
        // A day number (from a DATE or a coerced text literal) against a
        // date column, in the column's own type.
        (ScalarValue::Int64(v), DataType::Date32) => {
            let Ok(days) = i32::try_from(*v) else {
                return Ok(None);
            };
            Arc::new(arrow::array::Date32Array::from(vec![days]))
        }
        (ScalarValue::Float64(v), DataType::Float64) => Arc::new(Float64Array::from(vec![*v])),
        (ScalarValue::Bool(v), DataType::Boolean) => Arc::new(BooleanArray::from(vec![*v])),
        _ => return Ok(None),
    };
    let scalar = Scalar::new(scalar);
    let compare = |values: &dyn Datum| -> Result<BooleanArray> {
        Ok(match op {
            BinaryOp::Eq => eq(values, &scalar)?,
            BinaryOp::Ne => neq(values, &scalar)?,
            BinaryOp::Lt => lt(values, &scalar)?,
            BinaryOp::Le => lt_eq(values, &scalar)?,
            BinaryOp::Gt => gt(values, &scalar)?,
            BinaryOp::Ge => gt_eq(values, &scalar)?,
            _ => unreachable!("filtered above"),
        })
    };
    if let DataType::Dictionary(_, _) = array.data_type() {
        let dictionary = array.as_dictionary::<Int32Type>();
        let verdicts = compare(dictionary.values())?;
        let gathered = compute::take(&verdicts, dictionary.keys(), None)?;
        return Ok(Some(gathered));
    }
    Ok(Some(Arc::new(compare(&array)?)))
}

/// Row-wise functions that map a null input to a null output, so evaluating
/// them over a dictionary's values and gathering by key is exactly the
/// per-row result.
const NULL_PROPAGATING_FUNCTIONS: &[&str] = &[
    "REGEXP_REPLACE",
    "REGEXP_EXTRACT",
    "REGEXP_COUNT",
    "REGEXP_EXTRACT_ALL",
    "UPPER",
    "LOWER",
    "TRIM",
    "LTRIM",
    "RTRIM",
    "LENGTH",
    "LEN",
    "CHAR_LENGTH",
    "CHARACTER_LENGTH",
    "SUBSTR",
    "SUBSTRING",
    "REPLACE",
    "LEFT",
    "RIGHT",
    "LPAD",
    "RPAD",
    "STARTS_WITH",
    "ENDS_WITH",
    "CONTAINS",
    "STRPOS",
    "POSITION",
    "SPLIT_PART",
    "REVERSE",
    "REPEAT",
];

/// A function over one dictionary column and literals runs once per
/// dictionary value the batch uses, not once per row: `UPPER(surface)`
/// over a batch is a handful of string operations, and
/// `REGEXP_REPLACE(Referer, …)` over a batch of 8192 rows that repeat
/// four hundred values is four hundred regular expressions. A text result
/// stays dictionary-encoded over the transformed values, so no bytes are
/// copied per row; every other result is gathered by key.
///
/// The same holds for `REGEXP_REPLACE` over a plain text column: the
/// batch's distinct values are found first and the pattern runs once per
/// distinct value, since a regular expression with captures costs
/// hundreds of nanoseconds a row where a hash costs tens. The result is
/// plain text, as the input was.
fn eval_function_through_dictionary(
    name: &str,
    args: &[Expr],
    batch: &RecordBatch,
) -> Result<Option<ArrayRef>> {
    let function = name.to_uppercase();
    if !NULL_PROPAGATING_FUNCTIONS.contains(&function.as_str()) {
        return Ok(None);
    }
    let mut column = None;
    for (index, arg) in args.iter().enumerate() {
        match arg {
            Expr::Literal(_) => {}
            Expr::Column(name) if column.is_none() => {
                column = Some((index, resolve_column(name, batch)?));
            }
            _ => return Ok(None),
        }
    }
    let Some((position, array)) = column else {
        return Ok(None);
    };
    let regex = function == "REGEXP_REPLACE";
    let (values, keys) = match array.data_type() {
        DataType::Dictionary(key, _) if key.as_ref() == &DataType::Int32 => {
            let dictionary = array.as_dictionary::<Int32Type>();
            let cost = if regex {
                ValueCost::Regex
            } else {
                ValueCost::StringFunction
            };
            used_dictionary_values(dictionary, compact_dictionary_above(dictionary.len(), cost))?
        }
        DataType::Utf8 if regex => match distinct_text_values(array.as_string::<i32>()) {
            Some((values, keys)) => (Arc::new(values) as ArrayRef, keys),
            None => return Ok(None),
        },
        _ => return Ok(None),
    };
    let evaluated: Vec<ArrayRef> = args
        .iter()
        .enumerate()
        .map(|(index, arg)| {
            if index == position {
                Ok(Arc::clone(&values))
            } else {
                match arg {
                    Expr::Literal(value) => literal_to_array(value, values.len()),
                    _ => unreachable!("only literals remain"),
                }
            }
        })
        .collect::<Result<_>>()?;
    let over_values = eval_scalar_function(name, &evaluated, values.len())?;
    if matches!(array.data_type(), DataType::Dictionary(_, _)) {
        return rewrap_dictionary(&keys, over_values).map(Some);
    }
    // Plain in, plain out: the rows gather their distinct value's result.
    if let DataType::Utf8 | DataType::LargeUtf8 = over_values.data_type() {
        reserve_string_expansion(array.get_array_memory_size() as u64, keys.len())?;
    }
    Ok(Some(compute::take(&over_values, &keys, None)?))
}

/// What one evaluation over one dictionary value costs, for deciding
/// whether a pass over the keys to skip the unused values pays.
#[derive(Clone, Copy)]
enum ValueCost {
    /// A regular expression with captures: hundreds of nanoseconds.
    Regex,
    /// LIKE or a string function: tens of nanoseconds.
    StringFunction,
}

/// Dictionaries this small are used whole whatever the function: a
/// compaction pass over the keys would cost more than the evaluations it
/// saves.
const SMALL_DICTIONARY: usize = 64;

/// The dictionary size above which a batch of `rows` compacts the
/// dictionary to the values it uses before a function runs over them.
/// Compaction is a pass over the keys at a couple of nanoseconds a row;
/// it pays once the values it skips would cost more. A regular expression
/// costs hundreds of nanoseconds a value, so a dictionary of more than a
/// few dozen values is compacted; a string function or LIKE costs tens,
/// so the dictionary must hold more than an eighth of the rows — a
/// row group's dictionary handed to an 8192-row batch, not a batch's own
/// few hundred values.
fn compact_dictionary_above(rows: usize, cost: ValueCost) -> usize {
    match cost {
        ValueCost::Regex => SMALL_DICTIONARY,
        ValueCost::StringFunction => (rows / 8).max(SMALL_DICTIONARY),
    }
}

/// A batch's dictionary column reduced to the values its keys use, with
/// the keys renumbered to match. The dictionary a row group decodes to is
/// shared by every batch of the row group — the reader hands each batch
/// the whole dictionary page — so a function evaluated over the values as
/// they come runs over the row group's distinct values once per batch,
/// several times the row count. Nulls stay null; a key at a null row is
/// not read, since the reader leaves those slots arbitrary. The values and
/// keys come back as they are when every value is used, or the dictionary
/// holds no more than `compact_above` values.
fn used_dictionary_values(
    dictionary: &Int32DictionaryArray,
    compact_above: usize,
) -> Result<(ArrayRef, Int32Array)> {
    let values = dictionary.values();
    let keys = dictionary.keys();
    if values.len() <= compact_above {
        return Ok((Arc::clone(values), keys.clone()));
    }
    let nulls = keys.nulls();
    let mut renumbered = vec![-1_i32; values.len()];
    let mut used: Vec<i32> = Vec::new();
    for (row, &code) in keys.values().iter().enumerate() {
        if nulls.is_some_and(|nulls| nulls.is_null(row)) {
            continue;
        }
        let index = usize::try_from(code)
            .ok()
            .filter(|&index| index < values.len())
            .ok_or_else(|| {
                KaveonError::Execution(format!(
                    "dictionary key {code} is outside its {} values",
                    values.len()
                ))
            })?;
        if renumbered[index] < 0 {
            renumbered[index] = used.len() as i32;
            used.push(code);
        }
    }
    if used.len() == values.len() {
        return Ok((Arc::clone(values), keys.clone()));
    }
    let compact_values = compute::take(values, &Int32Array::from(used), None)?;
    let compact_keys = keys
        .values()
        .iter()
        .enumerate()
        .map(|(row, &code)| {
            if nulls.is_some_and(|nulls| nulls.is_null(row)) {
                0
            } else {
                renumbered[code as usize]
            }
        })
        .collect::<Vec<_>>();
    let compact_keys = Int32Array::new(compact_keys.into(), nulls.cloned());
    Ok((compact_values, compact_keys))
}

/// A function's result over a dictionary's values, back as the batch's
/// rows. Text stays a dictionary over the transformed values — the rows
/// are the keys, and no bytes are copied per row; anything else is
/// gathered by key. A null result is a null key, so the result's nulls are
/// exact without consulting its values.
fn rewrap_dictionary(keys: &Int32Array, values: ArrayRef) -> Result<ArrayRef> {
    match values.data_type() {
        DataType::Utf8 | DataType::LargeUtf8 => {
            let keys = if values.null_count() == 0 {
                keys.clone()
            } else {
                keys.iter()
                    .map(|key| key.filter(|&key| !values.is_null(key as usize)))
                    .collect::<Int32Array>()
            };
            Ok(Arc::new(Int32DictionaryArray::try_new(keys, values)?))
        }
        _ => Ok(compute::take(&values, keys, None)?),
    }
}

/// The first rows a batch is judged on before its distinct values are
/// collected in full: a batch whose values are mostly distinct is left to
/// the row path, where the hash would be pure overhead.
const DISTINCT_PROBE_ROWS: usize = 1024;

/// A plain text column's distinct values in first-seen order, with the
/// rows as keys into them; None when the batch is mostly distinct (more
/// than three quarters of its first `DISTINCT_PROBE_ROWS` rows).
fn distinct_text_values(array: &StringArray) -> Option<(StringArray, Int32Array)> {
    let rows = array.len();
    let hasher = ahash::RandomState::new();
    let mut table = hashbrown::HashTable::<u32>::with_capacity(rows.min(DISTINCT_PROBE_ROWS));
    let mut distinct: Vec<&str> = Vec::new();
    let mut keys: Vec<i32> = Vec::with_capacity(rows);
    for row in 0..rows {
        if row == DISTINCT_PROBE_ROWS && distinct.len() * 4 > row * 3 {
            return None;
        }
        if array.is_null(row) {
            keys.push(0);
            continue;
        }
        let value = array.value(row);
        let hash = hasher.hash_one(value);
        let index = match table.find(hash, |&index| distinct[index as usize] == value) {
            Some(&index) => index,
            None => {
                let index = distinct.len() as u32;
                table.insert_unique(hash, index, |&index| {
                    hasher.hash_one(distinct[index as usize])
                });
                distinct.push(value);
                index
            }
        };
        keys.push(index as i32);
    }
    let keys = Int32Array::new(keys.into(), array.nulls().cloned());
    Some((StringArray::from_iter_values(distinct), keys))
}

/// A dictionary column as its plain values; every other array unchanged.
fn decode_dictionary(array: &ArrayRef) -> Result<ArrayRef> {
    match array.data_type() {
        DataType::Dictionary(_, values) => Ok(compute::cast(array, values)?),
        _ => Ok(Arc::clone(array)),
    }
}

fn literal_to_array(value: &ScalarValue, len: usize) -> Result<ArrayRef> {
    match value {
        ScalarValue::Null => Ok(arrow::array::new_null_array(&DataType::Null, len)),
        ScalarValue::Bool(v) => Ok(Arc::new(BooleanArray::from(vec![*v; len]))),
        ScalarValue::Int64(v) => Ok(Arc::new(Int64Array::from(vec![*v; len]))),
        ScalarValue::Float64(v) => Ok(Arc::new(Float64Array::from(vec![*v; len]))),
        ScalarValue::Utf8(v) => {
            let bytes = (v.len() as u64)
                .checked_mul(len as u64)
                .ok_or_else(|| KaveonError::Execution("string literal size overflow".into()))?;
            reserve_string_expansion(bytes, len)?;
            Ok(Arc::new(StringArray::from(vec![v.as_str(); len])))
        }
        ScalarValue::Decimal128 {
            value,
            precision,
            scale,
        } => Ok(Arc::new(
            Decimal128Array::from(vec![*value; len])
                .with_precision_and_scale(*precision, *scale)?,
        )),
    }
}

fn primitive_arrays_overlap<T>(left: &PrimitiveArray<T>, right: &PrimitiveArray<T>) -> Option<bool>
where
    T: ArrowPrimitiveType,
    T::Native: PartialEq,
{
    let saw_null = left.null_count() > 0 || right.null_count() > 0;
    for left_index in 0..left.len() {
        if left.is_null(left_index) {
            continue;
        }
        for right_index in 0..right.len() {
            if right.is_null(right_index) {
            } else if left.value(left_index) == right.value(right_index) {
                return Some(true);
            }
        }
    }
    if saw_null { None } else { Some(false) }
}

fn string_arrays_overlap(left: &arrow::array::StringArray, right: &arrow::array::StringArray) -> Option<bool> {
    let saw_null = left.null_count() > 0 || right.null_count() > 0;
    for left_index in 0..left.len() {
        if left.is_null(left_index) {
            continue;
        }
        for right_index in 0..right.len() {
            if right.is_null(right_index) {
            } else if left.value(left_index) == right.value(right_index) {
                return Some(true);
            }
        }
    }
    if saw_null { None } else { Some(false) }
}

// ── Binary operations ───────────────────────────────────────────────────────

fn eval_binary_op(left: &ArrayRef, op: BinaryOp, right: &ArrayRef) -> Result<ArrayRef> {
    if op == BinaryOp::StringConcat {
        return eval_string_concat(left, right);
    }
    match op {
        BinaryOp::Eq => Ok(Arc::new(comparison(left, right, CompareKind::Eq)?)),
        BinaryOp::Ne => Ok(Arc::new(comparison(left, right, CompareKind::Ne)?)),
        BinaryOp::Lt => Ok(Arc::new(comparison(left, right, CompareKind::Lt)?)),
        BinaryOp::Le => Ok(Arc::new(comparison(left, right, CompareKind::Le)?)),
        BinaryOp::Gt => Ok(Arc::new(comparison(left, right, CompareKind::Gt)?)),
        BinaryOp::Ge => Ok(Arc::new(comparison(left, right, CompareKind::Ge)?)),
        BinaryOp::Plus
        | BinaryOp::Minus
        | BinaryOp::Multiply
        | BinaryOp::Divide
        | BinaryOp::Modulo => arithmetic(left, op, right),
        BinaryOp::StringConcat => unreachable!(),
    }
}

fn eval_string_concat(left: &ArrayRef, right: &ArrayRef) -> Result<ArrayRef> {
    let left_str = cast_to_string(left)?;
    let right_str = cast_to_string(right)?;
    reserve_string_expansion(
        (left_str.value_data().len() as u64).saturating_add(right_str.value_data().len() as u64),
        left_str.len(),
    )?;
    let result: StringArray = (0..left_str.len())
        .map(|i| match (left_str.is_null(i), right_str.is_null(i)) {
            (true, _) | (_, true) => None,
            _ => Some(format!("{}{}", left_str.value(i), right_str.value(i))),
        })
        .collect();
    Ok(Arc::new(result))
}

fn cast_to_string(arr: &ArrayRef) -> Result<StringArray> {
    match arr.data_type() {
        DataType::Utf8 => Ok(arr
            .as_any()
            .downcast_ref::<StringArray>()
            .expect("type checked")
            .clone()),
        _ => {
            let casted = compute::cast(arr, &DataType::Utf8)?;
            Ok(casted
                .as_any()
                .downcast_ref::<StringArray>()
                .expect("cast to Utf8")
                .clone())
        }
    }
}

#[derive(Clone, Copy)]
enum CompareKind {
    Eq,
    Ne,
    Lt,
    Le,
    Gt,
    Ge,
}

fn comparison(left: &ArrayRef, right: &ArrayRef, kind: CompareKind) -> Result<BooleanArray> {
    use arrow::compute::kernels::cmp::{eq, gt, gt_eq, lt, lt_eq, neq};
    let left_list_field = match left.data_type() {
        DataType::List(field) | DataType::LargeList(field) => Some(field),
        _ => None,
    };
    let right_list_field = match right.data_type() {
        DataType::List(field) | DataType::LargeList(field) => Some(field),
        _ => None,
    };
    if matches!(left.data_type(), DataType::LargeList(_))
        || matches!(right.data_type(), DataType::LargeList(_))
    {
        let (Some(left_field), Some(right_field)) = (left_list_field, right_list_field) else {
            unreachable!("large-list comparison branch has list operands");
        };
        if left_field.data_type() != right_field.data_type() {
            return Err(KaveonError::Execution(
                "ARRAY values must have matching element types for comparison".into(),
            ));
        }
        let list_type = DataType::List(left_field.clone());
        let left = compute::cast(left, &list_type)?;
        let right = compute::cast(right, &list_type)?;
        return comparison(&left, &right, kind);
    }
    if let (Some(left), Some(right)) = (
        left.as_any().downcast_ref::<ListArray>(),
        right.as_any().downcast_ref::<ListArray>(),
    ) {
        if left.len() != right.len() {
            return Err(KaveonError::Execution(
                "ARRAY values must have matching row counts for comparison".into(),
            ));
        }
        let mut rows = Vec::with_capacity(left.len());
        for row in 0..left.len() {
            if left.is_null(row) || right.is_null(row) {
                rows.push(None);
                continue;
            }
            let left_values = left.value(row);
            let right_values = right.value(row);
            if left_values.len() != right_values.len()
                && matches!(kind, CompareKind::Eq | CompareKind::Ne)
            {
                rows.push(Some(matches!(kind, CompareKind::Ne)));
                continue;
            }
            let mut equal = Some(true);
            if matches!(
                kind,
                CompareKind::Lt | CompareKind::Le | CompareKind::Gt | CompareKind::Ge
            ) {
                let mut ordered = None;
                let mut decided = false;
                for index in 0..left_values.len().min(right_values.len()) {
                    let left_value = left_values.slice(index, 1);
                    let right_value = right_values.slice(index, 1);
                    let value_equal = comparison(&left_value, &right_value, CompareKind::Eq)?;
                    if value_equal.is_null(0) {
                        ordered = None;
                        decided = true;
                        break;
                    }
                    if value_equal.value(0) {
                        continue;
                    }
                    ordered = comparison(&left_value, &right_value, kind)?
                        .iter()
                        .next()
                        .flatten();
                    decided = true;
                    break;
                }
                if !decided {
                    ordered = Some(match kind {
                        CompareKind::Lt | CompareKind::Le => {
                            left_values.len() < right_values.len()
                        }
                        CompareKind::Gt | CompareKind::Ge => {
                            left_values.len() > right_values.len()
                        }
                        CompareKind::Eq | CompareKind::Ne => unreachable!(),
                    });
                }
                rows.push(ordered);
                continue;
            }
            for index in 0..left_values.len() {
                let value_equal = comparison(
                    &left_values.slice(index, 1),
                    &right_values.slice(index, 1),
                    CompareKind::Eq,
                )?;
                if value_equal.is_null(0) {
                    if equal != Some(false) {
                        equal = None;
                    }
                } else if !value_equal.value(0) {
                    equal = Some(false);
                    break;
                }
            }
            rows.push(if matches!(kind, CompareKind::Ne) {
                equal.map(|value| !value)
            } else {
                equal
            });
        }
        return Ok(BooleanArray::from(rows));
    }
    if let (Some(left), Some(right)) = (
        left.as_any().downcast_ref::<MapArray>(),
        right.as_any().downcast_ref::<MapArray>(),
    ) {
        if !matches!(kind, CompareKind::Eq | CompareKind::Ne) {
            return Err(KaveonError::Execution(
                "ordered comparison is not supported for MAP values".into(),
            ));
        }
        if left.len() != right.len() {
            return Err(KaveonError::Execution(
                "MAP values must have matching row counts for comparison".into(),
            ));
        }
        let mut rows = Vec::with_capacity(left.len());
        for row in 0..left.len() {
            if left.is_null(row) || right.is_null(row) {
                rows.push(None);
                continue;
            }
            let left_start = left.value_offsets()[row] as usize;
            let left_end = left.value_offsets()[row + 1] as usize;
            let right_start = right.value_offsets()[row] as usize;
            let right_end = right.value_offsets()[row + 1] as usize;
            if left_end - left_start != right_end - right_start {
                rows.push(Some(matches!(kind, CompareKind::Ne)));
                continue;
            }
            let mut equal = Some(true);
            for left_index in left_start..left_end {
                let left_key = compute::take(
                    left.keys(),
                    &Int64Array::from(vec![left_index as i64]),
                    None,
                )?;
                let mut matched = None;
                for right_index in right_start..right_end {
                    let right_key = compute::take(
                        right.keys(),
                        &Int64Array::from(vec![right_index as i64]),
                        None,
                    )?;
                    let key_equal = comparison(&left_key, &right_key, CompareKind::Eq)?;
                    if key_equal.is_valid(0) && key_equal.value(0) {
                        matched = Some(right_index);
                        break;
                    }
                }
                let Some(right_index) = matched else {
                    equal = Some(false);
                    break;
                };
                let left_value = compute::take(
                    left.values(),
                    &Int64Array::from(vec![left_index as i64]),
                    None,
                )?;
                let right_value = compute::take(
                    right.values(),
                    &Int64Array::from(vec![right_index as i64]),
                    None,
                )?;
                let value_equal = comparison(&left_value, &right_value, CompareKind::Eq)?;
                if value_equal.is_null(0) {
                    if equal != Some(false) {
                        equal = None;
                    }
                } else if !value_equal.value(0) {
                    equal = Some(false);
                    break;
                }
            }
            rows.push(if matches!(kind, CompareKind::Ne) {
                equal.map(|value| !value)
            } else {
                equal
            });
        }
        return Ok(BooleanArray::from(rows));
    }
    if let (Some(left), Some(right)) = (
        left.as_any().downcast_ref::<StructArray>(),
        right.as_any().downcast_ref::<StructArray>(),
    ) {
        if left.num_columns() != right.num_columns() || left.column_names() != right.column_names()
        {
            return Err(KaveonError::Execution(
                "ROW values must have matching fields for comparison".into(),
            ));
        }
        if matches!(kind, CompareKind::Lt | CompareKind::Le | CompareKind::Gt | CompareKind::Ge) {
            let mut rows = Vec::with_capacity(left.len());
            for row in 0..left.len() {
                let mut result = Some(false);
                for (left_field, right_field) in left.columns().iter().zip(right.columns()) {
                    let left_value = left_field.slice(row, 1);
                    let right_value = right_field.slice(row, 1);
                    let equal = comparison(&left_value, &right_value, CompareKind::Eq)?;
                    if equal.is_null(0) {
                        result = None;
                        break;
                    }
                    if equal.value(0) {
                        continue;
                    }
                    let ordered = comparison(&left_value, &right_value, kind)?;
                    result = ordered.iter().next().flatten();
                    break;
                }
                rows.push(result);
            }
            return Ok(BooleanArray::from(rows));
        }
        let mut result = BooleanArray::from(vec![true; left.len()]);
        for (left_field, right_field) in left.columns().iter().zip(right.columns()) {
            let field_equal = comparison(left_field, right_field, CompareKind::Eq)?;
            result = compute::and_kleene(&result, &field_equal)?;
        }
        return if matches!(kind, CompareKind::Ne) {
            Ok(compute::not(&result)?)
        } else {
            Ok(result)
        };
    }
    let (left, right) = coerce_numeric_pair(left, right)?;
    let result = match kind {
        CompareKind::Eq => eq(&left, &right)?,
        CompareKind::Ne => neq(&left, &right)?,
        CompareKind::Lt => lt(&left, &right)?,
        CompareKind::Le => lt_eq(&left, &right)?,
        CompareKind::Gt => gt(&left, &right)?,
        CompareKind::Ge => gt_eq(&left, &right)?,
    };
    Ok(result)
}

/// `expression op literal` (or the reverse) for the five arithmetic
/// operators over integer and float inputs, with the literal as an Arrow
/// scalar. None when the shape is anything else.
fn arithmetic_with_literal(
    left: &Expr,
    op: BinaryOp,
    right: &Expr,
    batch: &RecordBatch,
) -> Result<Option<ArrayRef>> {
    use arrow::array::{Datum, Scalar};
    if !matches!(
        op,
        BinaryOp::Plus | BinaryOp::Minus | BinaryOp::Multiply | BinaryOp::Divide | BinaryOp::Modulo
    ) {
        return Ok(None);
    }
    let (expression, literal, literal_on_left) = match (left, right) {
        (expression, Expr::Literal(literal)) => (expression, literal, false),
        (Expr::Literal(literal), expression) => (expression, literal, true),
        _ => return Ok(None),
    };
    let literal_type = match literal {
        ScalarValue::Int64(_) => DataType::Int64,
        ScalarValue::Float64(_) => DataType::Float64,
        _ => return Ok(None),
    };
    let array = decode_dictionary(&evaluate(expression, batch)?)?;
    let integer_like =
        |data_type: &DataType| is_integer(data_type) || matches!(data_type, DataType::Date32);
    // The same meeting types as coerce_numeric_pair: integers with an
    // integer literal meet as Int64, anything else numeric as Float64.
    let meet = if integer_like(array.data_type()) && literal_type == DataType::Int64 {
        DataType::Int64
    } else if is_numeric(array.data_type()) || integer_like(array.data_type()) {
        DataType::Float64
    } else {
        return Ok(None);
    };
    let array = if array.data_type() == &meet {
        array
    } else {
        let array = match array.data_type() {
            DataType::Date32 => compute::cast(&array, &DataType::Int32)?,
            _ => array,
        };
        compute::cast(&array, &meet)?
    };
    let scalar: ArrayRef = match (literal, &meet) {
        (ScalarValue::Int64(v), DataType::Int64) => Arc::new(Int64Array::from(vec![*v])),
        (ScalarValue::Int64(v), _) => Arc::new(Float64Array::from(vec![*v as f64])),
        (ScalarValue::Float64(v), _) => Arc::new(Float64Array::from(vec![*v])),
        _ => return Ok(None),
    };
    let scalar = Scalar::new(scalar);
    let (l, r): (&dyn Datum, &dyn Datum) = if literal_on_left {
        (&scalar, &array)
    } else {
        (&array, &scalar)
    };
    let result = match op {
        BinaryOp::Plus => compute::kernels::numeric::add(l, r)?,
        BinaryOp::Minus => compute::kernels::numeric::sub(l, r)?,
        BinaryOp::Multiply => compute::kernels::numeric::mul(l, r)?,
        BinaryOp::Divide => compute::kernels::numeric::div(l, r)?,
        BinaryOp::Modulo => compute::kernels::numeric::rem(l, r)?,
        _ => unreachable!(),
    };
    Ok(Some(result))
}

fn arithmetic(left: &ArrayRef, op: BinaryOp, right: &ArrayRef) -> Result<ArrayRef> {
    let (left, right) = coerce_numeric_pair(left, right)?;
    let result: ArrayRef = match (left.data_type(), right.data_type()) {
        (DataType::Int64, DataType::Int64) => {
            let l = left.as_primitive::<Int64Type>();
            let r = right.as_primitive::<Int64Type>();
            match op {
                BinaryOp::Plus => Arc::new(compute::kernels::numeric::add(l, r)?),
                BinaryOp::Minus => Arc::new(compute::kernels::numeric::sub(l, r)?),
                BinaryOp::Multiply => Arc::new(compute::kernels::numeric::mul(l, r)?),
                BinaryOp::Divide => Arc::new(compute::kernels::numeric::div(l, r)?),
                BinaryOp::Modulo => Arc::new(compute::kernels::numeric::rem(l, r)?),
                _ => unreachable!(),
            }
        }
        (DataType::Float64, DataType::Float64) => {
            let l = left.as_primitive::<Float64Type>();
            let r = right.as_primitive::<Float64Type>();
            match op {
                BinaryOp::Plus => Arc::new(compute::kernels::numeric::add(l, r)?),
                BinaryOp::Minus => Arc::new(compute::kernels::numeric::sub(l, r)?),
                BinaryOp::Multiply => Arc::new(compute::kernels::numeric::mul(l, r)?),
                BinaryOp::Divide => Arc::new(compute::kernels::numeric::div(l, r)?),
                BinaryOp::Modulo => Arc::new(compute::kernels::numeric::rem(l, r)?),
                _ => unreachable!(),
            }
        }
        // Decimals stay exact: `0.06 - 0.01` is the decimal 0.05, with the
        // precision and scale the kernels derive for the operation.
        (DataType::Decimal128(_, _), DataType::Decimal128(_, _)) => match op {
            BinaryOp::Plus => compute::kernels::numeric::add(&left, &right)?,
            BinaryOp::Minus => compute::kernels::numeric::sub(&left, &right)?,
            BinaryOp::Multiply => compute::kernels::numeric::mul(&left, &right)?,
            BinaryOp::Divide => compute::kernels::numeric::div(&left, &right)?,
            BinaryOp::Modulo => compute::kernels::numeric::rem(&left, &right)?,
            _ => unreachable!(),
        },
        (l, r) => {
            return Err(KaveonError::Execution(format!(
                "arithmetic not supported between {l} and {r}"
            )));
        }
    };
    Ok(result)
}

fn coerce_numeric_pair(left: &ArrayRef, right: &ArrayRef) -> Result<(ArrayRef, ArrayRef)> {
    if left.data_type() == &DataType::Null {
        return Ok((
            arrow::array::new_null_array(right.data_type(), left.len()),
            right.clone(),
        ));
    }
    if right.data_type() == &DataType::Null {
        return Ok((
            left.clone(),
            arrow::array::new_null_array(left.data_type(), right.len()),
        ));
    }
    if left.data_type() == right.data_type() {
        return Ok((Arc::clone(left), Arc::clone(right)));
    }
    // Text against a day-number date is read as a date, the way SQL
    // coerces `date_col BETWEEN '2026-07-10' AND '2026-07-12'`; text that
    // is not a date compares as null.
    match (left.data_type(), right.data_type()) {
        (DataType::Date32, DataType::Utf8 | DataType::LargeUtf8) => {
            return Ok((Arc::clone(left), compute::cast(right, &DataType::Date32)?));
        }
        (DataType::Utf8 | DataType::LargeUtf8, DataType::Date32) => {
            return Ok((compute::cast(left, &DataType::Date32)?, Arc::clone(right)));
        }
        _ => {}
    }
    // Integers of different widths, and a day-number date against an
    // integer, meet as Int64; anything else numeric meets as Float64.
    let integer_like =
        |data_type: &DataType| is_integer(data_type) || matches!(data_type, DataType::Date32);
    if integer_like(left.data_type()) && integer_like(right.data_type()) {
        let as_int64 = |array: &ArrayRef| -> Result<ArrayRef> {
            let array = match array.data_type() {
                DataType::Date32 => compute::cast(array, &DataType::Int32)?,
                _ => Arc::clone(array),
            };
            Ok(compute::cast(&array, &DataType::Int64)?)
        };
        return Ok((as_int64(left)?, as_int64(right)?));
    }
    if is_numeric(left.data_type()) && is_numeric(right.data_type()) {
        return Ok((
            compute::cast(left, &DataType::Float64)?,
            compute::cast(right, &DataType::Float64)?,
        ));
    }
    Ok((Arc::clone(left), Arc::clone(right)))
}

fn is_integer(data_type: &DataType) -> bool {
    matches!(
        data_type,
        DataType::Int8
            | DataType::Int16
            | DataType::Int32
            | DataType::Int64
            | DataType::UInt8
            | DataType::UInt16
            | DataType::UInt32
            | DataType::UInt64
    )
}

fn is_numeric(data_type: &DataType) -> bool {
    matches!(
        data_type,
        DataType::Int8
            | DataType::Int16
            | DataType::Int32
            | DataType::Int64
            | DataType::UInt8
            | DataType::UInt16
            | DataType::UInt32
            | DataType::UInt64
            | DataType::Float16
            | DataType::Float32
            | DataType::Float64
            | DataType::Decimal128(_, _)
    )
}

fn as_boolean(arr: &ArrayRef) -> Result<&BooleanArray> {
    arr.as_any()
        .downcast_ref::<BooleanArray>()
        .ok_or_else(|| KaveonError::Execution("expected boolean array".into()))
}

fn as_string_array(arr: &ArrayRef) -> Result<&StringArray> {
    arr.as_any()
        .downcast_ref::<StringArray>()
        .ok_or_else(|| KaveonError::Execution("expected string array".into()))
}

// ── CASE WHEN ───────────────────────────────────────────────────────────────

fn eval_case(
    operand: Option<&Expr>,
    when_then: &[(Expr, Expr)],
    else_expr: Option<&Expr>,
    batch: &RecordBatch,
) -> Result<ArrayRef> {
    let num_rows = batch.num_rows();
    let operand_arr = operand.map(|e| evaluate(e, batch)).transpose()?;

    let mut conditions: Vec<BooleanArray> = Vec::with_capacity(when_then.len());
    let mut results: Vec<ArrayRef> = Vec::with_capacity(when_then.len());

    for (when_expr, then_expr) in when_then {
        let cond = if let Some(ref op_arr) = operand_arr {
            let when_arr = evaluate(when_expr, batch)?;
            comparison(op_arr, &when_arr, CompareKind::Eq)?
        } else {
            let when_arr = evaluate(when_expr, batch)?;
            as_boolean(&when_arr)?.clone()
        };
        conditions.push(cond);
        results.push(evaluate(then_expr, batch)?);
    }

    let else_arr = match else_expr {
        Some(e) => evaluate(e, batch)?,
        None => Arc::new(BooleanArray::new_null(num_rows)) as ArrayRef,
    };

    let target_type = results
        .first()
        .map(|a| a.data_type().clone())
        .unwrap_or_else(|| else_arr.data_type().clone());
    let target_type = match target_type {
        DataType::Dictionary(_, values) => *values,
        other => other,
    };

    let mut output = compute::cast(&else_arr, &target_type)?;
    for (cond, result) in conditions.iter().zip(results.iter()).rev() {
        let result = compute::cast(result, &target_type)?;
        output = zip_arrays(cond, &result, &output)?;
    }
    Ok(output)
}

fn zip_arrays(
    mask: &BooleanArray,
    true_vals: &ArrayRef,
    false_vals: &ArrayRef,
) -> Result<ArrayRef> {
    let len = mask.len();
    match true_vals.data_type() {
        DataType::Int64 => {
            let t = true_vals.as_primitive::<Int64Type>();
            let f = false_vals.as_primitive::<Int64Type>();
            let result: Int64Array = (0..len)
                .map(|i| {
                    if !mask.is_null(i) && mask.value(i) {
                        if t.is_null(i) { None } else { Some(t.value(i)) }
                    } else if f.is_null(i) {
                        None
                    } else {
                        Some(f.value(i))
                    }
                })
                .collect();
            Ok(Arc::new(result))
        }
        DataType::UInt64 => {
            let t = true_vals.as_primitive::<UInt64Type>();
            let f = false_vals.as_primitive::<UInt64Type>();
            let result: UInt64Array = (0..len)
                .map(|i| {
                    if !mask.is_null(i) && mask.value(i) {
                        if t.is_null(i) { None } else { Some(t.value(i)) }
                    } else if f.is_null(i) {
                        None
                    } else {
                        Some(f.value(i))
                    }
                })
                .collect();
            Ok(Arc::new(result))
        }
        DataType::Float64 => {
            let t = true_vals.as_primitive::<Float64Type>();
            let f = false_vals.as_primitive::<Float64Type>();
            let result: Float64Array = (0..len)
                .map(|i| {
                    if !mask.is_null(i) && mask.value(i) {
                        if t.is_null(i) { None } else { Some(t.value(i)) }
                    } else if f.is_null(i) {
                        None
                    } else {
                        Some(f.value(i))
                    }
                })
                .collect();
            Ok(Arc::new(result))
        }
        DataType::Int32 => {
            let t = true_vals.as_primitive::<Int32Type>();
            let f = false_vals.as_primitive::<Int32Type>();
            let result: Int32Array = (0..len)
                .map(|i| {
                    if !mask.is_null(i) && mask.value(i) {
                        if t.is_null(i) { None } else { Some(t.value(i)) }
                    } else if f.is_null(i) {
                        None
                    } else {
                        Some(f.value(i))
                    }
                })
                .collect();
            Ok(Arc::new(result))
        }
        DataType::Boolean => {
            let t = as_boolean(true_vals)?;
            let f = as_boolean(false_vals)?;
            let result: BooleanArray = (0..len)
                .map(|i| {
                    if !mask.is_null(i) && mask.value(i) {
                        if t.is_null(i) { None } else { Some(t.value(i)) }
                    } else if f.is_null(i) {
                        None
                    } else {
                        Some(f.value(i))
                    }
                })
                .collect();
            Ok(Arc::new(result))
        }
        DataType::Utf8 => {
            let t = as_string_array(true_vals)?;
            let f = as_string_array(false_vals)?;
            let result: StringArray = (0..len)
                .map(|i| {
                    if !mask.is_null(i) && mask.value(i) {
                        if t.is_null(i) { None } else { Some(t.value(i)) }
                    } else if f.is_null(i) {
                        None
                    } else {
                        Some(f.value(i))
                    }
                })
                .collect();
            Ok(Arc::new(result))
        }
        dt => Err(KaveonError::Execution(format!(
            "CASE not supported for type {dt}"
        ))),
    }
}

// ── LIKE ────────────────────────────────────────────────────────────────────

fn eval_like(
    expr: &Expr,
    pattern: &Expr,
    negated: bool,
    case_insensitive: bool,
    batch: &RecordBatch,
) -> Result<ArrayRef> {
    let values = evaluate(expr, batch)?;
    if let (DataType::Dictionary(key, _), Expr::Literal(literal)) = (values.data_type(), pattern)
        && key.as_ref() == &DataType::Int32
    {
        // Match the dictionary values the batch uses once and gather by key.
        let dictionary = values.as_dictionary::<Int32Type>();
        let (values, keys) = used_dictionary_values(
            dictionary,
            compact_dictionary_above(dictionary.len(), ValueCost::StringFunction),
        )?;
        let patterns = literal_to_array(literal, values.len())?;
        let verdicts = like_arrays(
            as_string_array(&values)?,
            as_string_array(&patterns)?,
            negated,
            case_insensitive,
        )?;
        return Ok(compute::take(&verdicts, &keys, None)?);
    }
    let values = decode_dictionary(&values)?;
    let patterns = evaluate(pattern, batch)?;
    let values = as_string_array(&values)?;
    let patterns = as_string_array(&patterns)?;
    Ok(Arc::new(like_arrays(
        values,
        patterns,
        negated,
        case_insensitive,
    )?))
}

fn like_arrays(
    values: &StringArray,
    patterns: &StringArray,
    negated: bool,
    case_insensitive: bool,
) -> Result<BooleanArray> {
    use arrow::compute::kernels::comparison::{ilike, like, nilike, nlike};
    // Arrow's kernel recognises the common shapes — `%needle%`, `needle%`,
    // `%needle`, no wildcard — and runs them as byte searches; a pattern
    // given once (a literal) is a scalar so it is classified once.
    let first = (!patterns.is_empty() && !patterns.is_null(0)).then(|| patterns.value(0));
    let scalar_pattern = (patterns.len() == 1
        || (first.is_some() && patterns.iter().all(|p| p == first)))
    .then(|| arrow::array::Scalar::new(patterns.slice(0, 1)));
    let matched = match (&scalar_pattern, negated, case_insensitive) {
        (Some(pattern), false, false) => like(values, pattern)?,
        (Some(pattern), true, false) => nlike(values, pattern)?,
        (Some(pattern), false, true) => ilike(values, pattern)?,
        (Some(pattern), true, true) => nilike(values, pattern)?,
        (None, false, false) => like(values, patterns)?,
        (None, true, false) => nlike(values, patterns)?,
        (None, false, true) => ilike(values, patterns)?,
        (None, true, true) => nilike(values, patterns)?,
    };
    Ok(matched)
}

// ── BETWEEN ─────────────────────────────────────────────────────────────────

fn eval_between(
    expr: &Expr,
    low: &Expr,
    high: &Expr,
    negated: bool,
    batch: &RecordBatch,
) -> Result<ArrayRef> {
    let val = evaluate(expr, batch)?;
    let lo = evaluate(low, batch)?;
    let hi = evaluate(high, batch)?;
    let (val_lo, lo_c) = coerce_numeric_pair(&val, &lo)?;
    let (val_hi, hi_c) = coerce_numeric_pair(&val, &hi)?;
    let ge_low = comparison(&val_lo, &lo_c, CompareKind::Ge)?;
    let le_high = comparison(&val_hi, &hi_c, CompareKind::Le)?;
    let result = compute::and(&ge_low, &le_high)?;
    if negated {
        Ok(Arc::new(compute::not(&result)?))
    } else {
        Ok(Arc::new(result))
    }
}

// ── IN list ─────────────────────────────────────────────────────────────────

fn eval_in_list(
    expr: &Expr,
    list: &[Expr],
    negated: bool,
    batch: &RecordBatch,
) -> Result<ArrayRef> {
    let val = evaluate(expr, batch)?;
    let mut result = BooleanArray::from(vec![false; batch.num_rows()]);
    for item in list {
        let item_arr = evaluate(item, batch)?;
        let (val_c, item_c) = coerce_numeric_pair(&val, &item_arr)?;
        let eq = comparison(&val_c, &item_c, CompareKind::Eq)?;
        result = compute::or_kleene(&result, &eq)?;
    }
    if negated {
        Ok(Arc::new(compute::not(&result)?))
    } else {
        Ok(Arc::new(result))
    }
}

// ── CAST ────────────────────────────────────────────────────────────────────

fn eval_cast(expr: &Expr, target: CastTarget, batch: &RecordBatch) -> Result<ArrayRef> {
    let arr = evaluate(expr, batch)?;
    if matches!(target, CastTarget::TimestampMicros { with_timezone: true }) {
        // Arrow's string parser requires the optional chrono-tz feature for
        // named zones such as UTC. Parse the instant as a naive timestamp,
        // then attach the Engine's canonical UTC metadata without changing
        // the microsecond value.
        let naive = compute::cast(
            &arr,
            &DataType::Timestamp(TimeUnit::Microsecond, None),
        )?;
        let values = naive
            .as_any()
            .downcast_ref::<TimestampMicrosecondArray>()
            .ok_or_else(|| KaveonError::Execution("timestamp cast produced an invalid array".into()))?
            .clone()
            .with_timezone("UTC");
        return Ok(Arc::new(values));
    }
    let arrow_type = target.to_arrow_type();
    Ok(compute::cast(&arr, &arrow_type)?)
}

// ── Scalar functions ────────────────────────────────────────────────────────

/// Evaluate Trino's quantified array comparisons (`x < ANY (array)` and
/// `x >= ALL (array)`).  The reduction is deliberately done after the normal
/// vectorized comparison kernel so every supported Arrow scalar type keeps its
/// existing coercion and NULL behavior.  ANY is true on one true element and
/// otherwise NULL when a NULL element was observed; ALL is false on one false
/// element and otherwise NULL when a NULL element was observed.  Empty arrays
/// therefore yield false for ANY and true for ALL, as in Trino.
fn eval_array_quantified_compare(
    name: &str,
    args: &[ArrayRef],
    num_rows: usize,
) -> Result<ArrayRef> {
    check_arity(name, args, 2)?;
    let (is_any, kind) = if let Some(suffix) = name.strip_prefix("ARRAY_ANY_") {
        (true, quantified_compare_kind(suffix)?)
    } else if let Some(suffix) = name.strip_prefix("ARRAY_ALL_") {
        (false, quantified_compare_kind(suffix)?)
    } else {
        return Err(KaveonError::Execution(format!(
            "unknown quantified array comparison: {name}"
        )));
    };

    let mut result = BooleanBuilder::with_capacity(num_rows);
    for row in 0..num_rows {
        let inner = match args[0].data_type() {
            DataType::List(_) => args[0]
                .as_any()
                .downcast_ref::<ListArray>()
                .expect("List data type must be ListArray")
                .value(row),
            DataType::LargeList(_) => args[0]
                .as_any()
                .downcast_ref::<LargeListArray>()
                .expect("LargeList data type must be LargeListArray")
                .value(row),
            other => {
                return Err(KaveonError::Execution(format!(
                    "{name} requires an ARRAY/List value, found {other}"
                )))
            }
        };
        let list_is_null = match args[0].data_type() {
            DataType::List(_) => args[0]
                .as_any()
                .downcast_ref::<ListArray>()
                .expect("List data type must be ListArray")
                .is_null(row),
            DataType::LargeList(_) => args[0]
                .as_any()
                .downcast_ref::<LargeListArray>()
                .expect("LargeList data type must be LargeListArray")
                .is_null(row),
            _ => false,
        };
        if list_is_null {
            result.append_null();
            continue;
        }
        if inner.is_empty() {
            result.append_value(!is_any);
            continue;
        }

        let indices = UInt32Array::from(vec![0; inner.len()]);
        let scalar = compute::take(&args[1].slice(row, 1), &indices, None)?;
        let comparisons = comparison(&inner, &scalar, kind)?;
        let mut saw_null = false;
        let reduced = if is_any {
            let mut matched = false;
            for index in 0..comparisons.len() {
                if comparisons.is_null(index) {
                    saw_null = true;
                } else if comparisons.value(index) {
                    matched = true;
                    break;
                }
            }
            if matched { Some(true) } else if saw_null { None } else { Some(false) }
        } else {
            let mut failed = false;
            for index in 0..comparisons.len() {
                if comparisons.is_null(index) {
                    saw_null = true;
                } else if !comparisons.value(index) {
                    failed = true;
                    break;
                }
            }
            if failed { Some(false) } else if saw_null { None } else { Some(true) }
        };
        result.append_option(reduced);
    }
    Ok(Arc::new(result.finish()))
}

fn quantified_compare_kind(suffix: &str) -> Result<CompareKind> {
    match suffix {
        "EQ" => Ok(CompareKind::Eq),
        "NE" => Ok(CompareKind::Ne),
        "LT" => Ok(CompareKind::Lt),
        "LE" => Ok(CompareKind::Le),
        "GT" => Ok(CompareKind::Gt),
        "GE" => Ok(CompareKind::Ge),
        _ => Err(KaveonError::Execution(format!(
            "unsupported quantified comparison operator: {suffix}"
        ))),
    }
}

fn arrays_overlap_nested<L: OffsetSizeTrait>(
    left: &GenericListArray<L>,
    right: &GenericListArray<L>,
) -> Result<ArrayRef> {
    if left.len() != right.len() || left.value_type() != right.value_type() {
        return Err(KaveonError::Execution(
            "ARRAYS_OVERLAP requires arrays with matching row counts and element types".into(),
        ));
    }
    let rows = (0..left.len())
        .map(|row| {
            if left.is_null(row) || right.is_null(row) {
                return None;
            }
            let left_values = left.value(row);
            let right_values = right.value(row);
            let mut saw_null = false;
            for left_index in 0..left_values.len() {
                if left_values.is_null(left_index) {
                    saw_null = true;
                    continue;
                }
                for right_index in 0..right_values.len() {
                    if right_values.is_null(right_index) {
                        saw_null = true;
                        continue;
                    }
                    if comparison(
                        &left_values.slice(left_index, 1),
                        &right_values.slice(right_index, 1),
                        CompareKind::Eq,
                    )
                    .map(|equal| !equal.is_null(0) && equal.value(0))
                    .unwrap_or(false)
                    {
                        return Some(true);
                    }
                }
            }
            if saw_null { None } else { Some(false) }
        })
        .collect::<BooleanArray>();
    Ok(Arc::new(rows))
}

fn arrays_overlap_primitive<L, T>(
    left: &GenericListArray<L>,
    right: &GenericListArray<L>,
) -> Result<ArrayRef>
where
    L: OffsetSizeTrait,
    T: ArrowPrimitiveType,
    T::Native: PartialEq,
{
    if left.len() != right.len() || left.value_type() != right.value_type() {
        return Err(KaveonError::Execution(
            "ARRAYS_OVERLAP requires arrays with matching row counts and element types".into(),
        ));
    }
    let rows = (0..left.len())
        .map(|row| {
            if left.is_null(row) || right.is_null(row) {
                None
            } else {
                primitive_arrays_overlap(
                    left.value(row).as_primitive::<T>(),
                    right.value(row).as_primitive::<T>(),
                )
            }
        })
        .collect::<BooleanArray>();
    Ok(Arc::new(rows))
}

fn arrays_overlap_boolean<L: OffsetSizeTrait>(
    left: &GenericListArray<L>,
    right: &GenericListArray<L>,
) -> Result<ArrayRef> {
    if left.len() != right.len() || left.value_type() != right.value_type() {
        return Err(KaveonError::Execution(
            "ARRAYS_OVERLAP requires arrays with matching row counts and element types".into(),
        ));
    }
    let rows = (0..left.len())
        .map(|row| {
            if left.is_null(row) || right.is_null(row) {
                None
            } else {
                let left_values = left.value(row);
                let right_values = right.value(row);
                let saw_null = left_values.null_count() > 0 || right_values.null_count() > 0;
                for left_index in 0..left_values.len() {
                    if left_values.is_null(left_index) {
                        continue;
                    }
                    for right_index in 0..right_values.len() {
                        if !right_values.is_null(right_index)
                            && left_values.as_boolean().value(left_index)
                                == right_values.as_boolean().value(right_index)
                        {
                            return Some(true);
                        }
                    }
                }
                if saw_null { None } else { Some(false) }
            }
        })
        .collect::<BooleanArray>();
    Ok(Arc::new(rows))
}

fn arrays_overlap_utf8<L: OffsetSizeTrait>(
    left: &GenericListArray<L>,
    right: &GenericListArray<L>,
) -> Result<ArrayRef> {
    if left.len() != right.len() || left.value_type() != right.value_type() {
        return Err(KaveonError::Execution(
            "ARRAYS_OVERLAP requires arrays with matching row counts and element types".into(),
        ));
    }
    let rows = (0..left.len())
        .map(|row| {
            if left.is_null(row) || right.is_null(row) {
                None
            } else {
                string_arrays_overlap(
                    left.value(row).as_string::<i32>(),
                    right.value(row).as_string::<i32>(),
                )
            }
        })
        .collect::<BooleanArray>();
    Ok(Arc::new(rows))
}

fn array_position_nested<L: OffsetSizeTrait>(
    list: &GenericListArray<L>,
    needle: &ArrayRef,
) -> Result<ArrayRef> {
    let result = (0..list.len())
        .map(|row| {
            if list.is_null(row) {
                return None;
            }
            let values = list.value(row);
            let needle_is_null = needle.is_null(row);
            let needle = (!needle_is_null).then(|| needle.slice(row, 1));
            let position = (0..values.len()).find(|&index| {
                if values.is_null(index) {
                    return needle_is_null;
                }
                let Some(needle) = needle.as_ref() else {
                    return false;
                };
                comparison(&values.slice(index, 1), needle, CompareKind::Eq)
                    .map(|equal| !equal.is_null(0) && equal.value(0))
                    .unwrap_or(false)
            });
            Some(position.map(|index| index as i64 + 1).unwrap_or(0))
        })
        .collect::<Vec<_>>();
    Ok(Arc::new(Int64Array::from(result)))
}

fn contains_nested<L: OffsetSizeTrait>(
    list: &GenericListArray<L>,
    needle: &ArrayRef,
) -> Result<ArrayRef> {
    let result = (0..list.len())
        .map(|row| {
            if list.is_null(row) || needle.is_null(row) {
                return None;
            }
            let values = list.value(row);
            let needle = needle.slice(row, 1);
            let mut saw_null = false;
            for index in 0..values.len() {
                if values.is_null(index) {
                    saw_null = true;
                    continue;
                }
                if comparison(&values.slice(index, 1), &needle, CompareKind::Eq)
                    .map(|equal| !equal.is_null(0) && equal.value(0))
                    .unwrap_or(false)
                {
                    return Some(true);
                }
            }
            if saw_null { None } else { Some(false) }
        })
        .collect::<BooleanArray>();
    Ok(Arc::new(result))
}

fn contains_primitive<L, T>(
    list: &GenericListArray<L>,
    needle: &ArrayRef,
) -> Result<ArrayRef>
where
    L: OffsetSizeTrait,
    T: ArrowPrimitiveType,
    T::Native: PartialEq,
{
    let needle = compute::cast(needle, &list.value_type())?;
    let needle = needle.as_primitive::<T>();
    let result = (0..list.len())
        .map(|row| {
            if list.is_null(row) || needle.is_null(row) {
                return None;
            }
            let values = list.value(row);
            let values = values.as_primitive::<T>();
            let mut saw_null = false;
            for index in 0..values.len() {
                if values.is_null(index) {
                    saw_null = true;
                } else if values.value(index) == needle.value(row) {
                    return Some(true);
                }
            }
            if saw_null { None } else { Some(false) }
        })
        .collect::<BooleanArray>();
    Ok(Arc::new(result))
}

fn contains_boolean<L: OffsetSizeTrait>(
    list: &GenericListArray<L>,
    needle: &ArrayRef,
) -> Result<ArrayRef> {
    let needle = compute::cast(needle, &DataType::Boolean)?;
    let needle = needle.as_boolean();
    let result = (0..list.len())
        .map(|row| {
            if list.is_null(row) || needle.is_null(row) {
                return None;
            }
            let values = list.value(row).as_boolean().clone();
            let mut saw_null = false;
            for index in 0..values.len() {
                if values.is_null(index) {
                    saw_null = true;
                } else if values.value(index) == needle.value(row) {
                    return Some(true);
                }
            }
            if saw_null { None } else { Some(false) }
        })
        .collect::<BooleanArray>();
    Ok(Arc::new(result))
}

fn contains_utf8<L: OffsetSizeTrait>(
    list: &GenericListArray<L>,
    needle: &ArrayRef,
) -> Result<ArrayRef> {
    let needle = compute::cast(needle, &DataType::Utf8)?;
    let needle = needle.as_string::<i32>();
    let result = (0..list.len())
        .map(|row| {
            if list.is_null(row) || needle.is_null(row) {
                return None;
            }
            let values_array = list.value(row);
            let values = values_array.as_string::<i32>();
            let mut saw_null = false;
            for index in 0..values.len() {
                if values.is_null(index) {
                    saw_null = true;
                } else if values.value(index) == needle.value(row) {
                    return Some(true);
                }
            }
            if saw_null { None } else { Some(false) }
        })
        .collect::<BooleanArray>();
    Ok(Arc::new(result))
}

fn array_distinct_nested<L: OffsetSizeTrait>(
    list: &GenericListArray<L>,
) -> Result<ArrayRef> {
    let mut selected = Vec::<ArrayRef>::new();
    let mut offsets = vec![L::zero()];
    let mut valid = Vec::with_capacity(list.len());
    for row in 0..list.len() {
        if list.is_null(row) {
            valid.push(false);
            offsets.push(L::from_usize(selected.len()).ok_or_else(|| {
                KaveonError::Execution("ARRAY_DISTINCT result offset overflow".into())
            })?);
            continue;
        }
        valid.push(true);
        let inner = list.value(row);
        let row_start = selected.len();
        let mut saw_null = false;
        for index in 0..inner.len() {
            if inner.is_null(index) {
                if !saw_null {
                    selected.push(arrow::array::new_null_array(inner.data_type(), 1));
                    saw_null = true;
                }
                continue;
            }
            let candidate = inner.slice(index, 1);
            let duplicate = selected[row_start..].iter().any(|previous| {
                if previous.null_count() != 0 {
                    return false;
                }
                comparison(&candidate, previous, CompareKind::Eq)
                    .map(|equal| !equal.is_null(0) && equal.value(0))
                    .unwrap_or(false)
            });
            if !duplicate {
                selected.push(candidate);
            }
        }
        offsets.push(L::from_usize(selected.len()).ok_or_else(|| {
            KaveonError::Execution("ARRAY_DISTINCT result offset overflow".into())
        })?);
    }
    let values = if selected.is_empty() {
        arrow::array::new_empty_array(&list.value_type())
    } else {
        let chunks = selected
            .iter()
            .map(|value| value.as_ref() as &dyn Array)
            .collect::<Vec<_>>();
        compute::concat(&chunks)?
    };
    let field = Arc::new(arrow::datatypes::Field::new(
        "item",
        values.data_type().clone(),
        true,
    ));
    Ok(Arc::new(GenericListArray::<L>::try_new(
        field,
        OffsetBuffer::new(ScalarBuffer::from(offsets)),
        values,
        Some(arrow::buffer::NullBuffer::from(valid)),
    )?))
}

fn array_reverse_generic<L: OffsetSizeTrait>(
    list: &GenericListArray<L>,
) -> Result<ArrayRef> {
    let mut selected = Vec::<ArrayRef>::new();
    let mut offsets = vec![L::zero()];
    let mut valid = Vec::with_capacity(list.len());
    for row in 0..list.len() {
        if list.is_null(row) {
            valid.push(false);
            offsets.push(L::from_usize(selected.len()).ok_or_else(|| {
                KaveonError::Execution("ARRAY_REVERSE result offset overflow".into())
            })?);
            continue;
        }
        valid.push(true);
        let inner = list.value(row);
        for index in (0..inner.len()).rev() {
            selected.push(inner.slice(index, 1));
        }
        offsets.push(L::from_usize(selected.len()).ok_or_else(|| {
            KaveonError::Execution("ARRAY_REVERSE result offset overflow".into())
        })?);
    }
    let values = if selected.is_empty() {
        arrow::array::new_empty_array(&list.value_type())
    } else {
        let chunks = selected
            .iter()
            .map(|value| value.as_ref() as &dyn Array)
            .collect::<Vec<_>>();
        compute::concat(&chunks)?
    };
    let field = Arc::new(arrow::datatypes::Field::new(
        "item",
        values.data_type().clone(),
        true,
    ));
    Ok(Arc::new(GenericListArray::<L>::try_new(
        field,
        OffsetBuffer::new(ScalarBuffer::from(offsets)),
        values,
        Some(arrow::buffer::NullBuffer::from(valid)),
    )?))
}

fn array_sort_generic<L: OffsetSizeTrait>(list: &GenericListArray<L>) -> Result<ArrayRef> {
    let supported = matches!(
        list.value_type(),
        DataType::Int32
            | DataType::Int64
            | DataType::Float64
            | DataType::Date32
            | DataType::Date64
            | DataType::Time32(TimeUnit::Second)
            | DataType::Time32(TimeUnit::Millisecond)
            | DataType::Timestamp(TimeUnit::Microsecond, _)
            | DataType::Decimal128(_, _)
            | DataType::Utf8
            | DataType::Boolean
    );
    if !supported {
        return Err(KaveonError::Execution(format!(
            "ARRAY_SORT does not support list element type {}",
            list.value_type()
        )));
    }
    let mut selected = Vec::<ArrayRef>::new();
    let mut offsets = vec![L::zero()];
    let mut valid = Vec::with_capacity(list.len());
    for row in 0..list.len() {
        if list.is_null(row) {
            valid.push(false);
            offsets.push(L::from_usize(selected.len()).ok_or_else(|| {
                KaveonError::Execution("ARRAY_SORT result offset overflow".into())
            })?);
            continue;
        }
        valid.push(true);
        let inner = list.value(row);
        let mut entries = (0..inner.len())
            .map(|index| {
                if inner.is_null(index) {
                    None
                } else {
                    Some(inner.slice(index, 1))
                }
            })
            .collect::<Vec<_>>();
        entries.sort_by(|left, right| match (left, right) {
            (None, None) => std::cmp::Ordering::Equal,
            (None, Some(_)) => std::cmp::Ordering::Greater,
            (Some(_), None) => std::cmp::Ordering::Less,
            (Some(left), Some(right)) => {
                let less = comparison(left, right, CompareKind::Lt)
                    .map(|value| !value.is_null(0) && value.value(0))
                    .unwrap_or(false);
                let greater = comparison(left, right, CompareKind::Gt)
                    .map(|value| !value.is_null(0) && value.value(0))
                    .unwrap_or(false);
                if less {
                    std::cmp::Ordering::Less
                } else if greater {
                    std::cmp::Ordering::Greater
                } else {
                    std::cmp::Ordering::Equal
                }
            }
        });
        selected.extend(entries.into_iter().flatten());
        offsets.push(L::from_usize(selected.len()).ok_or_else(|| {
            KaveonError::Execution("ARRAY_SORT result offset overflow".into())
        })?);
    }
    let values = if selected.is_empty() {
        arrow::array::new_empty_array(&list.value_type())
    } else {
        let chunks = selected
            .iter()
            .map(|value| value.as_ref() as &dyn Array)
            .collect::<Vec<_>>();
        compute::concat(&chunks)?
    };
    let field = Arc::new(arrow::datatypes::Field::new(
        "item",
        values.data_type().clone(),
        true,
    ));
    Ok(Arc::new(GenericListArray::<L>::try_new(
        field,
        OffsetBuffer::new(ScalarBuffer::from(offsets)),
        values,
        Some(arrow::buffer::NullBuffer::from(valid)),
    )?))
}

fn array_slice_generic<L: OffsetSizeTrait>(
    list: &GenericListArray<L>,
    lower: &Int64Array,
    upper: &Int64Array,
    stride: &Int64Array,
) -> Result<ArrayRef> {
    let mut selected = Vec::<ArrayRef>::new();
    let mut offsets = vec![L::zero()];
    let mut valid = Vec::with_capacity(list.len());
    for row in 0..list.len() {
        if list.is_null(row) {
            valid.push(false);
            offsets.push(L::from_usize(selected.len()).ok_or_else(|| {
                KaveonError::Execution("ARRAY_SLICE result offset overflow".into())
            })?);
            continue;
        }
        let inner = list.value(row);
        let length = inner.len() as i64;
        let step = stride.is_valid(row).then(|| stride.value(row)).unwrap_or(1);
        if step == 0 {
            return Err(KaveonError::Execution("ARRAY_SLICE stride cannot be zero".into()));
        }
        let start = lower
            .is_valid(row)
            .then(|| lower.value(row))
            .unwrap_or(if step < 0 { length } else { 1 });
        let end = upper
            .is_valid(row)
            .then(|| upper.value(row))
            .unwrap_or(if step < 0 { 1 } else { length });
        let first = if step > 0 {
            start.max(1).min(length + 1)
        } else {
            start.max(1).min(length)
        };
        let last = if step > 0 {
            end.max(0).min(length)
        } else {
            end.max(1).min(length)
        };
        if (step > 0 && first <= last) || (step < 0 && first >= last) {
            let mut index = first;
            while (step > 0 && index <= last) || (step < 0 && index >= last) {
                selected.push(inner.slice((index - 1) as usize, 1));
                index += step;
            }
        }
        valid.push(true);
        offsets.push(L::from_usize(selected.len()).ok_or_else(|| {
            KaveonError::Execution("ARRAY_SLICE result offset overflow".into())
        })?);
    }
    let values = if selected.is_empty() {
        arrow::array::new_empty_array(&list.value_type())
    } else {
        let chunks = selected
            .iter()
            .map(|value| value.as_ref() as &dyn Array)
            .collect::<Vec<_>>();
        compute::concat(&chunks)?
    };
    let field = Arc::new(arrow::datatypes::Field::new(
        "item",
        values.data_type().clone(),
        true,
    ));
    Ok(Arc::new(GenericListArray::<L>::try_new(
        field,
        OffsetBuffer::new(ScalarBuffer::from(offsets)),
        values,
        Some(arrow::buffer::NullBuffer::from(valid)),
    )?))
}

fn array_extrema_generic<L: OffsetSizeTrait>(
    list: &GenericListArray<L>,
    take_max: bool,
    name: &str,
) -> Result<ArrayRef> {
    let supported = matches!(
        list.value_type(),
        DataType::Int32
            | DataType::Int64
            | DataType::Float64
            | DataType::Date32
            | DataType::Date64
            | DataType::Time32(TimeUnit::Second)
            | DataType::Time32(TimeUnit::Millisecond)
            | DataType::Timestamp(TimeUnit::Microsecond, _)
            | DataType::Decimal128(_, _)
            | DataType::Utf8
    );
    if !supported {
        return Err(KaveonError::Execution(format!(
            "{name} does not support list element type {}",
            list.value_type()
        )));
    }
    let mut selected = Vec::<ArrayRef>::with_capacity(list.len());
    for row in 0..list.len() {
        if list.is_null(row) {
            selected.push(arrow::array::new_null_array(&list.value_type(), 1));
            continue;
        }
        let inner = list.value(row);
        let mut best: Option<ArrayRef> = None;
        for index in 0..inner.len() {
            if inner.is_null(index) {
                continue;
            }
            let candidate = inner.slice(index, 1);
            let replace = match best.as_ref() {
                None => true,
                Some(current) => {
                    let kind = if take_max { CompareKind::Gt } else { CompareKind::Lt };
                    comparison(&candidate, current, kind)
                        .map(|value| !value.is_null(0) && value.value(0))
                        .unwrap_or(false)
                }
            };
            if replace {
                best = Some(candidate);
            }
        }
        selected.push(
            best.unwrap_or_else(|| arrow::array::new_null_array(&list.value_type(), 1)),
        );
    }
    let chunks = selected
        .iter()
        .map(|value| value.as_ref() as &dyn Array)
        .collect::<Vec<_>>();
    Ok(compute::concat(&chunks)?)
}


#[derive(Clone, Copy)]
enum ArraySetOp {
    Union,
    Intersect,
    Except,
}

fn array_set_operation<L: OffsetSizeTrait>(
    left: &GenericListArray<L>,
    right: &GenericListArray<L>,
    operation: ArraySetOp,
) -> Result<ArrayRef> {
    if left.len() != right.len() || left.value_type() != right.value_type() {
        return Err(KaveonError::Execution(
            "array set operations require matching row counts and element types".into(),
        ));
    }
    let mut selected = Vec::<ArrayRef>::new();
    let mut offsets = vec![L::zero()];
    let mut valid = Vec::with_capacity(left.len());
    for row in 0..left.len() {
        if left.is_null(row) || right.is_null(row) {
            valid.push(false);
            offsets.push(L::from_usize(selected.len()).ok_or_else(|| {
                KaveonError::Execution("array set operation result offset overflow".into())
            })?);
            continue;
        }
        valid.push(true);
        let left_inner = left.value(row);
        let right_inner = right.value(row);
        let row_start = selected.len();
        let mut add_candidate = |candidate: ArrayRef| {
            let duplicate = selected[row_start..].iter().any(|previous| {
                if candidate.null_count() != 0 || previous.null_count() != 0 {
                    candidate.null_count() != 0 && previous.null_count() != 0
                } else {
                    comparison(&candidate, previous, CompareKind::Eq)
                        .map(|equal| !equal.is_null(0) && equal.value(0))
                        .unwrap_or(false)
                }
            });
            if !duplicate {
                selected.push(candidate);
            }
        };
        match operation {
            ArraySetOp::Union => {
                for source in [&left_inner, &right_inner] {
                    for index in 0..source.len() {
                        add_candidate(if source.is_null(index) {
                            arrow::array::new_null_array(source.data_type(), 1)
                        } else {
                            source.slice(index, 1)
                        });
                    }
                }
            }
            ArraySetOp::Intersect | ArraySetOp::Except => {
                for index in 0..left_inner.len() {
                    let candidate = if left_inner.is_null(index) {
                        arrow::array::new_null_array(left_inner.data_type(), 1)
                    } else {
                        left_inner.slice(index, 1)
                    };
                    let in_right = (0..right_inner.len()).any(|right_index| {
                        if candidate.null_count() != 0 || right_inner.is_null(right_index) {
                            candidate.null_count() != 0 && right_inner.is_null(right_index)
                        } else {
                            comparison(
                                &candidate,
                                &right_inner.slice(right_index, 1),
                                CompareKind::Eq,
                            )
                            .map(|equal| !equal.is_null(0) && equal.value(0))
                            .unwrap_or(false)
                        }
                    });
                    if matches!(operation, ArraySetOp::Intersect) == in_right {
                        add_candidate(candidate);
                    }
                }
            }
        }
        offsets.push(L::from_usize(selected.len()).ok_or_else(|| {
            KaveonError::Execution("array set operation result offset overflow".into())
        })?);
    }
    let values = if selected.is_empty() {
        arrow::array::new_empty_array(&left.value_type())
    } else {
        let chunks = selected
            .iter()
            .map(|value| value.as_ref() as &dyn Array)
            .collect::<Vec<_>>();
        compute::concat(&chunks)?
    };
    let field = Arc::new(arrow::datatypes::Field::new(
        "item",
        values.data_type().clone(),
        true,
    ));
    Ok(Arc::new(GenericListArray::<L>::try_new(
        field,
        OffsetBuffer::new(ScalarBuffer::from(offsets)),
        values,
        Some(arrow::buffer::NullBuffer::from(valid)),
    )?))
}

fn eval_scalar_function(name: &str, args: &[ArrayRef], num_rows: usize) -> Result<ArrayRef> {
    let upper = name.to_uppercase();
    if upper.starts_with("ARRAY_ANY_") || upper.starts_with("ARRAY_ALL_") {
        return eval_array_quantified_compare(&upper, args, num_rows);
    }
    if upper == "ARRAY" {
        if args.is_empty() {
            return Err(KaveonError::Execution(
                "ARRAY requires at least one element".into(),
            ));
        }
        let mut data_type = args
            .iter()
            .map(|arg| arg.data_type())
            .find(|data_type| **data_type != DataType::Null)
            .cloned()
            .unwrap_or(DataType::Null);
        if data_type == DataType::Int64
            && args
                .iter()
                .any(|arg| arg.data_type() == &DataType::Float64)
        {
            data_type = DataType::Float64;
        }
        if args.iter().any(|arg| {
            arg.data_type() != &data_type
                && arg.data_type() != &DataType::Null
                && !(data_type == DataType::Float64 && arg.data_type() == &DataType::Int64)
        }) {
            return Err(KaveonError::Execution(
                "ARRAY elements must have one common type".into(),
            ));
        }
        if matches!(
            data_type,
            DataType::List(_) | DataType::LargeList(_) | DataType::Struct(_) | DataType::Map(_, _)
        ) {
            let mut child_chunks = Vec::with_capacity(num_rows.saturating_mul(args.len()));
            let mut offsets = vec![0_i32];
            for row in 0..num_rows {
                for arg in args {
                    let chunk = match arg.data_type() {
                        DataType::List(_)
                        | DataType::LargeList(_)
                        | DataType::Struct(_)
                        | DataType::Map(_, _) => arg.slice(row, 1),
                        DataType::Null => arrow::array::new_null_array(&data_type, 1),
                        other => {
                            return Err(KaveonError::Execution(format!(
                                "ARRAY elements must have one common type, found {other}"
                            )))
                        }

                    };
                    child_chunks.push(chunk);
                }
                offsets.push(child_chunks.len() as i32);
            }
            let values = if child_chunks.is_empty() {
                arrow::array::new_empty_array(&data_type)
            } else {
                let chunks = child_chunks
                    .iter()
                    .map(|chunk| chunk.as_ref() as &dyn Array)
                    .collect::<Vec<_>>();
                compute::concat(&chunks)?
            };
            let field = Arc::new(arrow::datatypes::Field::new(
                "item",
                values.data_type().clone(),
                true,
            ));
            return Ok(Arc::new(ListArray::try_new(
                field,
                OffsetBuffer::new(ScalarBuffer::from(offsets)),
                values,
                None,
            )?));
        }
        macro_rules! build_array {
            ($builder:expr, $array_ty:ty, $value:expr) => {{
                let mut builder = ListBuilder::new($builder);
                for row in 0..num_rows {
                    for arg in args {
                        if arg.data_type() == &DataType::Null {
                            builder.values().append_null();
                            continue;
                        }
                        let values = arg.as_any().downcast_ref::<$array_ty>().ok_or_else(|| {
                            KaveonError::Execution("ARRAY element type could not be decoded".into())
                        })?;
                        if values.is_null(row) {
                            builder.values().append_null();
                        } else {
                            builder.values().append_value($value(values, row));
                        }
                    }
                    builder.append(true);
                }
                Ok(Arc::new(builder.finish()) as ArrayRef)
            }};
        }
        return match data_type {
            DataType::Boolean => build_array!(
                BooleanBuilder::new(),
                BooleanArray,
                |values: &BooleanArray, row| values.value(row)
            ),
            DataType::Int64 => build_array!(
                Int64Builder::new(),
                Int64Array,
                |values: &Int64Array, row| values.value(row)
            ),
            DataType::Int32 => build_array!(
                Int32Builder::new(),
                Int32Array,
                |values: &Int32Array, row| values.value(row)
            ),
            DataType::Int8 => build_array!(
                PrimitiveBuilder::<Int8Type>::new(),
                Int8Array,
                |values: &Int8Array, row| values.value(row)
            ),
            DataType::Int16 => build_array!(
                PrimitiveBuilder::<Int16Type>::new(),
                Int16Array,
                |values: &Int16Array, row| values.value(row)
            ),
            DataType::UInt32 => build_array!(
                PrimitiveBuilder::<UInt32Type>::new(),
                UInt32Array,
                |values: &UInt32Array, row| values.value(row)
            ),
            DataType::UInt64 => build_array!(
                PrimitiveBuilder::<UInt64Type>::new(),
                UInt64Array,
                |values: &UInt64Array, row| values.value(row)
            ),
            DataType::UInt8 => build_array!(
                PrimitiveBuilder::<UInt8Type>::new(),
                UInt8Array,
                |values: &UInt8Array, row| values.value(row)
            ),
            DataType::UInt16 => build_array!(
                PrimitiveBuilder::<UInt16Type>::new(),
                UInt16Array,
                |values: &UInt16Array, row| values.value(row)
            ),
            DataType::Float32 => build_array!(
                PrimitiveBuilder::<Float32Type>::new(),
                Float32Array,
                |values: &Float32Array, row| values.value(row)
            ),
            DataType::Date32 => build_array!(
                PrimitiveBuilder::<arrow::datatypes::Date32Type>::new(),
                Date32Array,
                |values: &Date32Array, row| values.value(row)
            ),
            DataType::Date64 => build_array!(
                PrimitiveBuilder::<arrow::datatypes::Date64Type>::new(),
                Date64Array,
                |values: &Date64Array, row| values.value(row)
            ),
            DataType::Time32(TimeUnit::Second) => build_array!(
                PrimitiveBuilder::<Time32SecondType>::new(),
                Time32SecondArray,
                |values: &Time32SecondArray, row| values.value(row)
            ),
            DataType::Time32(TimeUnit::Millisecond) => build_array!(
                PrimitiveBuilder::<Time32MillisecondType>::new(),
                Time32MillisecondArray,
                |values: &Time32MillisecondArray, row| values.value(row)
            ),
            DataType::Time64(TimeUnit::Microsecond) => build_array!(
                PrimitiveBuilder::<Time64MicrosecondType>::new(),
                Time64MicrosecondArray,
                |values: &Time64MicrosecondArray, row| values.value(row)
            ),
            DataType::Time64(TimeUnit::Nanosecond) => build_array!(
                PrimitiveBuilder::<Time64NanosecondType>::new(),
                Time64NanosecondArray,
                |values: &Time64NanosecondArray, row| values.value(row)
            ),
            DataType::Timestamp(TimeUnit::Microsecond, _) => build_array!(
                PrimitiveBuilder::<TimestampMicrosecondType>::new(),
                TimestampMicrosecondArray,
                |values: &TimestampMicrosecondArray, row| values.value(row)
            ),
            DataType::Timestamp(TimeUnit::Second, _) => build_array!(
                PrimitiveBuilder::<TimestampSecondType>::new(),
                TimestampSecondArray,
                |values: &TimestampSecondArray, row| values.value(row)
            ),
            DataType::Timestamp(TimeUnit::Millisecond, _) => build_array!(
                PrimitiveBuilder::<TimestampMillisecondType>::new(),
                TimestampMillisecondArray,
                |values: &TimestampMillisecondArray, row| values.value(row)
            ),
            DataType::Timestamp(TimeUnit::Nanosecond, _) => build_array!(
                PrimitiveBuilder::<TimestampNanosecondType>::new(),
                TimestampNanosecondArray,
                |values: &TimestampNanosecondArray, row| values.value(row)
            ),
            DataType::Float64 => {
                let mut builder = ListBuilder::new(Float64Builder::new());
                for row in 0..num_rows {
                    for arg in args {
                        if arg.data_type() == &DataType::Null {
                            builder.values().append_null();
                        } else if let Some(values) = arg.as_any().downcast_ref::<Float64Array>() {
                            if values.is_null(row) {
                                builder.values().append_null();
                            } else {
                                builder.values().append_value(values.value(row));
                            }
                        } else if let Some(values) = arg.as_any().downcast_ref::<Int64Array>() {
                            if values.is_null(row) {
                                builder.values().append_null();
                            } else {
                                builder.values().append_value(values.value(row) as f64);
                            }
                        } else {
                            return Err(KaveonError::Execution(
                                "ARRAY element type could not be decoded".into(),
                            ));
                        }
                    }
                    builder.append(true);
                }
                Ok(Arc::new(builder.finish()) as ArrayRef)
            }
            DataType::Utf8 => {
                let mut builder = ListBuilder::new(StringBuilder::new());
                for row in 0..num_rows {
                    for arg in args {
                        if arg.data_type() == &DataType::Null {
                            builder.values().append_null();
                            continue;
                        }
                        let values = arg
                            .as_any()
                            .downcast_ref::<StringArray>()
                            .ok_or_else(|| {
                                KaveonError::Execution(
                                    "ARRAY element type could not be decoded".into(),
                                )
                            })?;
                        if values.is_null(row) {
                            builder.values().append_null();
                        } else {
                            builder.values().append_value(values.value(row));
                        }
                    }
                    builder.append(true);
                }
                Ok(Arc::new(builder.finish()) as ArrayRef)
            }
            DataType::Decimal128(precision, scale) => {
                let mut builder = ListBuilder::new(
                    Decimal128Builder::new()
                        .with_precision_and_scale(precision, scale)
                        .map_err(|error| KaveonError::Execution(error.to_string()))?,
                );
                for row in 0..num_rows {
                    for arg in args {
                        if arg.data_type() == &DataType::Null {
                            builder.values().append_null();
                            continue;
                        }
                        let values = arg
                            .as_any()
                            .downcast_ref::<Decimal128Array>()
                            .ok_or_else(|| {
                                KaveonError::Execution(
                                    "ARRAY element type could not be decoded".into(),
                                )
                            })?;
                        if values.is_null(row) {
                            builder.values().append_null();
                        } else {
                            builder.values().append_value(values.value(row));
                        }
                    }
                    builder.append(true);
                }
                Ok(Arc::new(builder.finish()) as ArrayRef)
            }
            DataType::Null => {
                let mut builder = ListBuilder::new(NullBuilder::new());
                for _row in 0..num_rows {
                    for _ in args {
                        builder.values().append_null();
                    }
                    builder.append(true);
                }
                Ok(Arc::new(builder.finish()) as ArrayRef)
            }
            _ => Err(KaveonError::Execution(format!(
                "ARRAY elements of type {data_type} are not supported"
            ))),
        };
    }
    if matches!(upper.as_str(), "ARRAY_UNION" | "ARRAY_INTERSECT" | "ARRAY_EXCEPT") {
        check_arity(name, args, 2)?;
        let operation = match upper.as_str() {
            "ARRAY_UNION" => ArraySetOp::Union,
            "ARRAY_INTERSECT" => ArraySetOp::Intersect,
            _ => ArraySetOp::Except,
        };
        if let (Some(left), Some(right)) = (
            args[0].as_any().downcast_ref::<ListArray>(),
            args[1].as_any().downcast_ref::<ListArray>(),
        ) {
            return array_set_operation(left, right, operation);
        }
        if let (Some(left), Some(right)) = (
            args[0].as_any().downcast_ref::<LargeListArray>(),
            args[1].as_any().downcast_ref::<LargeListArray>(),
        ) {
            return array_set_operation(left, right, operation);
        }
        return Err(KaveonError::Execution(format!(
            "{upper} requires matching ARRAY/List values"
        )))
    }
    if upper == "ARRAY_REPEAT" {
        check_arity(name, args, 2)?;
        let counts = compute::cast(&args[1], &DataType::Int64)?;
        let counts = counts.as_primitive::<Int64Type>();
        const MAX_ARRAY_REPEAT: i64 = 1_000_000;
        macro_rules! repeat_primitive {
            ($array_ty:ty, $builder:expr, $value:expr) => {{
                let elements = args[0].as_any().downcast_ref::<$array_ty>().ok_or_else(|| {
                    KaveonError::Execution(format!(
                        "ARRAY_REPEAT does not support element type {}",
                        args[0].data_type()
                    ))
                })?;
                let mut builder = ListBuilder::new($builder);
                for row in 0..num_rows {
                    if counts.is_null(row) {
                        builder.append(false);
                        continue;
                    }
                    let count = counts.value(row);
                    if !(0..=MAX_ARRAY_REPEAT).contains(&count) {
                        return Err(KaveonError::Execution(
                            "ARRAY_REPEAT count must be between 0 and 1000000".into(),
                        ));
                    }
                    if elements.is_null(row) {
                        for _ in 0..count {
                            builder.values().append_null();
                        }
                    } else {
                        for _ in 0..count {
                            builder.values().append_value($value(elements, row));
                        }
                    }
                    builder.append(true);
                }
                Ok(Arc::new(builder.finish()) as ArrayRef)
            }};
        }
        return match args[0].data_type() {
            DataType::Boolean => repeat_primitive!(BooleanArray, BooleanBuilder::new(), |a: &BooleanArray, r| a.value(r)),
            DataType::Int32 => repeat_primitive!(Int32Array, Int32Builder::new(), |a: &Int32Array, r| a.value(r)),
            DataType::Int64 => repeat_primitive!(Int64Array, Int64Builder::new(), |a: &Int64Array, r| a.value(r)),
            DataType::Float32 => repeat_primitive!(Float32Array, PrimitiveBuilder::<Float32Type>::new(), |a: &Float32Array, r| a.value(r)),
            DataType::Float64 => repeat_primitive!(Float64Array, Float64Builder::new(), |a: &Float64Array, r| a.value(r)),
            DataType::Utf8 => {
                let elements = args[0].as_string::<i32>();
                let mut builder = ListBuilder::new(StringBuilder::new());
                for row in 0..num_rows {
                    if counts.is_null(row) {
                        builder.append(false);
                        continue;
                    }
                    let count = counts.value(row);
                    if !(0..=MAX_ARRAY_REPEAT).contains(&count) {
                        return Err(KaveonError::Execution(
                            "ARRAY_REPEAT count must be between 0 and 1000000".into(),
                        ));
                    }
                    for _ in 0..count {
                        if elements.is_null(row) {
                            builder.values().append_null();
                        } else {
                            builder.values().append_value(elements.value(row));
                        }
                    }
                    builder.append(true);
                }
                Ok(Arc::new(builder.finish()) as ArrayRef)
            }
            DataType::Null => {
                let mut builder = ListBuilder::new(NullBuilder::new());
                for row in 0..num_rows {
                    if counts.is_null(row) {
                        builder.append(false);
                        continue;
                    }
                    let count = counts.value(row);
                    if !(0..=MAX_ARRAY_REPEAT).contains(&count) {
                        return Err(KaveonError::Execution(
                            "ARRAY_REPEAT count must be between 0 and 1000000".into(),
                        ));
                    }
                    for _ in 0..count {
                        builder.values().append_null();
                    }
                    builder.append(true);
                }
                Ok(Arc::new(builder.finish()) as ArrayRef)
            }
            other => Err(KaveonError::Execution(format!(
                "ARRAY_REPEAT does not support element type {other}"
            ))),
        };
    }
    if upper == "ROW_FIELD" {
        check_arity(name, args, 2)?;
        let row = args[0]
            .as_any()
            .downcast_ref::<StructArray>()
            .ok_or_else(|| KaveonError::Execution("ROW_FIELD requires a ROW value".into()))?;
        let field = as_string_array(&args[1])?;
        let field = if field.is_null(0) {
            return Err(KaveonError::Execution(
                "ROW_FIELD name cannot be NULL".into(),
            ));
        } else {
            field.value(0)
        };
        return row
            .column_by_name(field)
            .cloned()
            .ok_or_else(|| KaveonError::Execution(format!("ROW_FIELD '{field}' not found")));
    }
    if upper == "ROW" {
        let fields = args
            .iter()
            .enumerate()
            .map(|(index, value)| {
                Arc::new(arrow::datatypes::Field::new(
                    format!("field{index}"),
                    value.data_type().clone(),
                    true,
                ))
            })
            .collect::<Vec<_>>()
            .into();
        return Ok(Arc::new(StructArray::new(fields, args.to_vec(), None)));
    }
    if upper == "MAP" {
        check_arity(name, args, 2)?;
        let keys = args[0]
            .as_any()
            .downcast_ref::<ListArray>()
            .ok_or_else(|| KaveonError::Execution("MAP keys must be an ARRAY/List value".into()))?;
        let values = args[1]
            .as_any()
            .downcast_ref::<ListArray>()
            .ok_or_else(|| {
                KaveonError::Execution("MAP values must be an ARRAY/List value".into())
            })?;
        if keys.offsets().as_ref() != values.offsets().as_ref() {
            return Err(KaveonError::Execution(
                "MAP keys and values must have matching list offsets".into(),
            ));
        }
        if keys.nulls() != values.nulls() {
            return Err(KaveonError::Execution(
                "MAP keys and values must have matching null rows".into(),
            ));
        }
        if keys.values().data_type() == &DataType::Null || keys.values().null_count() > 0 {
            return Err(KaveonError::Execution(
                "MAP keys cannot contain NULL values".into(),
            ));
        }
        let fields = vec![
            Arc::new(arrow::datatypes::Field::new(
                "key",
                keys.values().data_type().clone(),
                false,
            )),
            Arc::new(arrow::datatypes::Field::new(
                "value",
                values.values().data_type().clone(),
                true,
            )),
        ];
        let entry_fields: arrow::datatypes::Fields = fields.clone().into();
        let entries = StructArray::new(
            entry_fields.clone(),
            vec![keys.values().clone(), values.values().clone()],
            None,
        );
        let entry_field = Arc::new(arrow::datatypes::Field::new(
            "entries",
            DataType::Struct(entry_fields),
            false,
        ));
        return Ok(Arc::new(MapArray::try_new(
            entry_field,
            keys.offsets().clone(),
            entries,
            keys.nulls().cloned(),
            false,
        )?));
    }
    if upper == "SPLIT_TO_MAP" {
        check_arity(name, args, 3)?;
        let sources = as_string_array(&args[0])?;
        let entry_delimiters = as_string_array(&args[1])?;
        let key_delimiters = as_string_array(&args[2])?;
        let mut offsets = Vec::with_capacity(num_rows + 1);
        offsets.push(0_i32);
        let mut valid = Vec::with_capacity(num_rows);
        let mut keys = Vec::new();
        let mut values = Vec::new();
        for row in 0..num_rows {
            if sources.is_null(row)
                || entry_delimiters.is_null(row)
                || key_delimiters.is_null(row)
            {
                valid.push(false);
                offsets.push(keys.len() as i32);
                continue;
            }
            let entry_delimiter = entry_delimiters.value(row);
            let key_delimiter = key_delimiters.value(row);
            if entry_delimiter.is_empty() || key_delimiter.is_empty() {
                return Err(KaveonError::Execution(
                    "SPLIT_TO_MAP delimiters must not be empty".into(),
                ));
            }
            let mut seen = std::collections::HashSet::new();
            for entry in sources.value(row).split(entry_delimiter) {
                let mut pair = entry.splitn(2, key_delimiter);
                let key = pair.next().unwrap_or_default();
                let value = pair.next().unwrap_or_default();
                if !seen.insert(key.to_owned()) {
                    return Err(KaveonError::Execution(format!(
                        "SPLIT_TO_MAP duplicate key {key:?}"
                    )));
                }
                keys.push(key.to_owned());
                values.push(value.to_owned());
            }
            valid.push(true);
            offsets.push(keys.len() as i32);
        }
        let key_array = Arc::new(StringArray::from(keys)) as ArrayRef;
        let value_array = Arc::new(StringArray::from(values)) as ArrayRef;
        let entry_fields: arrow::datatypes::Fields = vec![
            Arc::new(arrow::datatypes::Field::new("key", DataType::Utf8, false)),
            Arc::new(arrow::datatypes::Field::new("value", DataType::Utf8, true)),
        ]
        .into();
        let entries = StructArray::new(entry_fields.clone(), vec![key_array, value_array], None);
        let entry_field = Arc::new(arrow::datatypes::Field::new(
            "entries",
            DataType::Struct(entry_fields),
            false,
        ));
        return Ok(Arc::new(MapArray::try_new(
            entry_field,
            OffsetBuffer::new(ScalarBuffer::from(offsets)),
            entries,
            Some(arrow::buffer::NullBuffer::from(valid)),
            false,
        )?));
    }
    if upper == "SPLIT_TO_MULTIMAP" {
        check_arity(name, args, 3)?;
        let sources = as_string_array(&args[0])?;
        let entry_delimiters = as_string_array(&args[1])?;
        let key_delimiters = as_string_array(&args[2])?;
        let mut map_offsets = vec![0_i32];
        let mut value_offsets = vec![0_i32];
        let mut valid = Vec::with_capacity(num_rows);
        let mut keys = Vec::new();
        let mut values = Vec::new();
        for row in 0..num_rows {
            if sources.is_null(row)
                || entry_delimiters.is_null(row)
                || key_delimiters.is_null(row)
            {
                valid.push(false);
                map_offsets.push(keys.len() as i32);
                continue;
            }
            let entry_delimiter = entry_delimiters.value(row);
            let key_delimiter = key_delimiters.value(row);
            if entry_delimiter.is_empty() || key_delimiter.is_empty() {
                return Err(KaveonError::Execution(
                    "SPLIT_TO_MULTIMAP delimiters must not be empty".into(),
                ));
            }
            let mut grouped = Vec::<(String, Vec<String>)>::new();
            let mut positions = std::collections::HashMap::<String, usize>::new();
            for entry in sources.value(row).split(entry_delimiter) {
                let mut pair = entry.splitn(2, key_delimiter);
                let key = pair.next().unwrap_or_default().to_owned();
                let value = pair.next().unwrap_or_default().to_owned();
                if let Some(position) = positions.get(&key).copied() {
                    grouped[position].1.push(value);
                } else {
                    positions.insert(key.clone(), grouped.len());
                    grouped.push((key, vec![value]));
                }
            }
            for (key, grouped_values) in grouped {
                keys.push(key);
                values.extend(grouped_values);
                value_offsets.push(values.len() as i32);
            }
            valid.push(true);
            map_offsets.push(keys.len() as i32);
        }
        let key_array = Arc::new(StringArray::from(keys)) as ArrayRef;
        let value_array = Arc::new(StringArray::from(values)) as ArrayRef;
        let list_field = Arc::new(arrow::datatypes::Field::new("item", DataType::Utf8, true));
        let list_array = ListArray::try_new(
            list_field,
            OffsetBuffer::new(ScalarBuffer::from(value_offsets)),
            value_array,
            None,
        )?;
        let entry_fields: arrow::datatypes::Fields = vec![
            Arc::new(arrow::datatypes::Field::new("key", DataType::Utf8, false)),
            Arc::new(arrow::datatypes::Field::new(
                "value",
                list_array.data_type().clone(),
                true,
            )),
        ]
        .into();
        let entries = StructArray::new(
            entry_fields.clone(),
            vec![key_array, Arc::new(list_array)],
            None,
        );
        let entry_field = Arc::new(arrow::datatypes::Field::new(
            "entries",
            DataType::Struct(entry_fields),
            false,
        ));
        return Ok(Arc::new(MapArray::try_new(
            entry_field,
            OffsetBuffer::new(ScalarBuffer::from(map_offsets)),
            entries,
            Some(arrow::buffer::NullBuffer::from(valid)),
            false,
        )?));
    }
    if upper == "MAP_CONCAT" {
        if args.len() < 2 {
            return Err(KaveonError::Execution(
                "MAP_CONCAT requires at least 2 arguments".into(),
            ));
        }
        let maps = args
            .iter()
            .map(|arg| {
                arg.as_any().downcast_ref::<MapArray>().ok_or_else(|| {
                    KaveonError::Execution("MAP_CONCAT requires MAP values".into())
                })
            })
            .collect::<Result<Vec<_>>>()?;
        let key_type = maps[0].keys().data_type().clone();
        let value_type = maps[0].values().data_type().clone();
        if !matches!(
            &key_type,
            DataType::Boolean | DataType::Int64 | DataType::Float64 | DataType::Utf8
        ) {
            return Err(KaveonError::Execution(format!(
                "MAP_CONCAT does not support key type {key_type}"
            )));
        }
        if maps.iter().any(|map| {
            map.keys().data_type() != &key_type || map.values().data_type() != &value_type
        }) {
            return Err(KaveonError::Execution(
                "MAP_CONCAT requires matching key and value types".into(),
            ));
        }
        let key_sources = maps
            .iter()
            .map(|map| map.keys().as_ref() as &dyn Array)
            .collect::<Vec<_>>();
        let value_sources = maps
            .iter()
            .map(|map| map.values().as_ref() as &dyn Array)
            .collect::<Vec<_>>();
        let all_keys = compute::concat(&key_sources)?;
        let all_values = compute::concat(&value_sources)?;
        let mut selected = Vec::<u32>::new();
        let mut offsets = vec![0_i32];
        let mut valid = Vec::with_capacity(num_rows);
        let mut bases = Vec::with_capacity(maps.len());
        let mut base = 0_u32;
        for map in &maps {
            bases.push(base);
            base += map.keys().len() as u32;
        }
        for row in 0..num_rows {
            if maps.iter().any(|map| map.is_null(row)) {
                valid.push(false);
                offsets.push(selected.len() as i32);
                continue;
            }
            valid.push(true);
            for (map_index, map) in maps.iter().enumerate() {
                let start = map.value_offsets()[row] as usize;
                let end = map.value_offsets()[row + 1] as usize;
                for entry in start..end {
                    let global = bases[map_index] + entry as u32;
                    if let Some(position) = selected.iter().position(|candidate| {
                        map_keys_equal(&all_keys, *candidate as usize, global as usize)
                            .unwrap_or(false)
                    }) {
                        selected[position] = global;
                    } else {
                        selected.push(global);
                    }
                }
            }
            offsets.push(selected.len() as i32);
        }
        let indices = arrow::array::UInt32Array::from(selected);
        let keys = compute::take(all_keys.as_ref(), &indices, None)?;
        let values = compute::take(all_values.as_ref(), &indices, None)?;
        let entry_fields: arrow::datatypes::Fields = vec![
            Arc::new(arrow::datatypes::Field::new("key", key_type, false)),
            Arc::new(arrow::datatypes::Field::new("value", value_type, true)),
        ]
        .into();
        let entries = StructArray::new(entry_fields.clone(), vec![keys, values], None);
        let entry_field = Arc::new(arrow::datatypes::Field::new(
            "entries",
            DataType::Struct(entry_fields),
            false,
        ));
        return Ok(Arc::new(MapArray::try_new(
            entry_field,
            OffsetBuffer::new(ScalarBuffer::from(offsets)),
            entries,
            Some(arrow::buffer::NullBuffer::from(valid)),
            false,
        )?));
    }
    if upper == "MAP_KEYS" || upper == "MAP_VALUES" || upper == "MAP_ENTRIES" {
        check_arity(name, args, 1)?;
        let map = args[0].as_any().downcast_ref::<MapArray>().ok_or_else(|| {
            KaveonError::Execution(format!("{upper} currently supports MAP values only"))
        })?;
        if upper == "MAP_ENTRIES" {
            let values = Arc::new(map.entries().clone()) as ArrayRef;
            let field = Arc::new(arrow::datatypes::Field::new(
                "item",
                values.data_type().clone(),
                true,
            ));
            return Ok(Arc::new(ListArray::try_new(
                field,
                map.offsets().clone(),
                values,
                map.nulls().cloned(),
            )?));
        }
        let values = if upper == "MAP_KEYS" {
            map.keys()
        } else {
            map.values()
        };
        let field = Arc::new(arrow::datatypes::Field::new(
            "item",
            values.data_type().clone(),
            true,
        ));
        return Ok(Arc::new(ListArray::try_new(
            field,
            map.offsets().clone(),
            Arc::clone(values),
            map.nulls().cloned(),
        )?));
    }
    if upper == "MAP_ELEMENT_AT" {
        check_arity(name, args, 2)?;
        let map = args[0]
            .as_any()
            .downcast_ref::<MapArray>()
            .ok_or_else(|| KaveonError::Execution("MAP_ELEMENT_AT requires a MAP value".into()))?;
        let map_keys = map.keys();
        let requested = compute::cast(&args[1], map_keys.data_type())?;
        let mut positions = Vec::with_capacity(num_rows);
        for row in 0..num_rows {
            if map.is_null(row) || requested.is_null(row) {
                positions.push(None);
                continue;
            }
            let start = map.value_offsets()[row] as usize;
            let end = map.value_offsets()[row + 1] as usize;
            let found = match map_keys.data_type() {
                DataType::Float64 => {
                    let keys = map_keys.as_primitive::<Float64Type>();
                    let needle = requested.as_primitive::<Float64Type>().value(row);
                    (start..end).find(|&index| !keys.is_null(index) && keys.value(index) == needle)
                }
                DataType::Int64 => {
                    let keys = map_keys.as_primitive::<Int64Type>();
                    let needle = requested.as_primitive::<Int64Type>().value(row);
                    (start..end).find(|&index| !keys.is_null(index) && keys.value(index) == needle)
                }
                DataType::Utf8 => {
                    let keys = map_keys.as_string::<i32>();
                    let needle = requested.as_string::<i32>().value(row);
                    (start..end).find(|&index| !keys.is_null(index) && keys.value(index) == needle)
                }
                other => {
                    return Err(KaveonError::Execution(format!(
                        "MAP_ELEMENT_AT does not support key type {other}"
                    )));
                }
            };
            positions.push(found.map(|index| index as i64));
        }
        return Ok(compute::take(
            map.values().as_ref(),
            &Int64Array::from(positions),
            None,
        )?);
    }
    if let Some(target) = upper.strip_prefix("TRY_CAST_") {
        check_arity(name, args, 1)?;
        let target = match target {
            "BOOLEAN" => CastTarget::Boolean,
            "INT32" => CastTarget::Int32,
            "INT64" => CastTarget::Int64,
            "FLOAT64" => CastTarget::Float64,
            "UTF8" => CastTarget::Utf8,
            "DATE32" => CastTarget::Date32,
            "TIMESTAMP" => CastTarget::TimestampMicros {
                with_timezone: false,
            },
            "TIMESTAMP_TZ" => CastTarget::TimestampMicros {
                with_timezone: true,
            },
            decimal if decimal.starts_with("DECIMAL_") => {
                let mut pieces = decimal.split('_');
                let _ = pieces.next();
                let precision = pieces.next().and_then(|value| value.parse::<u8>().ok());
                let scale = pieces.next().and_then(|value| value.parse::<i8>().ok());
                if pieces.next().is_some() || precision.is_none() || scale.is_none() {
                    return Err(KaveonError::Execution(format!(
                        "unsupported TRY_CAST target: {decimal}"
                    )));
                }
                CastTarget::Decimal128 {
                    precision: precision.unwrap(),
                    scale: scale.unwrap(),
                }
            }
            _ => {
                return Err(KaveonError::Execution(format!(
                    "unsupported TRY_CAST target: {target}"
                )));
            }
        };
        return Ok(compute::cast_with_options(
            &args[0],
            &target.to_arrow_type(),
            &compute::CastOptions {
                safe: true,
                ..Default::default()
            },
        )?);
    }
    match upper.as_str() {
        "UPPER" => {
            check_arity(name, args, 1)?;
            let arr = as_string_array(&args[0])?;
            let result: StringArray = arr.iter().map(|v| v.map(|s| s.to_uppercase())).collect();
            Ok(Arc::new(result))
        }
        "LOWER" => {
            check_arity(name, args, 1)?;
            let arr = as_string_array(&args[0])?;
            let result: StringArray = arr.iter().map(|v| v.map(|s| s.to_lowercase())).collect();
            Ok(Arc::new(result))
        }
        "TRIM" => {
            check_arity(name, args, 1)?;
            let arr = as_string_array(&args[0])?;
            let result: StringArray = arr.iter().map(|v| v.map(|s| s.trim().to_owned())).collect();
            Ok(Arc::new(result))
        }
        "CHR" => {
            check_arity(name, args, 1)?;
            let values = compute::cast(&args[0], &DataType::Int64)?;
            let values = values.as_primitive::<Int64Type>();
            let result: StringArray = (0..num_rows)
                .map(|row| {
                    if values.is_null(row) {
                        return Ok(None);
                    }
                    let value = values.value(row);
                    let codepoint = u32::try_from(value).map_err(|_| {
                        KaveonError::Execution(format!("CHR code point {value} is invalid"))
                    })?;
                    char::from_u32(codepoint)
                        .map(|character| Some(character.to_string()))
                        .ok_or_else(|| {
                            KaveonError::Execution(format!("CHR code point {value} is invalid"))
                        })
                })
                .collect::<Result<Vec<_>>>()?
                .into();
            Ok(Arc::new(result))
        }
        "CODEPOINT" => {
            check_arity(name, args, 1)?;
            let values = as_string_array(&args[0])?;
            let result: Int64Array = (0..num_rows)
                .map(|row| {
                    if values.is_null(row) {
                        return Ok(None);
                    }
                    let mut characters = values.value(row).chars();
                    let character = characters.next().ok_or_else(|| {
                        KaveonError::Execution(
                            "CODEPOINT requires exactly one Unicode code point".into(),
                        )
                    })?;
                    if characters.next().is_some() {
                        return Err(KaveonError::Execution(
                            "CODEPOINT requires exactly one Unicode code point".into(),
                        ));
                    }
                    Ok(Some(i64::from(u32::from(character))))
                })
                .collect::<Result<Vec<_>>>()?
                .into();
            Ok(Arc::new(result))
        }
        "JSON_ARRAY_LENGTH" => {
            check_arity(name, args, 1)?;
            let documents = as_string_array(&args[0])?;
            let result: Int64Array = (0..num_rows)
                .map(|row| {
                    if documents.is_null(row) {
                        return None;
                    }
                    match serde_json::from_str::<serde_json::Value>(documents.value(row)).ok()? {
                        serde_json::Value::Array(values) => Some(values.len() as i64),
                        _ => None,
                    }
                })
                .collect();
            Ok(Arc::new(result))
        }
        "JSON_SIZE" => {
            check_arity(name, args, 2)?;
            let documents = as_string_array(&args[0])?;
            let paths = as_string_array(&args[1])?;
            let result: Int64Array = (0..num_rows)
                .map(|row| {
                    if documents.is_null(row) || paths.is_null(row) {
                        return None;
                    }
                    let value = json_extract_value(documents.value(row), paths.value(row))?;
                    Some(match value {
                        serde_json::Value::Array(values) => values.len() as i64,
                        serde_json::Value::Object(values) => values.len() as i64,
                        _ => 0,
                    })
                })
                .collect();
            Ok(Arc::new(result))
        }
        "JSON_EXTRACT" | "JSON_EXTRACT_SCALAR" => {
            check_arity(name, args, 2)?;
            let documents = as_string_array(&args[0])?;
            let paths = as_string_array(&args[1])?;
            let result: StringArray = (0..num_rows)
                .map(|row| {
                    if documents.is_null(row) || paths.is_null(row) {
                        return None;
                    }
                    if upper == "JSON_EXTRACT" {
                        json_extract(documents.value(row), paths.value(row))
                    } else {
                        json_extract_scalar(documents.value(row), paths.value(row))
                    }
                })
                .collect();
            Ok(Arc::new(result))
        }
        "JSON_PARSE" | "TRY_JSON_PARSE" => {
            check_arity(name, args, 1)?;
            let documents = as_string_array(&args[0])?;
            let try_parse = upper == "TRY_JSON_PARSE";
            let mut result = StringBuilder::new();
            for row in 0..num_rows {
                if documents.is_null(row) {
                    result.append_null();
                    continue;
                }
                let document = documents.value(row);
                if serde_json::from_str::<serde_json::Value>(document).is_ok() {
                    result.append_value(document);
                } else if try_parse {
                    result.append_null();
                } else {
                    return Err(KaveonError::Execution(format!(
                        "invalid JSON value at row {row}"
                    )));
                }
            }
            Ok(Arc::new(result.finish()))
        }
        "LTRIM" => {
            check_arity(name, args, 1)?;
            let arr = as_string_array(&args[0])?;
            let result: StringArray = arr
                .iter()
                .map(|v| v.map(|s| s.trim_start().to_owned()))
                .collect();
            Ok(Arc::new(result))
        }
        "RTRIM" => {
            check_arity(name, args, 1)?;
            let arr = as_string_array(&args[0])?;
            let result: StringArray = arr
                .iter()
                .map(|v| v.map(|s| s.trim_end().to_owned()))
                .collect();
            Ok(Arc::new(result))
        }
        "LENGTH" | "LEN" | "CHAR_LENGTH" | "CHARACTER_LENGTH" => {
            check_arity(name, args, 1)?;
            let arr = as_string_array(&args[0])?;
            let result: Int64Array = arr.iter().map(|v| v.map(|s| s.len() as i64)).collect();
            Ok(Arc::new(result))
        }
        "SUBSTR" | "SUBSTRING" => {
            if args.len() < 2 || args.len() > 3 {
                return Err(KaveonError::Execution(format!(
                    "{name} requires 2 or 3 arguments"
                )));
            }
            let arr = as_string_array(&args[0])?;
            let starts = args[1].as_primitive::<Int64Type>();
            let lengths = if args.len() == 3 {
                Some(args[2].as_primitive::<Int64Type>())
            } else {
                None
            };
            let result: StringArray = (0..num_rows)
                .map(|i| {
                    if arr.is_null(i) || starts.is_null(i) {
                        return None;
                    }
                    let s = arr.value(i);
                    let start = (starts.value(i) - 1).max(0) as usize;
                    if start >= s.len() {
                        return Some(String::new());
                    }
                    let remaining = &s[start..];
                    match lengths {
                        Some(lens) if !lens.is_null(i) => {
                            let len = lens.value(i).max(0) as usize;
                            Some(remaining.chars().take(len).collect())
                        }
                        _ => Some(remaining.to_owned()),
                    }
                })
                .collect();
            Ok(Arc::new(result))
        }
        "CONCAT" => {
            if args.is_empty() {
                return Ok(Arc::new(StringArray::from(vec![""; num_rows])));
            }
            let string_args: Vec<StringArray> =
                args.iter().map(cast_to_string).collect::<Result<_>>()?;
            let bytes = string_args.iter().fold(0_u64, |bytes, array| {
                bytes.saturating_add(array.value_data().len() as u64)
            });
            reserve_string_expansion(bytes, num_rows)?;
            let result: StringArray = (0..num_rows)
                .map(|i| {
                    let mut s = String::new();
                    for arr in &string_args {
                        if arr.is_null(i) {
                            continue;
                        }
                        s.push_str(arr.value(i));
                    }
                    Some(s)
                })
                .collect();
            Ok(Arc::new(result))
        }
        "CONCAT_WS" => {
            if args.len() < 2 {
                return Err(KaveonError::Execution(
                    "CONCAT_WS requires a separator and at least one value".into(),
                ));
            }
            let string_args: Vec<StringArray> = args[1..]
                .iter()
                .map(cast_to_string)
                .collect::<Result<_>>()?;
            let separator = cast_to_string(&args[0])?;
            let bytes = string_args.iter().fold(0_u64, |bytes, array| {
                bytes.saturating_add(array.value_data().len() as u64)
            });
            reserve_string_expansion(bytes, num_rows)?;
            let result: StringArray = (0..num_rows)
                .map(|row| {
                    if separator.is_null(row) {
                        return None;
                    }
                    let mut output = String::new();
                    let mut added = false;
                    for array in &string_args {
                        if array.is_null(row) {
                            continue;
                        }
                        if added {
                            output.push_str(separator.value(row));
                        }
                        output.push_str(array.value(row));
                        added = true;
                    }
                    Some(output)
                })
                .collect();
            Ok(Arc::new(result))
        }
        "REGEXP_REPLACE" => {
            check_arity(name, args, 3)?;
            let arr = as_string_array(&args[0])?;
            let patterns = as_string_array(&args[1])?;
            let replacements = as_string_array(&args[2])?;
            // Bounded by the input: a replacement can only grow a row by the
            // replacement text per match, and the budget is charged for the
            // input again plus that growth once the rows are written.
            reserve_string_expansion(arr.values().len() as u64, num_rows)?;
            let mut builder = StringBuilder::with_capacity(num_rows, arr.values().len());
            let mut expression: Option<(&str, regex::Regex)> = None;
            let mut grown = 0_u64;
            for i in 0..num_rows {
                if arr.is_null(i) || patterns.is_null(i) || replacements.is_null(i) {
                    builder.append_null();
                    continue;
                }
                // The pattern is a literal in practice: one compiled regex
                // serves the batch, from the process-wide cache.
                let pattern = patterns.value(i);
                if expression
                    .as_ref()
                    .is_none_or(|(current, _)| *current != pattern)
                {
                    expression = Some((pattern, compiled_regex(pattern)?));
                }
                let (_, regex) = expression.as_ref().expect("set above");
                let source = arr.value(i);
                match regex.replace_all(source, replacements.value(i)) {
                    std::borrow::Cow::Borrowed(unchanged) => builder.append_value(unchanged),
                    std::borrow::Cow::Owned(replaced) => {
                        grown = grown.saturating_add(
                            (replaced.len() as u64).saturating_sub(source.len() as u64),
                        );
                        builder.append_value(&replaced);
                    }
                }
                if i % 1024 == 1023 {
                    check_expression_cancelled()?;
                }
            }
            if grown != 0 {
                reserve_string_expansion(grown, 0)?;
            }
            Ok(Arc::new(builder.finish()))
        }
        "REGEXP_LIKE" => {
            check_arity(name, args, 2)?;
            let values = as_string_array(&args[0])?;
            let patterns = as_string_array(&args[1])?;
            let result: BooleanArray = (0..num_rows)
                .map(|row| {
                    if values.is_null(row) || patterns.is_null(row) {
                        return Ok(None);
                    }
                    Ok(Some(
                        compiled_regex_for("REGEXP_LIKE", patterns.value(row))?
                            .is_match(values.value(row)),
                    ))
                })
                .collect::<Result<Vec<_>>>()?
                .into();
            Ok(Arc::new(result))
        }
        "REGEXP_COUNT" => {
            check_arity(name, args, 2)?;
            let values = as_string_array(&args[0])?;
            let patterns = as_string_array(&args[1])?;
            let result: Int64Array = (0..num_rows)
                .map(|row| {
                    if values.is_null(row) || patterns.is_null(row) {
                        return Ok(None);
                    }
                    let regex = compiled_regex_for("REGEXP_COUNT", patterns.value(row))?;
                    Ok(Some(regex.find_iter(values.value(row)).count() as i64))
                })
                .collect::<Result<Vec<_>>>()?
                .into();
            Ok(Arc::new(result))
        }
        "REGEXP_EXTRACT_ALL" => {
            check_arity(name, args, 2)?;
            let values = as_string_array(&args[0])?;
            let patterns = as_string_array(&args[1])?;
            let mut builder = ListBuilder::new(StringBuilder::new());
            for row in 0..num_rows {
                if values.is_null(row) || patterns.is_null(row) {
                    builder.append(false);
                    continue;
                }
                let regex = compiled_regex_for("REGEXP_EXTRACT_ALL", patterns.value(row))?;
                let has_capture = regex.captures_len() > 1;
                for captures in regex.captures_iter(values.value(row)) {
                    let matched = captures
                        .get(if has_capture { 1 } else { 0 })
                        .map(|match_| match_.as_str())
                        .unwrap_or("");
                    builder.values().append_value(matched);
                }
                builder.append(true);
            }
            Ok(Arc::new(builder.finish()))
        }
        "REGEXP_EXTRACT" => {
            if args.len() != 2 && args.len() != 3 {
                return Err(KaveonError::Execution(
                    "REGEXP_EXTRACT requires 2 or 3 arguments".into(),
                ));
            }
            let values = as_string_array(&args[0])?;
            let patterns = as_string_array(&args[1])?;
            let groups = if args.len() == 3 {
                Some(compute::cast(&args[2], &DataType::Int64)?)
            } else {
                None
            };
            let groups = groups.as_ref().map(|array| array.as_primitive::<Int64Type>());
            let result: StringArray = (0..num_rows)
                .map(|row| {
                    if values.is_null(row) || patterns.is_null(row) {
                        return Ok(None);
                    }
                    let group = match groups {
                        Some(groups) if groups.is_null(row) => return Ok(None),
                        Some(groups) => {
                            let group = groups.value(row);
                            if group < 0 {
                                return Err(KaveonError::Execution(
                                    "REGEXP_EXTRACT group must be non-negative".into(),
                                ));
                            }
                            group as usize
                        }
                        None => 0,
                    };
                    let regex = compiled_regex_for("REGEXP_EXTRACT", patterns.value(row))?;
                    let captures = regex.captures(values.value(row));
                    Ok(captures.and_then(|captures| {
                        captures.get(group).map(|matched| matched.as_str().to_owned())
                    }))
                })
                .collect::<Result<Vec<_>>>()?
                .into();
            Ok(Arc::new(result))
        }
        "REGEXP_SPLIT" => {
            check_arity(name, args, 2)?;
            let values = as_string_array(&args[0])?;
            let patterns = as_string_array(&args[1])?;
            let mut builder = ListBuilder::new(StringBuilder::new());
            for row in 0..num_rows {
                if values.is_null(row) || patterns.is_null(row) {
                    builder.append(false);
                    continue;
                }
                let regex = compiled_regex_for("REGEXP_SPLIT", patterns.value(row))?;
                for part in regex.split(values.value(row)) {
                    builder.values().append_value(part);
                }
                builder.append(true);
            }
            Ok(Arc::new(builder.finish()))
        }
        "REPLACE" => {
            check_arity(name, args, 3)?;
            let arr = as_string_array(&args[0])?;
            let from = as_string_array(&args[1])?;
            let to = as_string_array(&args[2])?;
            let bytes = (0..num_rows).try_fold(0_u64, |total, index| {
                if arr.is_null(index) || from.is_null(index) || to.is_null(index) {
                    return Ok(total);
                }
                let source = arr.value(index);
                let matches = if from.value(index).is_empty() {
                    source.chars().count() + 1
                } else {
                    source.len() / from.value(index).len()
                };
                (matches as u64)
                    .checked_mul(to.value(index).len() as u64)
                    .and_then(|bytes| bytes.checked_add(source.len() as u64))
                    .and_then(|bytes| total.checked_add(bytes))
                    .ok_or_else(|| KaveonError::Execution("REPLACE output size overflow".into()))
            })?;
            reserve_string_expansion(bytes, num_rows)?;
            let result: StringArray = (0..num_rows)
                .map(|i| {
                    if arr.is_null(i) || from.is_null(i) || to.is_null(i) {
                        None
                    } else {
                        Some(arr.value(i).replace(from.value(i), to.value(i)))
                    }
                })
                .collect();
            Ok(Arc::new(result))
        }
        "TRANSLATE" => {
            check_arity(name, args, 3)?;
            let sources = as_string_array(&args[0])?;
            let from = as_string_array(&args[1])?;
            let to = as_string_array(&args[2])?;
            let bytes = sources.values().len() as u64;
            reserve_string_expansion(bytes, num_rows)?;
            let result: StringArray = (0..num_rows)
                .map(|row| {
                    if sources.is_null(row) || from.is_null(row) || to.is_null(row) {
                        return None;
                    }
                    let source = sources.value(row);
                    let from_chars = from.value(row).chars().collect::<Vec<_>>();
                    let to_chars = to.value(row).chars().collect::<Vec<_>>();
                    let mut mapping = std::collections::HashMap::new();
                    for (index, character) in from_chars.into_iter().enumerate() {
                        mapping.entry(character).or_insert(to_chars.get(index).copied());
                    }
                    Some(
                        source
                            .chars()
                            .filter_map(|character| match mapping.get(&character) {
                                Some(Some(mapped)) => Some(*mapped),
                                Some(None) => None,
                                None => Some(character),
                            })
                            .collect::<String>(),
                    )
                })
                .collect();
            Ok(Arc::new(result))
        }
        "LEFT" => {
            check_arity(name, args, 2)?;
            let arr = as_string_array(&args[0])?;
            let lens = args[1].as_primitive::<Int64Type>();
            let result: StringArray = (0..num_rows)
                .map(|i| {
                    if arr.is_null(i) || lens.is_null(i) {
                        None
                    } else {
                        let n = lens.value(i).max(0) as usize;
                        Some(arr.value(i).chars().take(n).collect::<String>())
                    }
                })
                .collect();
            Ok(Arc::new(result))
        }
        "RIGHT" => {
            check_arity(name, args, 2)?;
            let arr = as_string_array(&args[0])?;
            let lens = args[1].as_primitive::<Int64Type>();
            let result: StringArray = (0..num_rows)
                .map(|i| {
                    if arr.is_null(i) || lens.is_null(i) {
                        None
                    } else {
                        let s = arr.value(i);
                        let n = lens.value(i).max(0) as usize;
                        let skip = s.len().saturating_sub(n);
                        Some(s.chars().skip(skip).collect::<String>())
                    }
                })
                .collect();
            Ok(Arc::new(result))
        }
        "LPAD" => {
            check_arity(name, args, 3)?;
            let arr = as_string_array(&args[0])?;
            let lens = args[1].as_primitive::<Int64Type>();
            let pads = as_string_array(&args[2])?;
            let bytes = (0..num_rows).try_fold(0_u64, |total, index| {
                if arr.is_null(index) || lens.is_null(index) || pads.is_null(index) {
                    return Ok(total);
                }
                (lens.value(index).max(0) as u64)
                    .checked_mul(4)
                    .and_then(|bytes| bytes.checked_add(arr.value(index).len() as u64))
                    .and_then(|bytes| total.checked_add(bytes))
                    .ok_or_else(|| KaveonError::Execution("padding output size overflow".into()))
            })?;
            reserve_string_expansion(bytes, num_rows)?;
            let result: StringArray = (0..num_rows)
                .map(|i| {
                    if arr.is_null(i) || lens.is_null(i) || pads.is_null(i) {
                        return None;
                    }
                    let s = arr.value(i);
                    let target_len = lens.value(i).max(0) as usize;
                    let pad = pads.value(i);
                    if s.len() >= target_len {
                        Some(s.chars().take(target_len).collect())
                    } else if pad.is_empty() {
                        Some(s.to_owned())
                    } else {
                        let needed = target_len - s.len();
                        let prefix: String = pad.chars().cycle().take(needed).collect();
                        Some(format!("{prefix}{s}"))
                    }
                })
                .collect();
            Ok(Arc::new(result))
        }
        "RPAD" => {
            check_arity(name, args, 3)?;
            let arr = as_string_array(&args[0])?;
            let lens = args[1].as_primitive::<Int64Type>();
            let pads = as_string_array(&args[2])?;
            let bytes = (0..num_rows).try_fold(0_u64, |total, index| {
                if arr.is_null(index) || lens.is_null(index) || pads.is_null(index) {
                    return Ok(total);
                }
                (lens.value(index).max(0) as u64)
                    .checked_mul(4)
                    .and_then(|bytes| bytes.checked_add(arr.value(index).len() as u64))
                    .and_then(|bytes| total.checked_add(bytes))
                    .ok_or_else(|| KaveonError::Execution("padding output size overflow".into()))
            })?;
            reserve_string_expansion(bytes, num_rows)?;
            let result: StringArray = (0..num_rows)
                .map(|i| {
                    if arr.is_null(i) || lens.is_null(i) || pads.is_null(i) {
                        return None;
                    }
                    let s = arr.value(i);
                    let target_len = lens.value(i).max(0) as usize;
                    let pad = pads.value(i);
                    if s.len() >= target_len {
                        Some(s.chars().take(target_len).collect())
                    } else if pad.is_empty() {
                        Some(s.to_owned())
                    } else {
                        let needed = target_len - s.len();
                        let suffix: String = pad.chars().cycle().take(needed).collect();
                        Some(format!("{s}{suffix}"))
                    }
                })
                .collect();
            Ok(Arc::new(result))
        }
        "STARTS_WITH" => {
            check_arity(name, args, 2)?;
            let arr = as_string_array(&args[0])?;
            let prefix = as_string_array(&args[1])?;
            let result: BooleanArray = (0..num_rows)
                .map(|i| {
                    if arr.is_null(i) || prefix.is_null(i) {
                        None
                    } else {
                        Some(arr.value(i).starts_with(prefix.value(i)))
                    }
                })
                .collect();
            Ok(Arc::new(result))
        }
        "ENDS_WITH" => {
            check_arity(name, args, 2)?;
            let arr = as_string_array(&args[0])?;
            let suffix = as_string_array(&args[1])?;
            let result: BooleanArray = (0..num_rows)
                .map(|i| {
                    if arr.is_null(i) || suffix.is_null(i) {
                        None
                    } else {
                        Some(arr.value(i).ends_with(suffix.value(i)))
                    }
                })
                .collect();
            Ok(Arc::new(result))
        }
        "CONTAINS" => {
            check_arity(name, args, 2)?;
            if let Some(list) = args[0].as_any().downcast_ref::<LargeListArray>() {
                return match list.value_type() {
                    DataType::List(_)
                    | DataType::LargeList(_)
                    | DataType::Struct(_)
                    | DataType::Map(_, _) => contains_nested(list, &args[1]),
                    DataType::Int32 => contains_primitive::<_, Int32Type>(list, &args[1]),
                    DataType::Int64 => contains_primitive::<_, Int64Type>(list, &args[1]),
                    DataType::Float64 => contains_primitive::<_, Float64Type>(list, &args[1]),
                    DataType::Date32 => contains_primitive::<_, Date32Type>(list, &args[1]),
                    DataType::Date64 => contains_primitive::<_, Date64Type>(list, &args[1]),
                    DataType::Time32(TimeUnit::Second) =>
                        contains_primitive::<_, Time32SecondType>(list, &args[1]),
                    DataType::Time32(TimeUnit::Millisecond) =>
                        contains_primitive::<_, Time32MillisecondType>(list, &args[1]),
                    DataType::Timestamp(TimeUnit::Microsecond, _) =>
                        contains_primitive::<_, TimestampMicrosecondType>(list, &args[1]),
                    DataType::Boolean => contains_boolean(list, &args[1]),
                    DataType::Utf8 => contains_utf8(list, &args[1]),
                    other => Err(KaveonError::Execution(format!(
                        "CONTAINS does not support list element type {other}"
                    ))),
                };
            }
            if matches!(args[0].data_type(), DataType::List(_)) {
                let list = args[0]
                    .as_any()
                    .downcast_ref::<arrow::array::ListArray>()
                    .expect("list data type must be ListArray");
                if matches!(
                    list.value_type(),
                    DataType::List(_)
                        | DataType::LargeList(_)
                        | DataType::Struct(_)
                        | DataType::Map(_, _)
                ) {
                    let result = (0..num_rows)
                        .map(|row| {
                            if list.is_null(row) || args[1].is_null(row) {
                                return None;
                            }
                            let values = list.value(row);
                            let needle = args[1].slice(row, 1);
                            let mut saw_null = false;
                            for index in 0..values.len() {
                                if values.is_null(index) {
                                    saw_null = true;
                                    continue;
                                }
                                if comparison(&values.slice(index, 1), &needle, CompareKind::Eq)
                                    .map(|equal| !equal.is_null(0) && equal.value(0))
                                    .unwrap_or(false)
                                {
                                    return Some(true);
                                }
                            }
                            if saw_null { None } else { Some(false) }
                        })
                        .collect::<BooleanArray>();
                    return Ok(Arc::new(result));
                }
                macro_rules! contains_primitive {
                    ($ty:ty, $data_type:expr) => {{
                        let needle = compute::cast(&args[1], &$data_type)?;
                        let needle = needle.as_primitive::<$ty>();
                        let result: BooleanArray = (0..num_rows)
                            .map(|row| {
                                if list.is_null(row) || needle.is_null(row) {
                                    return None;
                                }
                                let inner = list.value(row);
                                let values = inner.as_primitive::<$ty>();
                                let mut saw_null = false;
                                for index in 0..values.len() {
                                    if values.is_null(index) {
                                        saw_null = true;
                                    } else if values.value(index) == needle.value(row) {
                                        return Some(true);
                                    }
                                }
                                if saw_null { None } else { Some(false) }
                            })
                            .collect();
                        return Ok(Arc::new(result));
                    }};
                }
                match list.value_type() {
                    DataType::Int64 => {
                        let needle = compute::cast(&args[1], &DataType::Int64)?;
                        let needle = needle.as_primitive::<Int64Type>();
                        let result: BooleanArray = (0..num_rows)
                            .map(|row| {
                                if list.is_null(row) || needle.is_null(row) {
                                    return None;
                                }
                                let values_array = list.value(row);
                                let values = values_array.as_primitive::<Int64Type>();
                                let mut saw_null = false;
                                for index in 0..values.len() {
                                    if values.is_null(index) {
                                        saw_null = true;
                                    } else if values.value(index) == needle.value(row) {
                                        return Some(true);
                                    }
                                }
                                if saw_null { None } else { Some(false) }
                            })
                            .collect();
                        return Ok(Arc::new(result));
                    }
                    DataType::Float64 => {
                        let needle = compute::cast(&args[1], &DataType::Float64)?;
                        let needle = needle.as_primitive::<Float64Type>();
                        let result: BooleanArray = (0..num_rows)
                            .map(|row| {
                                if list.is_null(row) || needle.is_null(row) {
                                    return None;
                                }
                                let values_array = list.value(row);
                                let values = values_array.as_primitive::<Float64Type>();
                                let mut saw_null = false;
                                for index in 0..values.len() {
                                    if values.is_null(index) {
                                        saw_null = true;
                                    } else if values.value(index) == needle.value(row) {
                                        return Some(true);
                                    }
                                }
                                if saw_null { None } else { Some(false) }
                            })
                            .collect();
                        return Ok(Arc::new(result));
                    }
                    DataType::Date32 => contains_primitive!(Date32Type, DataType::Date32),
                    DataType::Date64 => contains_primitive!(Date64Type, DataType::Date64),
                    DataType::Time32(TimeUnit::Second) => contains_primitive!(
                        Time32SecondType,
                        DataType::Time32(TimeUnit::Second)
                    ),
                    DataType::Time32(TimeUnit::Millisecond) => contains_primitive!(
                        Time32MillisecondType,
                        DataType::Time32(TimeUnit::Millisecond)
                    ),
                    DataType::Timestamp(TimeUnit::Microsecond, timezone) => contains_primitive!(
                        TimestampMicrosecondType,
                        DataType::Timestamp(TimeUnit::Microsecond, timezone.clone())
                    ),
                    DataType::Decimal128(precision, scale) => {
                        let needle = compute::cast(
                            &args[1],
                            &DataType::Decimal128(precision, scale),
                        )?;
                        let needle = needle.as_primitive::<arrow::datatypes::Decimal128Type>();
                        let result: BooleanArray = (0..num_rows)
                            .map(|row| {
                                if list.is_null(row) || needle.is_null(row) {
                                    return None;
                                }
                                let values = list.value(row);
                                let values = values
                                    .as_primitive::<arrow::datatypes::Decimal128Type>();
                                let mut saw_null = false;
                                for index in 0..values.len() {
                                    if values.is_null(index) {
                                        saw_null = true;
                                    } else if values.value(index) == needle.value(row) {
                                        return Some(true);
                                    }
                                }
                                if saw_null { None } else { Some(false) }
                            })
                            .collect();
                        return Ok(Arc::new(result));
                    }
                    DataType::Utf8 => {
                        let needle = compute::cast(&args[1], &DataType::Utf8)?;
                        let needle = needle.as_string::<i32>();
                        let result: BooleanArray = (0..num_rows)
                            .map(|row| {
                                if list.is_null(row) || needle.is_null(row) {
                                    return None;
                                }
                                let values_array = list.value(row);
                                let values = values_array.as_string::<i32>();
                                let mut saw_null = false;
                                for index in 0..values.len() {
                                    if values.is_null(index) {
                                        saw_null = true;
                                    } else if values.value(index) == needle.value(row) {
                                        return Some(true);
                                    }
                                }
                                if saw_null { None } else { Some(false) }
                            })
                            .collect();
                        return Ok(Arc::new(result));
                    }
                    DataType::Boolean => {
                        let needle = compute::cast(&args[1], &DataType::Boolean)?;
                        let needle = needle.as_boolean();
                        let result: BooleanArray = (0..num_rows)
                            .map(|row| {
                                if list.is_null(row) || needle.is_null(row) {
                                    return None;
                                }
                                let values = list.value(row);
                                let values = values.as_boolean();
                                let mut saw_null = false;
                                for index in 0..values.len() {
                                    if values.is_null(index) {
                                        saw_null = true;
                                    } else if values.value(index) == needle.value(row) {
                                        return Some(true);
                                    }
                                }
                                if saw_null { None } else { Some(false) }
                            })
                            .collect();
                        return Ok(Arc::new(result));
                    }
                    other => {
                        return Err(KaveonError::Execution(format!(
                            "CONTAINS does not support list element type {other}"
                        )));
                    }
                }
            }
            let arr = as_string_array(&args[0])?;
            let needle = as_string_array(&args[1])?;
            let result: BooleanArray = (0..num_rows)
                .map(|i| {
                    if arr.is_null(i) || needle.is_null(i) {
                        None
                    } else {
                        Some(arr.value(i).contains(needle.value(i)))
                    }
                })
                .collect();
            Ok(Arc::new(result))
        }
        "ARRAY_POSITION" => {
            check_arity(name, args, 2)?;
            if let Some(list) = args[0].as_any().downcast_ref::<LargeListArray>() {
                if matches!(
                    list.value_type(),
                    DataType::List(_)
                        | DataType::LargeList(_)
                        | DataType::Struct(_)
                        | DataType::Map(_, _)
                ) {
                    return array_position_nested(list, &args[1]);
                }
                return Err(KaveonError::Execution(format!(
                    "ARRAY_POSITION does not support list element type {}",
                    list.value_type()
                )));
            }
            let list = args[0]
                .as_any()
                .downcast_ref::<arrow::array::ListArray>()
                .ok_or_else(|| {
                    KaveonError::Execution(
                        "ARRAY_POSITION currently supports ARRAY/List values only".into(),
                    )
                })?;
            if matches!(
                list.value_type(),
                DataType::List(_)
                    | DataType::LargeList(_)
                    | DataType::Struct(_)
                    | DataType::Map(_, _)
            ) {
                return array_position_nested(list, &args[1]);
            }
            macro_rules! position_primitive {
                ($ty:ty, $data_type:expr) => {{
                    let needle = compute::cast(&args[1], &$data_type)?;
                    let needle = needle.as_primitive::<$ty>();
                    (0..num_rows)
                        .map(|row| {
                            if list.is_null(row) {
                                return None;
                            }
                            let inner = list.value(row);
                            let values = inner.as_primitive::<$ty>();
                            (0..values.len())
                                .find(|&index| {
                                    if needle.is_null(row) {
                                        values.is_null(index)
                                    } else {
                                        !values.is_null(index)
                                            && values.value(index) == needle.value(row)
                                    }
                                })
                                .map(|index| index as i64 + 1)
                                .or(Some(0))
                        })
                        .collect::<Vec<_>>()
                }};
            }
            let result = match list.value_type() {
                DataType::Int64 => {
                    let needle = compute::cast(&args[1], &DataType::Int64)?;
                    let needle = needle.as_primitive::<Int64Type>();
                    (0..num_rows)
                        .map(|row| {
                            if list.is_null(row) {
                                return None;
                            }
                            let values = list.value(row);
                            let values = values.as_primitive::<Int64Type>();
                            (0..values.len())
                                .find(|&index| {
                                    if needle.is_null(row) {
                                        values.is_null(index)
                                    } else {
                                        !values.is_null(index)
                                            && values.value(index) == needle.value(row)
                                    }
                                })
                                .map(|index| index as i64 + 1)
                                .or(Some(0))
                        })
                        .collect::<Vec<_>>()
                }
                DataType::Float64 => {
                    let needle = compute::cast(&args[1], &DataType::Float64)?;
                    let needle = needle.as_primitive::<Float64Type>();
                    (0..num_rows)
                        .map(|row| {
                            if list.is_null(row) {
                                return None;
                            }
                            let values = list.value(row);
                            let values = values.as_primitive::<Float64Type>();
                            (0..values.len())
                                .find(|&index| {
                                    if needle.is_null(row) {
                                        values.is_null(index)
                                    } else {
                                        !values.is_null(index)
                                            && values.value(index) == needle.value(row)
                                    }
                                })
                                .map(|index| index as i64 + 1)
                                .or(Some(0))
                        })
                        .collect::<Vec<_>>()
                }
                DataType::Date32 => position_primitive!(Date32Type, DataType::Date32),
                DataType::Date64 => position_primitive!(Date64Type, DataType::Date64),
                DataType::Time32(TimeUnit::Second) => position_primitive!(
                    Time32SecondType,
                    DataType::Time32(TimeUnit::Second)
                ),
                DataType::Time32(TimeUnit::Millisecond) => position_primitive!(
                    Time32MillisecondType,
                    DataType::Time32(TimeUnit::Millisecond)
                ),
                DataType::Timestamp(TimeUnit::Microsecond, timezone) => position_primitive!(
                    TimestampMicrosecondType,
                    DataType::Timestamp(TimeUnit::Microsecond, timezone.clone())
                ),
                DataType::Decimal128(precision, scale) => {
                    let needle = compute::cast(
                        &args[1],
                        &DataType::Decimal128(precision, scale),
                    )?;
                    let needle = needle.as_primitive::<arrow::datatypes::Decimal128Type>();
                    (0..num_rows)
                        .map(|row| {
                            if list.is_null(row) {
                                return None;
                            }
                            let values = list.value(row);
                            let values = values
                                .as_primitive::<arrow::datatypes::Decimal128Type>();
                            (0..values.len())
                                .find(|&index| {
                                    if needle.is_null(row) {
                                        values.is_null(index)
                                    } else {
                                        !values.is_null(index)
                                            && values.value(index) == needle.value(row)
                                    }
                                })
                                .map(|index| index as i64 + 1)
                                .or(Some(0))
                        })
                        .collect::<Vec<_>>()
                }
                DataType::Utf8 => {
                    let needle = compute::cast(&args[1], &DataType::Utf8)?;
                    let needle = needle.as_string::<i32>();
                    (0..num_rows)
                        .map(|row| {
                            if list.is_null(row) {
                                return None;
                            }
                            let values = list.value(row);
                            let values = values.as_string::<i32>();
                            (0..values.len())
                                .find(|&index| {
                                    if needle.is_null(row) {
                                        values.is_null(index)
                                    } else {
                                        !values.is_null(index)
                                            && values.value(index) == needle.value(row)
                                    }
                                })
                                .map(|index| index as i64 + 1)
                                .or(Some(0))
                        })
                        .collect::<Vec<_>>()
                }
                DataType::Boolean => {
                    let needle = compute::cast(&args[1], &DataType::Boolean)?;
                    let needle = needle.as_boolean();
                    (0..num_rows)
                        .map(|row| {
                            if list.is_null(row) {
                                return None;
                            }
                            let values = list.value(row);
                            let values = values.as_boolean();
                            (0..values.len())
                                .find(|&index| {
                                    if needle.is_null(row) {
                                        values.is_null(index)
                                    } else {
                                        !values.is_null(index)
                                            && values.value(index) == needle.value(row)
                                    }
                                })
                                .map(|index| index as i64 + 1)
                                .or(Some(0))
                        })
                        .collect::<Vec<_>>()
                }
                other => {
                    return Err(KaveonError::Execution(format!(
                        "ARRAY_POSITION does not support list element type {other}"
                    )));
                }
            };
            Ok(Arc::new(Int64Array::from(result)))
        }
        "ARRAY_DISTINCT" => {
            check_arity(name, args, 1)?;
            if let Some(list) = args[0].as_any().downcast_ref::<LargeListArray>() {
                if matches!(
                    list.value_type(),
                    DataType::List(_)
                        | DataType::LargeList(_)
                        | DataType::Struct(_)
                        | DataType::Map(_, _)
                ) {
                    return array_distinct_nested(list);
                }
                return Err(KaveonError::Execution(format!(
                    "ARRAY_DISTINCT does not support list element type {}",
                    list.value_type()
                )));
            }
            let list = args[0]
                .as_any()
                .downcast_ref::<ListArray>()
                .ok_or_else(|| KaveonError::Execution("ARRAY_DISTINCT requires an ARRAY/List value".into()))?;
            if matches!(
                list.value_type(),
                DataType::List(_)
                    | DataType::LargeList(_)
                    | DataType::Struct(_)
                    | DataType::Map(_, _)
            ) {
                return array_distinct_nested(list);
            }
            macro_rules! distinct_primitive {
                ($ty:ty, $builder:expr) => {{
                    let mut builder = ListBuilder::new($builder);
                    for row in 0..num_rows {
                        if list.is_null(row) {
                            builder.append(false);
                            continue;
                        }
                        let inner = list.value(row);
                        let values = inner.as_primitive::<$ty>();
                        let mut seen = Vec::new();
                        for index in 0..values.len() {
                            let value = values.is_valid(index).then(|| values.value(index));
                            if !seen.contains(&value) {
                                seen.push(value);
                                builder.values().append_option(value);
                            }
                        }
                        builder.append(true);
                    }
                    Ok(Arc::new(builder.finish()))
                }};
            }
            match list.value_type() {
                DataType::Int32 => distinct_primitive!(
                    Int32Type,
                    PrimitiveBuilder::<Int32Type>::new()
                ),
                DataType::Int64 => {
                    let mut builder = ListBuilder::new(Int64Builder::new());
                    for row in 0..num_rows {
                        if list.is_null(row) {
                            builder.append(false);
                            continue;
                        }
                        let values = list.value(row).as_primitive::<Int64Type>().clone();
                        let mut seen = Vec::<Option<i64>>::new();
                        for index in 0..values.len() {
                            let value = values.is_valid(index).then(|| values.value(index));
                            if !seen.contains(&value) {
                                seen.push(value);
                                builder.values().append_option(value);
                            }
                        }
                        builder.append(true);
                    }
                    Ok(Arc::new(builder.finish()))
                }
                DataType::Float64 => {
                    let mut builder = ListBuilder::new(Float64Builder::new());
                    for row in 0..num_rows {
                        if list.is_null(row) {
                            builder.append(false);
                            continue;
                        }
                        let values = list.value(row).as_primitive::<Float64Type>().clone();
                        let mut seen = Vec::<Option<f64>>::new();
                        for index in 0..values.len() {
                            let value = values.is_valid(index).then(|| values.value(index));
                            if !seen.iter().any(|existing| match (existing, value) {
                                (None, None) => true,
                                (Some(left), Some(right)) => left == &right,
                                _ => false,
                            }) {
                                seen.push(value);
                                builder.values().append_option(value);
                            }
                        }
                        builder.append(true);
                    }
                    Ok(Arc::new(builder.finish()))
                }
                DataType::Date32 => distinct_primitive!(
                    Date32Type,
                    PrimitiveBuilder::<Date32Type>::new()
                ),
                DataType::Date64 => distinct_primitive!(
                    Date64Type,
                    PrimitiveBuilder::<Date64Type>::new()
                ),
                DataType::Time32(TimeUnit::Second) => distinct_primitive!(
                    Time32SecondType,
                    PrimitiveBuilder::<Time32SecondType>::new()
                ),
                DataType::Time32(TimeUnit::Millisecond) => distinct_primitive!(
                    Time32MillisecondType,
                    PrimitiveBuilder::<Time32MillisecondType>::new()
                ),
                DataType::Timestamp(TimeUnit::Microsecond, _) => distinct_primitive!(
                    TimestampMicrosecondType,
                    PrimitiveBuilder::<TimestampMicrosecondType>::new()
                ),
                DataType::Decimal128(precision, scale) => {
                    let mut builder = ListBuilder::new(
                        Decimal128Builder::new()
                            .with_precision_and_scale(precision, scale)
                            .map_err(|error| KaveonError::Execution(error.to_string()))?,
                    );
                    for row in 0..num_rows {
                        if list.is_null(row) {
                            builder.append(false);
                            continue;
                        }
                        let values = list
                            .value(row)
                            .as_primitive::<arrow::datatypes::Decimal128Type>()
                            .clone();
                        let mut seen = Vec::<Option<i128>>::new();
                        for index in 0..values.len() {
                            let value = values.is_valid(index).then(|| values.value(index));
                            if !seen.contains(&value) {
                                seen.push(value);
                                builder.values().append_option(value);
                            }
                        }
                        builder.append(true);
                    }
                    Ok(Arc::new(builder.finish()))
                }
                DataType::Utf8 => {
                    let mut builder = ListBuilder::new(StringBuilder::new());
                    for row in 0..num_rows {
                        if list.is_null(row) {
                            builder.append(false);
                            continue;
                        }
                        let values = list.value(row).as_string::<i32>().clone();
                        let mut seen = Vec::<Option<String>>::new();
                        for index in 0..values.len() {
                            let value = values.is_valid(index).then(|| values.value(index).to_owned());
                            if !seen.iter().any(|existing| existing == &value) {
                                seen.push(value.clone());
                                builder.values().append_option(value.as_deref());
                            }
                        }
                        builder.append(true);
                    }
                    Ok(Arc::new(builder.finish()))
                }
                DataType::Boolean => {
                    let mut builder = ListBuilder::new(BooleanBuilder::new());
                    for row in 0..num_rows {
                        if list.is_null(row) {
                            builder.append(false);
                            continue;
                        }
                        let values = list.value(row).as_boolean().clone();
                        let mut seen = Vec::<Option<bool>>::new();
                        for index in 0..values.len() {
                            let value = values.is_valid(index).then(|| values.value(index));
                            if !seen.contains(&value) {
                                seen.push(value);
                                builder.values().append_option(value);
                            }
                        }
                        builder.append(true);
                    }
                    Ok(Arc::new(builder.finish()))
                }
                other => Err(KaveonError::Execution(format!(
                    "ARRAY_DISTINCT does not support list element type {other}"
                ))),
            }
        }
        "ARRAY_SORT" => {
            check_arity(name, args, 1)?;
            if let Some(list) = args[0].as_any().downcast_ref::<LargeListArray>() {
                return array_sort_generic(list);
            }
            let list = args[0]
                .as_any()
                .downcast_ref::<ListArray>()
                .ok_or_else(|| KaveonError::Execution("ARRAY_SORT requires an ARRAY/List value".into()))?;
            macro_rules! sort_primitive {
                ($ty:ty, $builder:expr) => {{
                    let mut builder = ListBuilder::new($builder);
                    for row in 0..num_rows {
                        if list.is_null(row) {
                            builder.append(false);
                            continue;
                        }
                        let inner = list.value(row);
                        let values = inner.as_primitive::<$ty>();
                        let mut sorted = (0..values.len())
                            .map(|index| values.is_valid(index).then(|| values.value(index)))
                            .collect::<Vec<_>>();
                        sorted.sort_by(|left, right| match (left, right) {
                            (None, None) => std::cmp::Ordering::Equal,
                            (None, Some(_)) => std::cmp::Ordering::Greater,
                            (Some(_), None) => std::cmp::Ordering::Less,
                            (Some(left), Some(right)) => left.cmp(right),
                        });
                        for value in sorted {
                            builder.values().append_option(value);
                        }
                        builder.append(true);
                    }
                    Ok(Arc::new(builder.finish()))
                }};
            }
            match list.value_type() {
                DataType::Int64 => {
                    let mut builder = ListBuilder::new(Int64Builder::new());
                    for row in 0..num_rows {
                        if list.is_null(row) {
                            builder.append(false);
                            continue;
                        }
                        let inner = list.value(row);
                        let values = inner.as_primitive::<Int64Type>();
                        let mut sorted = (0..values.len())
                            .map(|index| values.is_valid(index).then(|| values.value(index)))
                            .collect::<Vec<_>>();
                        sorted.sort_by(|left, right| match (left, right) {
                            (None, None) => std::cmp::Ordering::Equal,
                            (None, Some(_)) => std::cmp::Ordering::Greater,
                            (Some(_), None) => std::cmp::Ordering::Less,
                            (Some(left), Some(right)) => left.cmp(right),
                        });
                        for value in sorted {
                            builder.values().append_option(value);
                        }
                        builder.append(true);
                    }
                    Ok(Arc::new(builder.finish()))
                }
                DataType::Float64 => {
                    let mut builder = ListBuilder::new(Float64Builder::new());
                    for row in 0..num_rows {
                        if list.is_null(row) {
                            builder.append(false);
                            continue;
                        }
                        let inner = list.value(row);
                        let values = inner.as_primitive::<Float64Type>();
                        let mut sorted = (0..values.len())
                            .map(|index| values.is_valid(index).then(|| values.value(index)))
                            .collect::<Vec<_>>();
                        sorted.sort_by(|left, right| match (left, right) {
                            (None, None) => std::cmp::Ordering::Equal,
                            (None, Some(_)) => std::cmp::Ordering::Greater,
                            (Some(_), None) => std::cmp::Ordering::Less,
                            (Some(left), Some(right)) => left
                                .partial_cmp(right)
                                .unwrap_or(std::cmp::Ordering::Equal),
                        });
                        for value in sorted {
                            builder.values().append_option(value);
                        }
                        builder.append(true);
                    }
                    Ok(Arc::new(builder.finish()))
                }
                DataType::Date32 => sort_primitive!(
                    Date32Type,
                    PrimitiveBuilder::<Date32Type>::new()
                ),
                DataType::Date64 => sort_primitive!(
                    Date64Type,
                    PrimitiveBuilder::<Date64Type>::new()
                ),
                DataType::Time32(TimeUnit::Second) => sort_primitive!(
                    Time32SecondType,
                    PrimitiveBuilder::<Time32SecondType>::new()
                ),
                DataType::Time32(TimeUnit::Millisecond) => sort_primitive!(
                    Time32MillisecondType,
                    PrimitiveBuilder::<Time32MillisecondType>::new()
                ),
                DataType::Timestamp(TimeUnit::Microsecond, _) => sort_primitive!(
                    TimestampMicrosecondType,
                    PrimitiveBuilder::<TimestampMicrosecondType>::new()
                ),
                DataType::Decimal128(precision, scale) => {
                    let mut builder = ListBuilder::new(
                        Decimal128Builder::new()
                            .with_precision_and_scale(precision, scale)
                            .map_err(|error| KaveonError::Execution(error.to_string()))?,
                    );
                    for row in 0..num_rows {
                        if list.is_null(row) {
                            builder.append(false);
                            continue;
                        }
                        let inner = list.value(row);
                        let values = inner.as_primitive::<arrow::datatypes::Decimal128Type>();
                        let mut sorted = (0..values.len())
                            .map(|index| values.is_valid(index).then(|| values.value(index)))
                            .collect::<Vec<_>>();
                        sorted.sort_by(|left, right| match (left, right) {
                            (None, None) => std::cmp::Ordering::Equal,
                            (None, Some(_)) => std::cmp::Ordering::Greater,
                            (Some(_), None) => std::cmp::Ordering::Less,
                            (Some(left), Some(right)) => left.cmp(right),
                        });
                        for value in sorted {
                            builder.values().append_option(value);
                        }
                        builder.append(true);
                    }
                    Ok(Arc::new(builder.finish()))
                }
                DataType::Utf8 => {
                    let mut builder = ListBuilder::new(StringBuilder::new());
                    for row in 0..num_rows {
                        if list.is_null(row) {
                            builder.append(false);
                            continue;
                        }
                        let inner = list.value(row);
                        let values = inner.as_string::<i32>();
                        let mut sorted = (0..values.len())
                            .map(|index| values.is_valid(index).then(|| values.value(index).to_owned()))
                            .collect::<Vec<_>>();
                        sorted.sort_by(|left, right| match (left, right) {
                            (None, None) => std::cmp::Ordering::Equal,
                            (None, Some(_)) => std::cmp::Ordering::Greater,
                            (Some(_), None) => std::cmp::Ordering::Less,
                            (Some(left), Some(right)) => left.cmp(right),
                        });
                        for value in sorted {
                            builder.values().append_option(value.as_deref());
                        }
                        builder.append(true);
                    }
                    Ok(Arc::new(builder.finish()))
                }
                DataType::Boolean => {
                    let mut builder = ListBuilder::new(BooleanBuilder::new());
                    for row in 0..num_rows {
                        if list.is_null(row) {
                            builder.append(false);
                            continue;
                        }
                        let inner = list.value(row);
                        let values = inner.as_boolean();
                        let mut sorted = (0..values.len())
                            .map(|index| values.is_valid(index).then(|| values.value(index)))
                            .collect::<Vec<_>>();
                        sorted.sort_by(|left, right| match (left, right) {
                            (None, None) => std::cmp::Ordering::Equal,
                            (None, Some(_)) => std::cmp::Ordering::Greater,
                            (Some(_), None) => std::cmp::Ordering::Less,
                            (Some(left), Some(right)) => left.cmp(right),
                        });
                        for value in sorted {
                            builder.values().append_option(value);
                        }
                        builder.append(true);
                    }
                    Ok(Arc::new(builder.finish()))
                }
                other => Err(KaveonError::Execution(format!(
                    "ARRAY_SORT does not support list element type {other}"
                ))),
            }
        }
        "ARRAY_REVERSE" => {
            check_arity(name, args, 1)?;
            if let Some(list) = args[0].as_any().downcast_ref::<LargeListArray>() {
                return array_reverse_generic(list);
            }
            let list = args[0]
                .as_any()
                .downcast_ref::<ListArray>()
                .ok_or_else(|| KaveonError::Execution("ARRAY_REVERSE requires an ARRAY/List value".into()))?;
            if matches!(
                list.value_type(),
                DataType::List(_)
                    | DataType::LargeList(_)
                    | DataType::Struct(_)
                    | DataType::Map(_, _)
            ) {
                return array_reverse_generic(list);
            }
            macro_rules! reverse_primitive {
                ($ty:ty, $builder:expr) => {{
                    let mut builder = ListBuilder::new($builder);
                    for row in 0..num_rows {
                        if list.is_null(row) {
                            builder.append(false);
                            continue;
                        }
                        let inner = list.value(row);
                        let values = inner.as_primitive::<$ty>();
                        for index in (0..values.len()).rev() {
                            builder
                                .values()
                                .append_option(values.is_valid(index).then(|| values.value(index)));
                        }
                        builder.append(true);
                    }
                    Ok(Arc::new(builder.finish()))
                }};
            }
            match list.value_type() {
                DataType::Int64 => {
                    let mut builder = ListBuilder::new(Int64Builder::new());
                    for row in 0..num_rows {
                        if list.is_null(row) {
                            builder.append(false);
                            continue;
                        }
                        let inner = list.value(row);
                        let values = inner.as_primitive::<Int64Type>();
                        for index in (0..values.len()).rev() {
                            builder
                                .values()
                                .append_option(values.is_valid(index).then(|| values.value(index)));
                        }
                        builder.append(true);
                    }
                    Ok(Arc::new(builder.finish()))
                }
                DataType::Float64 => {
                    let mut builder = ListBuilder::new(Float64Builder::new());
                    for row in 0..num_rows {
                        if list.is_null(row) {
                            builder.append(false);
                            continue;
                        }
                        let inner = list.value(row);
                        let values = inner.as_primitive::<Float64Type>();
                        for index in (0..values.len()).rev() {
                            builder.values().append_option(values.is_valid(index).then(|| values.value(index)));
                        }
                        builder.append(true);
                    }
                    Ok(Arc::new(builder.finish()))
                }
                DataType::Date32 => reverse_primitive!(
                    Date32Type,
                    PrimitiveBuilder::<Date32Type>::new()
                ),
                DataType::Date64 => reverse_primitive!(
                    Date64Type,
                    PrimitiveBuilder::<Date64Type>::new()
                ),
                DataType::Time32(TimeUnit::Second) => reverse_primitive!(
                    Time32SecondType,
                    PrimitiveBuilder::<Time32SecondType>::new()
                ),
                DataType::Time32(TimeUnit::Millisecond) => reverse_primitive!(
                    Time32MillisecondType,
                    PrimitiveBuilder::<Time32MillisecondType>::new()
                ),
                DataType::Timestamp(TimeUnit::Microsecond, _) => reverse_primitive!(
                    TimestampMicrosecondType,
                    PrimitiveBuilder::<TimestampMicrosecondType>::new()
                ),
                DataType::Decimal128(precision, scale) => {
                    let values_builder = Decimal128Builder::new()
                        .with_precision_and_scale(precision, scale)
                        .map_err(|error| KaveonError::Execution(error.to_string()))?;
                    let mut builder = ListBuilder::new(values_builder);
                    for row in 0..num_rows {
                        if list.is_null(row) {
                            builder.append(false);
                            continue;
                        }
                        let inner = list.value(row);
                        let values = inner.as_primitive::<arrow::datatypes::Decimal128Type>();
                        for index in (0..values.len()).rev() {
                            builder.values().append_option(
                                values.is_valid(index).then(|| values.value(index)),
                            );
                        }
                        builder.append(true);
                    }
                    Ok(Arc::new(builder.finish()))
                }
                DataType::Utf8 => {
                    let mut builder = ListBuilder::new(StringBuilder::new());
                    for row in 0..num_rows {
                        if list.is_null(row) {
                            builder.append(false);
                            continue;
                        }
                        let inner = list.value(row);
                        let values = inner.as_string::<i32>();
                        for index in (0..values.len()).rev() {
                            builder.values().append_option(values.is_valid(index).then(|| values.value(index)));
                        }
                        builder.append(true);
                    }
                    Ok(Arc::new(builder.finish()))
                }
                DataType::Boolean => {
                    let mut builder = ListBuilder::new(BooleanBuilder::new());
                    for row in 0..num_rows {
                        if list.is_null(row) {
                            builder.append(false);
                            continue;
                        }
                        let inner = list.value(row);
                        let values = inner.as_boolean();
                        for index in (0..values.len()).rev() {
                            builder.values().append_option(values.is_valid(index).then(|| values.value(index)));
                        }
                        builder.append(true);
                    }
                    Ok(Arc::new(builder.finish()))
                }
                other => Err(KaveonError::Execution(format!(
                    "ARRAY_REVERSE does not support list element type {other}"
                ))),
            }
        }
        "ARRAY_SLICE" => {
            check_arity(name, args, 4)?;
            let lower = compute::cast(&args[1], &DataType::Int64)?;
            let upper = compute::cast(&args[2], &DataType::Int64)?;
            let stride = compute::cast(&args[3], &DataType::Int64)?;
            let lower = lower.as_primitive::<Int64Type>();
            let upper = upper.as_primitive::<Int64Type>();
            let stride = stride.as_primitive::<Int64Type>();
            if let Some(list) = args[0].as_any().downcast_ref::<LargeListArray>() {
                return array_slice_generic(list, lower, upper, stride);
            }
            let list = args[0]
                .as_any()
                .downcast_ref::<ListArray>()
                .ok_or_else(|| KaveonError::Execution("ARRAY_SLICE requires an ARRAY/List value".into()))?;
            if matches!(
                list.value_type(),
                DataType::List(_)
                    | DataType::LargeList(_)
                    | DataType::Struct(_)
                    | DataType::Map(_, _)
            ) {
                return array_slice_generic(list, lower, upper, stride);
            }
            macro_rules! slice_primitive {
                ($ty:ty, $builder:expr) => {{
                    let mut builder = ListBuilder::new($builder);
                    for row in 0..num_rows {
                        if list.is_null(row) {
                            builder.append(false);
                            continue;
                        }
                        let inner = list.value(row);
                        let values = inner.as_primitive::<$ty>();
                        let length = values.len() as i64;
                        let step = stride.is_valid(row).then(|| stride.value(row)).unwrap_or(1);
                        let start = lower
                            .is_valid(row)
                            .then(|| lower.value(row))
                            .unwrap_or(if step < 0 { length } else { 1 });
                        let end = upper
                            .is_valid(row)
                            .then(|| upper.value(row))
                            .unwrap_or(if step < 0 { 1 } else { length });
                        if step == 0 {
                            return Err(KaveonError::Execution("ARRAY_SLICE stride cannot be zero".into()));
                        }
                        let first = if step > 0 { start.max(1).min(length + 1) } else { start.max(1).min(length) };
                        let last = if step > 0 { end.max(0).min(length) } else { end.max(1).min(length) };
                        if (step > 0 && first <= last) || (step < 0 && first >= last) {
                            let mut index = first;
                            while (step > 0 && index <= last) || (step < 0 && index >= last) {
                                let offset = (index - 1) as usize;
                                builder.values().append_option(
                                    values.is_valid(offset).then(|| values.value(offset)),
                                );
                                index += step;
                            }
                        }
                        builder.append(true);
                    }
                    Ok(Arc::new(builder.finish()))
                }};
            }
            match list.value_type() {
                DataType::Int64 => {
                    let mut builder = ListBuilder::new(Int64Builder::new());
                    for row in 0..num_rows {
                        if list.is_null(row) {
                            builder.append(false);
                            continue;
                        }
                        let inner = list.value(row);
                        let values = inner.as_primitive::<Int64Type>();
                        let length = values.len() as i64;
                        let step = stride.is_valid(row).then(|| stride.value(row)).unwrap_or(1);
                        let start = lower
                            .is_valid(row)
                            .then(|| lower.value(row))
                            .unwrap_or(if step < 0 { length } else { 1 });
                        let end = upper
                            .is_valid(row)
                            .then(|| upper.value(row))
                            .unwrap_or(if step < 0 { 1 } else { length });
                        if step == 0 {
                            return Err(KaveonError::Execution("ARRAY_SLICE stride cannot be zero".into()));
                        }
                        let first = if step > 0 { start.max(1).min(length + 1) } else { start.max(1).min(length) };
                        let last = if step > 0 { end.max(0).min(length) } else { end.max(1).min(length) };
                        if (step > 0 && first <= last) || (step < 0 && first >= last) {
                            let mut index = first;
                            while (step > 0 && index <= last) || (step < 0 && index >= last) {
                                let offset = (index - 1) as usize;
                                builder.values().append_option(values.is_valid(offset).then(|| values.value(offset)));
                                index += step;
                            }
                        }
                        builder.append(true);
                    }
                    Ok(Arc::new(builder.finish()))
                }
                DataType::Float64 => {
                    let mut builder = ListBuilder::new(Float64Builder::new());
                    for row in 0..num_rows {
                        if list.is_null(row) {
                            builder.append(false);
                            continue;
                        }
                        let values = list.value(row);
                        let values = values.as_primitive::<Float64Type>();
                        let length = values.len() as i64;
                        let step = stride.is_valid(row).then(|| stride.value(row)).unwrap_or(1);
                        let start = lower
                            .is_valid(row)
                            .then(|| lower.value(row))
                            .unwrap_or(if step < 0 { length } else { 1 });
                        let end = upper
                            .is_valid(row)
                            .then(|| upper.value(row))
                            .unwrap_or(if step < 0 { 1 } else { length });
                        if step == 0 {
                            return Err(KaveonError::Execution("ARRAY_SLICE stride cannot be zero".into()));
                        }
                        let first = if step > 0 {
                            start.max(1).min(length + 1)
                        } else {
                            start.max(1).min(length)
                        };
                        let last = if step > 0 {
                            end.max(0).min(length)
                        } else {
                            end.max(1).min(length)
                        };
                        if (step > 0 && first <= last) || (step < 0 && first >= last) {
                            let mut index = first;
                            while (step > 0 && index <= last) || (step < 0 && index >= last) {
                                let offset = (index - 1) as usize;
                                builder.values().append_option(values.is_valid(offset).then(|| values.value(offset)));
                                index += step;
                            }
                        }
                        builder.append(true);
                    }
                    Ok(Arc::new(builder.finish()))
                }
                DataType::Date32 => slice_primitive!(
                    Date32Type,
                    PrimitiveBuilder::<Date32Type>::new()
                ),
                DataType::Date64 => slice_primitive!(
                    Date64Type,
                    PrimitiveBuilder::<Date64Type>::new()
                ),
                DataType::Time32(TimeUnit::Second) => slice_primitive!(
                    Time32SecondType,
                    PrimitiveBuilder::<Time32SecondType>::new()
                ),
                DataType::Time32(TimeUnit::Millisecond) => slice_primitive!(
                    Time32MillisecondType,
                    PrimitiveBuilder::<Time32MillisecondType>::new()
                ),
                DataType::Timestamp(TimeUnit::Microsecond, _) => slice_primitive!(
                    TimestampMicrosecondType,
                    PrimitiveBuilder::<TimestampMicrosecondType>::new()
                ),
                DataType::Decimal128(precision, scale) => {
                    let values_builder = Decimal128Builder::new()
                        .with_precision_and_scale(precision, scale)
                        .map_err(|error| KaveonError::Execution(error.to_string()))?;
                    let mut builder = ListBuilder::new(values_builder);
                    for row in 0..num_rows {
                        if list.is_null(row) {
                            builder.append(false);
                            continue;
                        }
                        let inner = list.value(row);
                        let values = inner.as_primitive::<arrow::datatypes::Decimal128Type>();
                        let length = values.len() as i64;
                        let step = stride.is_valid(row).then(|| stride.value(row)).unwrap_or(1);
                        let start = lower
                            .is_valid(row)
                            .then(|| lower.value(row))
                            .unwrap_or(if step < 0 { length } else { 1 });
                        let end = upper
                            .is_valid(row)
                            .then(|| upper.value(row))
                            .unwrap_or(if step < 0 { 1 } else { length });
                        if step == 0 {
                            return Err(KaveonError::Execution("ARRAY_SLICE stride cannot be zero".into()));
                        }
                        let first = if step > 0 { start.max(1).min(length + 1) } else { start.max(1).min(length) };
                        let last = if step > 0 { end.max(0).min(length) } else { end.max(1).min(length) };
                        if (step > 0 && first <= last) || (step < 0 && first >= last) {
                            let mut index = first;
                            while (step > 0 && index <= last) || (step < 0 && index >= last) {
                                let offset = (index - 1) as usize;
                                builder.values().append_option(
                                    values.is_valid(offset).then(|| values.value(offset)),
                                );
                                index += step;
                            }
                        }
                        builder.append(true);
                    }
                    Ok(Arc::new(builder.finish()))
                }
                DataType::Utf8 => {
                    let mut builder = ListBuilder::new(StringBuilder::new());
                    for row in 0..num_rows {
                        if list.is_null(row) {
                            builder.append(false);
                            continue;
                        }
                        let values = list.value(row);
                        let values = values.as_string::<i32>();
                        let length = values.len() as i64;
                        let step = stride.is_valid(row).then(|| stride.value(row)).unwrap_or(1);
                        let start = lower
                            .is_valid(row)
                            .then(|| lower.value(row))
                            .unwrap_or(if step < 0 { length } else { 1 });
                        let end = upper
                            .is_valid(row)
                            .then(|| upper.value(row))
                            .unwrap_or(if step < 0 { 1 } else { length });
                        if step == 0 {
                            return Err(KaveonError::Execution("ARRAY_SLICE stride cannot be zero".into()));
                        }
                        let first = if step > 0 {
                            start.max(1).min(length + 1)
                        } else {
                            start.max(1).min(length)
                        };
                        let last = if step > 0 {
                            end.max(0).min(length)
                        } else {
                            end.max(1).min(length)
                        };
                        if (step > 0 && first <= last) || (step < 0 && first >= last) {
                            let mut index = first;
                            while (step > 0 && index <= last) || (step < 0 && index >= last) {
                                let offset = (index - 1) as usize;
                                builder.values().append_option(values.is_valid(offset).then(|| values.value(offset)));
                                index += step;
                            }
                        }
                        builder.append(true);
                    }
                    Ok(Arc::new(builder.finish()))
                }
                DataType::Boolean => {
                    let mut builder = ListBuilder::new(BooleanBuilder::new());
                    for row in 0..num_rows {
                        if list.is_null(row) {
                            builder.append(false);
                            continue;
                        }
                        let values = list.value(row);
                        let values = values.as_boolean();
                        let length = values.len() as i64;
                        let step = stride.is_valid(row).then(|| stride.value(row)).unwrap_or(1);
                        let start = lower.is_valid(row).then(|| lower.value(row)).unwrap_or(if step < 0 { length } else { 1 });
                        let end = upper.is_valid(row).then(|| upper.value(row)).unwrap_or(if step < 0 { 1 } else { length });
                        if step == 0 {
                            return Err(KaveonError::Execution("ARRAY_SLICE stride cannot be zero".into()));
                        }
                        let first = if step > 0 { start.max(1).min(length + 1) } else { start.max(1).min(length) };
                        let last = if step > 0 { end.max(0).min(length) } else { end.max(1).min(length) };
                        if (step > 0 && first <= last) || (step < 0 && first >= last) {
                            let mut index = first;
                            while (step > 0 && index <= last) || (step < 0 && index >= last) {
                                let offset = (index - 1) as usize;
                                builder.values().append_option(values.is_valid(offset).then(|| values.value(offset)));
                                index += step;
                            }
                        }
                        builder.append(true);
                    }
                    Ok(Arc::new(builder.finish()))
                }
                other => Err(KaveonError::Execution(format!("ARRAY_SLICE does not support list element type {other}"))),
            }
        }
        "ELEMENT_AT" => {
            check_arity(name, args, 2)?;
            if let Some(map) = args[0].as_any().downcast_ref::<MapArray>() {
                let map_keys = map.keys();
                let requested = compute::cast(&args[1], map_keys.data_type())?;
                let mut positions = Vec::with_capacity(num_rows);
                for row in 0..num_rows {
                    if map.is_null(row) || requested.is_null(row) {
                        positions.push(None);
                        continue;
                    }
                    let start = map.value_offsets()[row] as usize;
                    let end = map.value_offsets()[row + 1] as usize;
                    let found = match map_keys.data_type() {
                        DataType::Float64 => {
                            let keys = map_keys.as_primitive::<Float64Type>();
                            let needle = requested.as_primitive::<Float64Type>().value(row);
                            (start..end)
                                .find(|&index| !keys.is_null(index) && keys.value(index) == needle)
                        }
                        DataType::Int64 => {
                            let keys = map_keys.as_primitive::<Int64Type>();
                            let needle = requested.as_primitive::<Int64Type>().value(row);
                            (start..end)
                                .find(|&index| !keys.is_null(index) && keys.value(index) == needle)
                        }
                        DataType::Utf8 => {
                            let keys = map_keys.as_string::<i32>();
                            let needle = requested.as_string::<i32>().value(row);
                            (start..end)
                                .find(|&index| !keys.is_null(index) && keys.value(index) == needle)
                        }
                        other => {
                            return Err(KaveonError::Execution(format!(
                                "ELEMENT_AT does not support MAP key type {other}"
                            )));
                        }
                    };
                    positions.push(found.map(|index| index as i64));
                }
                return Ok(compute::take(
                    map.values().as_ref(),
                    &Int64Array::from(positions),
                    None,
                )?);
            }
            let list = args[0]
                .as_any()
                .downcast_ref::<ListArray>()
                .ok_or_else(|| {
                    KaveonError::Execution(
                        "ELEMENT_AT currently supports ARRAY/List values only".into(),
                    )
                })?;
            let indices = compute::cast(&args[1], &DataType::Int64)?;
            let indices = indices.as_primitive::<Int64Type>();
            let mut positions = Vec::with_capacity(num_rows);
            for row in 0..num_rows {
                if indices.is_null(row) {
                    positions.push(None);
                    continue;
                }
                let index = indices.value(row);
                if list.is_null(row) || index == 0 {
                    positions.push(None);
                    continue;
                }
                let start = list.value_offsets()[row] as i64;
                let end = list.value_offsets()[row + 1] as i64;
                let length = end - start;
                let offset = if index > 0 { index - 1 } else { length + index };
                positions.push((0..length).contains(&offset).then_some(start + offset));
            }
            Ok(compute::take(
                list.values().as_ref(),
                &Int64Array::from(positions),
                None,
            )?)
        }
        "STRPOS" => {
            if args.len() != 2 && args.len() != 3 {
                return Err(KaveonError::Execution(
                    "STRPOS requires 2 or 3 arguments".into(),
                ));
            }
            let arr = as_string_array(&args[0])?;
            let needle = as_string_array(&args[1])?;
            let instances = if args.len() == 3 {
                Some(compute::cast(&args[2], &DataType::Int64)?)
            } else {
                None
            };
            let instances = instances
                .as_ref()
                .map(|array| array.as_primitive::<Int64Type>());
            for row in 0..num_rows {
                if let Some(instances) = instances {
                    if !instances.is_null(row) && instances.value(row) == 0 {
                        return Err(KaveonError::Execution(
                            "STRPOS instance must not be zero".into(),
                        ));
                    }
                }
            }
            let result: Int64Array = (0..num_rows)
                .map(|i| {
                    if arr.is_null(i)
                        || needle.is_null(i)
                        || instances.is_some_and(|instances| instances.is_null(i))
                    {
                        return None;
                    }
                    let instance = instances.map_or(1, |instances| instances.value(i));
                    let positions = arr.value(i)
                        .match_indices(needle.value(i))
                        .map(|(byte, _)| arr.value(i)[..byte].chars().count() as i64 + 1)
                        .collect::<Vec<_>>();
                    let position = if instance > 0 {
                        positions.get(instance as usize - 1).copied()
                    } else {
                        let offset = instance.unsigned_abs() as usize;
                        (offset <= positions.len())
                            .then(|| positions[positions.len() - offset])
                    };
                    Some(position.unwrap_or(0))
                })
                .collect();
            Ok(Arc::new(result))
        }
        "POSITION" => {
            check_arity(name, args, 2)?;
            let arr = as_string_array(&args[0])?;
            let needle = as_string_array(&args[1])?;
            let result: Int64Array = (0..num_rows)
                .map(|i| {
                    if arr.is_null(i) || needle.is_null(i) {
                        None
                    } else {
                        Some(
                            arr.value(i)
                                .match_indices(needle.value(i))
                                .next()
                                .map(|(byte, _)| arr.value(i)[..byte].chars().count() as i64 + 1)
                                .unwrap_or(0),
                        )
                    }
                })
                .collect();
            Ok(Arc::new(result))
        }
        "SPLIT_PART" => {
            check_arity(name, args, 3)?;
            let values = as_string_array(&args[0])?;
            let delimiters = as_string_array(&args[1])?;
            let indices = compute::cast(&args[2], &DataType::Int64)?;
            let indices = indices.as_primitive::<Int64Type>();
            for row in 0..num_rows {
                if !indices.is_null(row) && indices.value(row) <= 0 {
                    return Err(KaveonError::Execution(
                        "SPLIT_PART index must be greater than zero".into(),
                    ));
                }
            }
            let result: StringArray = (0..num_rows)
                .map(|row| {
                    if values.is_null(row) || delimiters.is_null(row) || indices.is_null(row) {
                        return None;
                    }
                    let index = indices.value(row) as usize;
                    let delimiter = delimiters.value(row);
                    if delimiter.is_empty() {
                        return (index == 1).then(|| values.value(row).to_owned());
                    }
                    values.value(row).split(delimiter).nth(index - 1).map(str::to_owned)
                })
                .collect();
            Ok(Arc::new(result))
        }
        "SPLIT" => {
            if args.len() != 2 && args.len() != 3 {
                return Err(KaveonError::Execution(
                    "SPLIT requires 2 or 3 arguments".into(),
                ));
            }
            let values = as_string_array(&args[0])?;
            let delimiters = as_string_array(&args[1])?;
            let limits = if args.len() == 3 {
                Some(compute::cast(&args[2], &DataType::Int64)?)
            } else {
                None
            };
            let limits = limits.as_ref().map(|array| array.as_primitive::<Int64Type>());
            for row in 0..num_rows {
                if let Some(limits) = limits {
                    if !limits.is_null(row) && limits.value(row) <= 0 {
                        return Err(KaveonError::Execution(
                            "SPLIT limit must be greater than zero".into(),
                        ));
                    }
                }
            }
            let mut builder = ListBuilder::new(StringBuilder::new());
            for row in 0..num_rows {
                if values.is_null(row)
                    || delimiters.is_null(row)
                    || limits.is_some_and(|limits| limits.is_null(row))
                {
                    builder.append(false);
                    continue;
                }
                let value = values.value(row);
                let delimiter = delimiters.value(row);
                let limit = limits.map(|limits| limits.value(row) as usize);
                if delimiter.is_empty() {
                    let characters = value.chars().map(|character| character.to_string()).collect::<Vec<_>>();
                    if let Some(limit) = limit.filter(|limit| *limit < characters.len()) {
                        for character in &characters[..limit - 1] {
                            builder.values().append_value(character);
                        }
                        builder.values().append_value(characters[limit - 1..].concat());
                    } else {
                        for character in characters {
                            builder.values().append_value(character);
                        }
                    }
                } else {
                    if let Some(limit) = limit {
                        let mut parts = value.splitn(limit, delimiter);
                        for part in parts.by_ref() {
                            builder.values().append_value(part);
                        }
                    } else {
                        for part in value.split(delimiter) {
                            builder.values().append_value(part);
                        }
                    }
                }
                builder.append(true);
            }
            Ok(Arc::new(builder.finish()))
        }
        "REVERSE" => {
            check_arity(name, args, 1)?;
            let arr = as_string_array(&args[0])?;
            let result: StringArray = arr
                .iter()
                .map(|v| v.map(|s| s.chars().rev().collect::<String>()))
                .collect();
            Ok(Arc::new(result))
        }
        "REPEAT" => {
            check_arity(name, args, 2)?;
            let arr = as_string_array(&args[0])?;
            let counts = args[1].as_primitive::<Int64Type>();
            let bytes = (0..num_rows).try_fold(0_u64, |total, index| {
                if arr.is_null(index) || counts.is_null(index) {
                    return Ok(total);
                }
                let bytes = (arr.value(index).len() as u64)
                    .checked_mul(counts.value(index).max(0) as u64)
                    .and_then(|bytes| total.checked_add(bytes))
                    .ok_or_else(|| KaveonError::Execution("REPEAT output size overflow".into()))?;
                Ok::<_, KaveonError>(bytes)
            })?;
            reserve_string_expansion(bytes, num_rows)?;
            let result: StringArray = (0..num_rows)
                .map(|i| {
                    if arr.is_null(i) || counts.is_null(i) {
                        None
                    } else {
                        Some(arr.value(i).repeat(counts.value(i).max(0) as usize))
                    }
                })
                .collect();
            Ok(Arc::new(result))
        }

        // ── Math functions ──────────────────────────────────────────────
        "ABS" => {
            check_arity(name, args, 1)?;
            unary_float_fn(&args[0], f64::abs)
        }
        "CEIL" | "CEILING" => {
            check_arity(name, args, 1)?;
            unary_float_fn(&args[0], f64::ceil)
        }
        "FLOOR" => {
            check_arity(name, args, 1)?;
            unary_float_fn(&args[0], f64::floor)
        }
        "ROUND" => {
            if args.is_empty() || args.len() > 2 {
                return Err(KaveonError::Execution(
                    "ROUND requires 1 or 2 arguments".into(),
                ));
            }
            let arr = compute::cast(&args[0], &DataType::Float64)?;
            let vals = arr.as_primitive::<Float64Type>();
            if args.len() == 1 {
                let result: Float64Array = vals.iter().map(|v| v.map(f64::round)).collect();
                Ok(Arc::new(result))
            } else {
                let precision = args[1].as_primitive::<Int64Type>();
                let result: Float64Array = (0..num_rows)
                    .map(|i| {
                        if vals.is_null(i) || precision.is_null(i) {
                            None
                        } else {
                            let factor = 10_f64.powi(precision.value(i) as i32);
                            Some((vals.value(i) * factor).round() / factor)
                        }
                    })
                    .collect();
                Ok(Arc::new(result))
            }
        }
        "POWER" | "POW" => {
            check_arity(name, args, 2)?;
            let base = compute::cast(&args[0], &DataType::Float64)?;
            let exp = compute::cast(&args[1], &DataType::Float64)?;
            let b = base.as_primitive::<Float64Type>();
            let e = exp.as_primitive::<Float64Type>();
            let result: Float64Array = (0..num_rows)
                .map(|i| {
                    if b.is_null(i) || e.is_null(i) {
                        None
                    } else {
                        Some(b.value(i).powf(e.value(i)))
                    }
                })
                .collect();
            Ok(Arc::new(result))
        }
        "SQRT" => {
            check_arity(name, args, 1)?;
            unary_float_fn(&args[0], f64::sqrt)
        }
        "SIGN" | "SIGNUM" => {
            check_arity(name, args, 1)?;
            unary_float_fn(&args[0], f64::signum)
        }
        "LOG" | "LOG10" => {
            check_arity(name, args, 1)?;
            unary_float_fn(&args[0], f64::log10)
        }
        "LOG2" => {
            check_arity(name, args, 1)?;
            unary_float_fn(&args[0], f64::log2)
        }
        "LN" => {
            check_arity(name, args, 1)?;
            unary_float_fn(&args[0], f64::ln)
        }
        "EXP" => {
            check_arity(name, args, 1)?;
            unary_float_fn(&args[0], f64::exp)
        }
        "PI" => {
            if !args.is_empty() {
                return Err(KaveonError::Execution("PI takes no arguments".into()));
            }
            Ok(Arc::new(Float64Array::from(vec![
                std::f64::consts::PI;
                num_rows
            ])))
        }

        // ── Null-handling functions ──────────────────────────────────────
        "COALESCE" => {
            if args.is_empty() {
                return Err(KaveonError::Execution(
                    "COALESCE requires at least 1 argument".into(),
                ));
            }
            let target_type = args[0].data_type().clone();
            let mut result = compute::cast(&args[args.len() - 1], &target_type)?;
            for i in (0..args.len() - 1).rev() {
                let current = compute::cast(&args[i], &target_type)?;
                let is_not_null = compute::is_not_null(&current)?;
                result = zip_arrays(&is_not_null, &current, &result)?;
            }
            Ok(result)
        }
        "NULLIF" => {
            check_arity(name, args, 2)?;
            let (a, b) = coerce_numeric_pair(&args[0], &args[1])?;
            let eq = comparison(&a, &b, CompareKind::Eq)?;
            let len = a.len();
            let null_arr = make_null_array(a.data_type(), len)?;
            zip_arrays(&eq, &null_arr, &a)
        }
        "IF" | "IIF" => {
            check_arity(name, args, 3)?;
            let cond = as_boolean(&args[0])?;
            zip_arrays(cond, &args[1], &args[2])
        }
        "GREATEST" => {
            if args.len() < 2 {
                return Err(KaveonError::Execution(
                    "GREATEST requires at least 2 arguments".into(),
                ));
            }
            fold_extreme(&args[1..], &args[0], CompareKind::Ge)
        }
        "LEAST" => {
            if args.len() < 2 {
                return Err(KaveonError::Execution(
                    "LEAST requires at least 2 arguments".into(),
                ));
            }
            fold_extreme(&args[1..], &args[0], CompareKind::Le)
        }

        "NOW" | "CURRENT_TIMESTAMP" => {
            if !args.is_empty() {
                return Err(KaveonError::Execution(format!("{name} takes no arguments")));
            }
            let now = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_micros() as i64;
            Ok(Arc::new(arrow::array::TimestampMicrosecondArray::from(
                vec![now; num_rows],
            )))
        }
        "CURRENT_DATE" => {
            if !args.is_empty() {
                return Err(KaveonError::Execution(
                    "CURRENT_DATE takes no arguments".into(),
                ));
            }
            let days = (std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs()
                / 86400) as i32;
            Ok(Arc::new(arrow::array::Date32Array::from(vec![
                days;
                num_rows
            ])))
        }
        "DATE_TRUNC" => {
            if args.len() != 2 {
                return Err(KaveonError::Execution(
                    "DATE_TRUNC requires 2 arguments (unit, timestamp)".into(),
                ));
            }
            let unit_arr = as_string_array(&args[0])?;
            let ts = &args[1];
            eval_date_trunc(unit_arr, ts, num_rows)
        }
        "DATE_ADD_DAYS" => {
            check_arity(name, args, 2)?;
            eval_date_add_days(&args[0], &args[1], num_rows)
        }
        "DATE_ADD_SECONDS" => {
            check_arity(name, args, 2)?;
            eval_date_add_seconds(&args[0], &args[1], num_rows)
        }
        "DATE_ADD_MONTHS" => {
            check_arity(name, args, 2)?;
            eval_date_add_months(&args[0], &args[1], num_rows)
        }
        "DATE_PART" => {
            if args.len() != 2 {
                return Err(KaveonError::Execution(
                    "DATE_PART requires 2 arguments (field, source)".into(),
                ));
            }
            let field_arr = as_string_array(&args[0])?;
            let source = &args[1];
            eval_date_part(field_arr, source, num_rows)
        }
        "DATE_DIFF" => {
            if args.len() != 3 {
                return Err(KaveonError::Execution(
                    "DATE_DIFF requires 3 arguments (unit, start, end)".into(),
                ));
            }
            let unit_arr = as_string_array(&args[0])?;
            eval_date_diff(unit_arr, &args[1], &args[2], num_rows)
        }
        "TO_CHAR" | "DATE_FORMAT" => {
            check_arity(name, args, 2)?;
            let source = &args[0];
            let fmt_arr = as_string_array(&args[1])?;
            eval_to_char(source, fmt_arr, num_rows)
        }
        // ARRAY[...] is a typed list expression. Homogeneous integer
        // arguments retain Int64; mixed numeric arguments promote to
        // Float64. Boolean and string arguments retain their types.
        "ARRAY" => {
            let string_array = args.iter().any(|arg| arg.data_type() == &DataType::Utf8);
            let boolean_array = args.iter().any(|arg| arg.data_type() == &DataType::Boolean);
            let integer_array = args.iter().any(|arg| arg.data_type() == &DataType::Int64);
            let float_array = args.iter().any(|arg| arg.data_type() == &DataType::Float64);
            let temporal_type = args.iter().find_map(|arg| match arg.data_type() {
                DataType::Date32
                | DataType::Date64
                | DataType::Time32(TimeUnit::Second)
                | DataType::Time32(TimeUnit::Millisecond)
                | DataType::Timestamp(TimeUnit::Microsecond, _) => Some(arg.data_type().clone()),
                _ => None,
            });
            let decimal_type = args.iter().find_map(|arg| match arg.data_type() {
                DataType::Decimal128(precision, scale) => Some((*precision, *scale)),
                _ => None,
            });
            if string_array && boolean_array {
                return Err(KaveonError::Execution("ARRAY elements must share a type".into()));
            }
            if let Some(element_type) = temporal_type {
                if args.iter().any(|arg| {
                    arg.data_type() != &element_type && arg.data_type() != &DataType::Null
                }) {
                    return Err(KaveonError::Execution(
                        "ARRAY temporal elements must share a type".into(),
                    ));
                }
                macro_rules! temporal_array {
                    ($ty:ty) => {{
                        let values = args
                            .iter()
                            .map(|arg| compute::cast(arg, &element_type))
                            .collect::<std::result::Result<Vec<_>, _>>()?;
                        let mut builder = ListBuilder::new(PrimitiveBuilder::<$ty>::new());
                        for row in 0..num_rows {
                            for value in &values {
                                let value = value.as_primitive::<$ty>();
                                builder
                                    .values()
                                    .append_option(value.is_valid(row).then(|| value.value(row)));
                            }
                            builder.append(true);
                        }
                        Ok(Arc::new(builder.finish()))
                    }};
                }
                match &element_type {
                    DataType::Date32 => temporal_array!(Date32Type),
                    DataType::Date64 => temporal_array!(Date64Type),
                    DataType::Time32(TimeUnit::Second) => temporal_array!(Time32SecondType),
                    DataType::Time32(TimeUnit::Millisecond) => {
                        temporal_array!(Time32MillisecondType)
                    }
                    DataType::Timestamp(TimeUnit::Microsecond, _) => {
                        temporal_array!(TimestampMicrosecondType)
                    }
                    _ => unreachable!("temporal type was filtered above"),
                }
            } else if let Some((precision, scale)) = decimal_type {
                if args.iter().any(|arg| {
                    !matches!(arg.data_type(), DataType::Decimal128(p, s) if *p == precision && *s == scale)
                        && arg.data_type() != &DataType::Null
                }) {
                    return Err(KaveonError::Execution(
                        "ARRAY decimal elements must share precision and scale".into(),
                    ));
                }
                let values = args
                    .iter()
                    .map(|arg| compute::cast(arg, &DataType::Decimal128(precision, scale)))
                    .collect::<std::result::Result<Vec<_>, _>>()?;
                let decimal_builder = Decimal128Builder::new()
                    .with_precision_and_scale(precision, scale)
                    .map_err(|error| KaveonError::Execution(error.to_string()))?;
                let mut builder = ListBuilder::new(decimal_builder);
                for row in 0..num_rows {
                    for value in &values {
                        let value = value.as_primitive::<arrow::datatypes::Decimal128Type>();
                        builder
                            .values()
                            .append_option(value.is_valid(row).then(|| value.value(row)));
                    }
                    builder.append(true);
                }
                Ok(Arc::new(builder.finish()))
            } else if boolean_array {
                if args
                    .iter()
                    .any(|arg| !matches!(arg.data_type(), DataType::Boolean | DataType::Null))
                {
                    return Err(KaveonError::Execution("ARRAY elements must share a type".into()));
                }
                let values = args
                    .iter()
                    .map(|arg| compute::cast(arg, &DataType::Boolean))
                    .collect::<std::result::Result<Vec<_>, _>>()?;
                let mut builder = ListBuilder::new(BooleanBuilder::new());
                for row in 0..num_rows {
                    for value in &values {
                        let value = value.as_boolean();
                        builder
                            .values()
                            .append_option(value.is_valid(row).then(|| value.value(row)));
                    }
                    builder.append(true);
                }
                Ok(Arc::new(builder.finish()))
            } else if string_array {
                if args
                    .iter()
                    .any(|arg| !matches!(arg.data_type(), DataType::Utf8 | DataType::Null))
                {
                    return Err(KaveonError::Execution(
                        "ARRAY elements must share a type".into(),
                    ));
                }
                let values = args
                    .iter()
                    .map(|arg| compute::cast(arg, &DataType::Utf8))
                    .collect::<std::result::Result<Vec<_>, _>>()?;
                let mut builder = ListBuilder::new(StringBuilder::new());
                for row in 0..num_rows {
                    for value in &values {
                        let value = value.as_string::<i32>();
                        builder
                            .values()
                            .append_option(value.is_valid(row).then(|| value.value(row)));
                    }
                    builder.append(true);
                }
                Ok(Arc::new(builder.finish()))
            } else if integer_array && !float_array {
                if args
                    .iter()
                    .any(|arg| !matches!(arg.data_type(), DataType::Int64 | DataType::Null))
                {
                    return Err(KaveonError::Execution(
                        "ARRAY elements must share a type".into(),
                    ));
                }
                let values = args
                    .iter()
                    .map(|arg| compute::cast(arg, &DataType::Int64))
                    .collect::<std::result::Result<Vec<_>, _>>()?;
                let mut builder = ListBuilder::new(Int64Builder::new());
                for row in 0..num_rows {
                    for value in &values {
                        let value = value.as_primitive::<Int64Type>();
                        builder
                            .values()
                            .append_option(value.is_valid(row).then(|| value.value(row)));
                    }
                    builder.append(true);
                }
                Ok(Arc::new(builder.finish()))
            } else {
                let values = args
                    .iter()
                    .map(|arg| compute::cast(arg, &DataType::Float64))
                    .collect::<std::result::Result<Vec<_>, _>>()?;
                let mut builder = ListBuilder::new(Float64Builder::new());
                for row in 0..num_rows {
                    for value in &values {
                        let value = value.as_primitive::<Float64Type>();
                        builder
                            .values()
                            .append_option(value.is_valid(row).then(|| value.value(row)));
                    }
                    builder.append(true);
                }
                Ok(Arc::new(builder.finish()))
            }
        }
        "ARRAY_CONCAT" => {
            check_arity(name, args, 2)?;
            let left = args[0]
                .as_any()
                .downcast_ref::<ListArray>()
                .ok_or_else(|| KaveonError::Execution("ARRAY_CONCAT requires ARRAY/List values".into()))?;
            let right = args[1]
                .as_any()
                .downcast_ref::<ListArray>()
                .ok_or_else(|| KaveonError::Execution("ARRAY_CONCAT requires ARRAY/List values".into()))?;
            if left.len() != right.len() || left.value_type() != right.value_type() {
                return Err(KaveonError::Execution(
                    "ARRAY_CONCAT requires arrays with matching row counts and element types".into(),
                ));
            }
            // Complex element types (nested arrays, ROWs, and MAPs) cannot
            // use the primitive builders below, but Arrow can concatenate
            // their sliced child arrays without converting them to JSON or
            // losing nulls. Keep this path generic and preserve each outer
            // row's validity and offsets.
            if matches!(
                left.value_type(),
                DataType::List(_)
                    | DataType::LargeList(_)
                    | DataType::Struct(_)
                    | DataType::Map(_, _)
            ) {
                let mut child_chunks = Vec::with_capacity(num_rows * 2);
                let mut offsets = vec![0_i32];
                let mut valid = Vec::with_capacity(num_rows);
                for row in 0..num_rows {
                    if left.is_null(row) || right.is_null(row) {
                        valid.push(false);
                        offsets.push(*offsets.last().unwrap());
                        continue;
                    }
                    valid.push(true);
                    let left_inner = left.value(row);
                    let right_inner = right.value(row);
                    child_chunks.push(left_inner);
                    child_chunks.push(right_inner);
                    let length = offsets.last().copied().unwrap_or_default() as usize
                        + child_chunks[child_chunks.len() - 2].len()
                        + child_chunks[child_chunks.len() - 1].len();
                    offsets.push(i32::try_from(length).map_err(|_| {
                        KaveonError::Execution("ARRAY_CONCAT result offset overflow".into())
                    })?);
                }
                let values = if child_chunks.is_empty() {
                    arrow::array::new_empty_array(&left.value_type())
                } else {
                    let chunks = child_chunks
                        .iter()
                        .map(|chunk| chunk.as_ref() as &dyn Array)
                        .collect::<Vec<_>>();
                    compute::concat(&chunks)?
                };
                let field = Arc::new(arrow::datatypes::Field::new(
                    "item",
                    values.data_type().clone(),
                    true,
                ));
                return Ok(Arc::new(ListArray::try_new(
                    field,
                    OffsetBuffer::new(ScalarBuffer::from(offsets)),
                    values,
                    Some(arrow::buffer::NullBuffer::from(valid)),
                )?));
            }
            macro_rules! concat_primitive {
                ($ty:ty, $builder:expr) => {{
                    let mut builder = ListBuilder::new($builder);
                    for row in 0..num_rows {
                        if left.is_null(row) || right.is_null(row) {
                            builder.append(false);
                            continue;
                        }
                        let left_inner = left.value(row);
                        let right_inner = right.value(row);
                        let left_values = left_inner.as_primitive::<$ty>();
                        let right_values = right_inner.as_primitive::<$ty>();
                        for index in 0..left_values.len() {
                            builder.values().append_option(
                                left_values.is_valid(index).then(|| left_values.value(index)),
                            );
                        }
                        for index in 0..right_values.len() {
                            builder.values().append_option(
                                right_values.is_valid(index).then(|| right_values.value(index)),
                            );
                        }
                        builder.append(true);
                    }
                    Ok(Arc::new(builder.finish()))
                }};
            }
            match left.value_type() {
                DataType::Int64 => concat_primitive!(Int64Type, Int64Builder::new()),
                DataType::Float64 => concat_primitive!(Float64Type, Float64Builder::new()),
                DataType::Date32 => concat_primitive!(Date32Type, PrimitiveBuilder::<Date32Type>::new()),
                DataType::Date64 => concat_primitive!(Date64Type, PrimitiveBuilder::<Date64Type>::new()),
                DataType::Time32(TimeUnit::Second) => concat_primitive!(
                    Time32SecondType,
                    PrimitiveBuilder::<Time32SecondType>::new()
                ),
                DataType::Time32(TimeUnit::Millisecond) => concat_primitive!(
                    Time32MillisecondType,
                    PrimitiveBuilder::<Time32MillisecondType>::new()
                ),
                DataType::Timestamp(TimeUnit::Microsecond, _) => concat_primitive!(
                    TimestampMicrosecondType,
                    PrimitiveBuilder::<TimestampMicrosecondType>::new()
                ),
                DataType::Decimal128(precision, scale) => {
                    let mut builder = ListBuilder::new(
                        Decimal128Builder::new()
                            .with_precision_and_scale(precision, scale)
                            .map_err(|error| KaveonError::Execution(error.to_string()))?,
                    );
                    for row in 0..num_rows {
                        if left.is_null(row) || right.is_null(row) {
                            builder.append(false);
                            continue;
                        }
                        let left_inner = left.value(row);
                        let right_inner = right.value(row);
                        let left_values = left_inner.as_primitive::<arrow::datatypes::Decimal128Type>();
                        let right_values = right_inner.as_primitive::<arrow::datatypes::Decimal128Type>();
                        for index in 0..left_values.len() {
                            builder.values().append_option(
                                left_values.is_valid(index).then(|| left_values.value(index)),
                            );
                        }
                        for index in 0..right_values.len() {
                            builder.values().append_option(
                                right_values.is_valid(index).then(|| right_values.value(index)),
                            );
                        }
                        builder.append(true);
                    }
                    Ok(Arc::new(builder.finish()))
                }
                DataType::Utf8 => {
                    let mut builder = ListBuilder::new(StringBuilder::new());
                    for row in 0..num_rows {
                        if left.is_null(row) || right.is_null(row) {
                            builder.append(false);
                            continue;
                        }
                        let left_inner = left.value(row);
                        let right_inner = right.value(row);
                        let left_values = left_inner.as_string::<i32>();
                        let right_values = right_inner.as_string::<i32>();
                        for index in 0..left_values.len() {
                            builder.values().append_option(
                                left_values.is_valid(index).then(|| left_values.value(index)),
                            );
                        }
                        for index in 0..right_values.len() {
                            builder.values().append_option(
                                right_values.is_valid(index).then(|| right_values.value(index)),
                            );
                        }
                        builder.append(true);
                    }
                    Ok(Arc::new(builder.finish()))
                }
                DataType::Boolean => {
                    let mut builder = ListBuilder::new(BooleanBuilder::new());
                    for row in 0..num_rows {
                        if left.is_null(row) || right.is_null(row) {
                            builder.append(false);
                            continue;
                        }
                        let left_inner = left.value(row);
                        let right_inner = right.value(row);
                        let left_values = left_inner.as_boolean();
                        let right_values = right_inner.as_boolean();
                        for index in 0..left_values.len() {
                            builder.values().append_option(
                                left_values.is_valid(index).then(|| left_values.value(index)),
                            );
                        }
                        for index in 0..right_values.len() {
                            builder.values().append_option(
                                right_values.is_valid(index).then(|| right_values.value(index)),
                            );
                        }
                        builder.append(true);
                    }
                    Ok(Arc::new(builder.finish()))
                }
                other => Err(KaveonError::Execution(format!(
                    "ARRAY_CONCAT does not support list element type {other}"
                ))),
            }
        }
        "ARRAY_APPEND" | "ARRAY_PREPEND" => {
            check_arity(name, args, 2)?;
            let list = args[0]
                .as_any()
                .downcast_ref::<ListArray>()
                .ok_or_else(|| KaveonError::Execution(format!("{name} requires an ARRAY/List value")))?;
            let prepend = name == "ARRAY_PREPEND";
            if args[1].len() != num_rows {
                return Err(KaveonError::Execution(format!("{name} requires one value per row")));
            }
            if matches!(
                list.value_type(),
                DataType::List(_)
                    | DataType::LargeList(_)
                    | DataType::Struct(_)
                    | DataType::Map(_, _)
            ) {
                let scalar = if args[1].data_type() == &DataType::Null {
                    arrow::array::new_null_array(&list.value_type(), num_rows)
                } else {
                    if args[1].data_type() != &list.value_type() {
                        return Err(KaveonError::Execution(format!(
                            "{name} requires the appended value to match list element type"
                        )));
                    }
                    Arc::clone(&args[1])
                };
                let mut child_chunks = Vec::with_capacity(num_rows * 2);
                let mut offsets = vec![0_i32];
                let mut valid = Vec::with_capacity(num_rows);
                for row in 0..num_rows {
                    if list.is_null(row) {
                        valid.push(false);
                        offsets.push(*offsets.last().unwrap());
                        continue;
                    }
                    valid.push(true);
                    let inner = list.value(row);
                    let value = scalar.slice(row, 1);
                    if prepend {
                        child_chunks.push(value);
                        child_chunks.push(inner);
                    } else {
                        child_chunks.push(inner);
                        child_chunks.push(value);
                    }
                    let length = offsets.last().copied().unwrap_or_default() as usize
                        + child_chunks[child_chunks.len() - 2].len()
                        + child_chunks[child_chunks.len() - 1].len();
                    offsets.push(i32::try_from(length).map_err(|_| {
                        KaveonError::Execution(format!("{name} result offset overflow"))
                    })?);
                }
                let values = if child_chunks.is_empty() {
                    arrow::array::new_empty_array(&list.value_type())
                } else {
                    let chunks = child_chunks
                        .iter()
                        .map(|chunk| chunk.as_ref() as &dyn Array)
                        .collect::<Vec<_>>();
                    compute::concat(&chunks)?
                };
                let field = Arc::new(arrow::datatypes::Field::new(
                    "item",
                    values.data_type().clone(),
                    true,
                ));
                return Ok(Arc::new(ListArray::try_new(
                    field,
                    OffsetBuffer::new(ScalarBuffer::from(offsets)),
                    values,
                    Some(arrow::buffer::NullBuffer::from(valid)),
                )?));
            }
            macro_rules! append_primitive {
                ($ty:ty, $builder:expr) => {{
                    let scalar = compute::cast(&args[1], &list.value_type())?;
                    let scalar = scalar.as_primitive::<$ty>();
                    let mut builder = ListBuilder::new($builder);
                    for row in 0..num_rows {
                        if list.is_null(row) {
                            builder.append(false);
                            continue;
                        }
                        let inner = list.value(row);
                        let values = inner.as_primitive::<$ty>();
                        if prepend {
                            builder.values().append_option(
                                scalar.is_valid(row).then(|| scalar.value(row)),
                            );
                        }
                        for index in 0..values.len() {
                            builder.values().append_option(
                                values.is_valid(index).then(|| values.value(index)),
                            );
                        }
                        if !prepend {
                            builder.values().append_option(
                                scalar.is_valid(row).then(|| scalar.value(row)),
                            );
                        }
                        builder.append(true);
                    }
                    Ok(Arc::new(builder.finish()))
                }};
            }
            match list.value_type() {
                DataType::Int64 => append_primitive!(Int64Type, Int64Builder::new()),
                DataType::Float64 => append_primitive!(Float64Type, Float64Builder::new()),
                DataType::Date32 => append_primitive!(
                    Date32Type,
                    PrimitiveBuilder::<Date32Type>::new()
                ),
                DataType::Date64 => append_primitive!(
                    Date64Type,
                    PrimitiveBuilder::<Date64Type>::new()
                ),
                DataType::Time32(TimeUnit::Second) => append_primitive!(
                    Time32SecondType,
                    PrimitiveBuilder::<Time32SecondType>::new()
                ),
                DataType::Time32(TimeUnit::Millisecond) => append_primitive!(
                    Time32MillisecondType,
                    PrimitiveBuilder::<Time32MillisecondType>::new()
                ),
                DataType::Timestamp(TimeUnit::Microsecond, _) => append_primitive!(
                    TimestampMicrosecondType,
                    PrimitiveBuilder::<TimestampMicrosecondType>::new()
                ),
                DataType::Decimal128(precision, scale) => {
                    let scalar = compute::cast(&args[1], &list.value_type())?;
                    let scalar = scalar.as_primitive::<arrow::datatypes::Decimal128Type>();
                    let mut builder = ListBuilder::new(
                        Decimal128Builder::new()
                            .with_precision_and_scale(precision, scale)
                            .map_err(|error| KaveonError::Execution(error.to_string()))?,
                    );
                    for row in 0..num_rows {
                        if list.is_null(row) {
                            builder.append(false);
                            continue;
                        }
                        let inner = list.value(row);
                        let values = inner.as_primitive::<arrow::datatypes::Decimal128Type>();
                        if prepend {
                            builder.values().append_option(
                                scalar.is_valid(row).then(|| scalar.value(row)),
                            );
                        }
                        for index in 0..values.len() {
                            builder.values().append_option(
                                values.is_valid(index).then(|| values.value(index)),
                            );
                        }
                        if !prepend {
                            builder.values().append_option(
                                scalar.is_valid(row).then(|| scalar.value(row)),
                            );
                        }
                        builder.append(true);
                    }
                    Ok(Arc::new(builder.finish()))
                }
                DataType::Utf8 => {
                    let scalar = compute::cast(&args[1], &DataType::Utf8)?;
                    let scalar = scalar.as_string::<i32>();
                    let mut builder = ListBuilder::new(StringBuilder::new());
                    for row in 0..num_rows {
                        if list.is_null(row) {
                            builder.append(false);
                            continue;
                        }
                        let inner = list.value(row);
                        let values = inner.as_string::<i32>();
                        if prepend {
                            builder.values().append_option(
                                scalar.is_valid(row).then(|| scalar.value(row)),
                            );
                        }
                        for index in 0..values.len() {
                            builder.values().append_option(
                                values.is_valid(index).then(|| values.value(index)),
                            );
                        }
                        if !prepend {
                            builder.values().append_option(
                                scalar.is_valid(row).then(|| scalar.value(row)),
                            );
                        }
                        builder.append(true);
                    }
                    Ok(Arc::new(builder.finish()))
                }
                DataType::Boolean => {
                    let scalar = compute::cast(&args[1], &DataType::Boolean)?;
                    let scalar = scalar.as_boolean();
                    let mut builder = ListBuilder::new(BooleanBuilder::new());
                    for row in 0..num_rows {
                        if list.is_null(row) {
                            builder.append(false);
                            continue;
                        }
                        let inner = list.value(row);
                        let values = inner.as_boolean();
                        if prepend {
                            builder.values().append_option(
                                scalar.is_valid(row).then(|| scalar.value(row)),
                            );
                        }
                        for index in 0..values.len() {
                            builder.values().append_option(
                                values.is_valid(index).then(|| values.value(index)),
                            );
                        }
                        if !prepend {
                            builder.values().append_option(
                                scalar.is_valid(row).then(|| scalar.value(row)),
                            );
                        }
                        builder.append(true);
                    }
                    Ok(Arc::new(builder.finish()))
                }
                other => Err(KaveonError::Execution(format!(
                    "{name} does not support list element type {other}"
                ))),
            }
        }
        "ARRAY_REMOVE" => {
            check_arity(name, args, 2)?;
            let list = args[0]
                .as_any()
                .downcast_ref::<ListArray>()
                .ok_or_else(|| KaveonError::Execution("ARRAY_REMOVE requires an ARRAY/List value".into()))?;
            if args[1].len() != num_rows {
                return Err(KaveonError::Execution("ARRAY_REMOVE requires one value per row".into()));
            }
            if matches!(
                list.value_type(),
                DataType::List(_)
                    | DataType::LargeList(_)
                    | DataType::Struct(_)
                    | DataType::Map(_, _)
            ) {
                let mut selected = Vec::<ArrayRef>::new();
                let mut offsets = vec![0_i32];
                let mut valid = Vec::with_capacity(num_rows);
                for row in 0..num_rows {
                    if list.is_null(row) {
                        valid.push(false);
                        offsets.push(*offsets.last().unwrap());
                        continue;
                    }
                    valid.push(true);
                    let inner = list.value(row);
                    let target_null = args[1].is_null(row);
                    let target = args[1].slice(row, 1);
                    for index in 0..inner.len() {
                        let keep = if inner.is_null(index) {
                            !target_null
                        } else if target_null {
                            true
                        } else {
                            let candidate = inner.slice(index, 1);
                            let equal = comparison(&candidate, &target, CompareKind::Eq)?;
                            equal.is_null(0) || !equal.value(0)
                        };
                        if keep {
                            selected.push(inner.slice(index, 1));
                        }
                    }
                    offsets.push(i32::try_from(selected.len()).map_err(|_| {
                        KaveonError::Execution("ARRAY_REMOVE result offset overflow".into())
                    })?);
                }
                let values = if selected.is_empty() {
                    arrow::array::new_empty_array(&list.value_type())
                } else {
                    let chunks = selected
                        .iter()
                        .map(|chunk| chunk.as_ref() as &dyn Array)
                        .collect::<Vec<_>>();
                    compute::concat(&chunks)?
                };
                let field = Arc::new(arrow::datatypes::Field::new(
                    "item",
                    values.data_type().clone(),
                    true,
                ));
                return Ok(Arc::new(ListArray::try_new(
                    field,
                    OffsetBuffer::new(ScalarBuffer::from(offsets)),
                    values,
                    Some(arrow::buffer::NullBuffer::from(valid)),
                )?));
            }
            macro_rules! remove_primitive {
                ($ty:ty, $builder:expr) => {{
                    let target = compute::cast(&args[1], &list.value_type())?;
                    let target = target.as_primitive::<$ty>();
                    let mut builder = ListBuilder::new($builder);
                    for row in 0..num_rows {
                        if list.is_null(row) {
                            builder.append(false);
                            continue;
                        }
                        let inner = list.value(row);
                        let values = inner.as_primitive::<$ty>();
                        for index in 0..values.len() {
                            let keep = if values.is_null(index) {
                                !target.is_null(row)
                            } else {
                                target.is_null(row) || values.value(index) != target.value(row)
                            };
                            if keep {
                                builder.values().append_option(
                                    values.is_valid(index).then(|| values.value(index)),
                                );
                            }
                        }
                        builder.append(true);
                    }
                    Ok(Arc::new(builder.finish()))
                }};
            }
            match list.value_type() {
                DataType::Int64 => remove_primitive!(Int64Type, Int64Builder::new()),
                DataType::Float64 => remove_primitive!(Float64Type, Float64Builder::new()),
                DataType::Date32 => remove_primitive!(
                    Date32Type,
                    PrimitiveBuilder::<Date32Type>::new()
                ),
                DataType::Date64 => remove_primitive!(
                    Date64Type,
                    PrimitiveBuilder::<Date64Type>::new()
                ),
                DataType::Time32(TimeUnit::Second) => remove_primitive!(
                    Time32SecondType,
                    PrimitiveBuilder::<Time32SecondType>::new()
                ),
                DataType::Time32(TimeUnit::Millisecond) => remove_primitive!(
                    Time32MillisecondType,
                    PrimitiveBuilder::<Time32MillisecondType>::new()
                ),
                DataType::Timestamp(TimeUnit::Microsecond, _) => remove_primitive!(
                    TimestampMicrosecondType,
                    PrimitiveBuilder::<TimestampMicrosecondType>::new()
                ),
                DataType::Decimal128(precision, scale) => {
                    let target = compute::cast(&args[1], &list.value_type())?;
                    let target = target.as_primitive::<arrow::datatypes::Decimal128Type>();
                    let mut builder = ListBuilder::new(
                        Decimal128Builder::new()
                            .with_precision_and_scale(precision, scale)
                            .map_err(|error| KaveonError::Execution(error.to_string()))?,
                    );
                    for row in 0..num_rows {
                        if list.is_null(row) {
                            builder.append(false);
                            continue;
                        }
                        let inner = list.value(row);
                        let values = inner.as_primitive::<arrow::datatypes::Decimal128Type>();
                        for index in 0..values.len() {
                            let keep = if values.is_null(index) {
                                !target.is_null(row)
                            } else {
                                target.is_null(row) || values.value(index) != target.value(row)
                            };
                            if keep {
                                builder.values().append_option(
                                    values.is_valid(index).then(|| values.value(index)),
                                );
                            }
                        }
                        builder.append(true);
                    }
                    Ok(Arc::new(builder.finish()))
                }
                DataType::Utf8 => {
                    let target = compute::cast(&args[1], &DataType::Utf8)?;
                    let target = target.as_string::<i32>();
                    let mut builder = ListBuilder::new(StringBuilder::new());
                    for row in 0..num_rows {
                        if list.is_null(row) {
                            builder.append(false);
                            continue;
                        }
                        let inner = list.value(row);
                        let values = inner.as_string::<i32>();
                        for index in 0..values.len() {
                            let keep = if values.is_null(index) {
                                !target.is_null(row)
                            } else {
                                target.is_null(row) || values.value(index) != target.value(row)
                            };
                            if keep {
                                builder.values().append_option(
                                    values.is_valid(index).then(|| values.value(index)),
                                );
                            }
                        }
                        builder.append(true);
                    }
                    Ok(Arc::new(builder.finish()))
                }
                DataType::Boolean => {
                    let target = compute::cast(&args[1], &DataType::Boolean)?;
                    let target = target.as_boolean();
                    let mut builder = ListBuilder::new(BooleanBuilder::new());
                    for row in 0..num_rows {
                        if list.is_null(row) {
                            builder.append(false);
                            continue;
                        }
                        let inner = list.value(row);
                        let values = inner.as_boolean();
                        for index in 0..values.len() {
                            let keep = if values.is_null(index) {
                                !target.is_null(row)
                            } else {
                                target.is_null(row) || values.value(index) != target.value(row)
                            };
                            if keep {
                                builder.values().append_option(
                                    values.is_valid(index).then(|| values.value(index)),
                                );
                            }
                        }
                        builder.append(true);
                    }
                    Ok(Arc::new(builder.finish()))
                }
                other => Err(KaveonError::Execution(format!(
                    "ARRAY_REMOVE does not support list element type {other}"
                ))),
            }
        }
        "ARRAY_MIN" | "ARRAY_MAX" => {
            check_arity(name, args, 1)?;
            if let Some(list) = args[0].as_any().downcast_ref::<LargeListArray>() {
                return array_extrema_generic(list, name == "ARRAY_MAX", name);
            }
            let list = args[0]
                .as_any()
                .downcast_ref::<ListArray>()
                .ok_or_else(|| {
                    KaveonError::Execution(format!("{name} requires an ARRAY/List value"))
                })?;
            let take_max = name == "ARRAY_MAX";
            macro_rules! temporal_extrema {
                ($ty:ty, $builder:expr) => {{
                    let mut result = $builder;
                    for row in 0..num_rows {
                        if list.is_null(row) {
                            result.append_null();
                            continue;
                        }
                        let inner = list.value(row);
                        let values = inner.as_primitive::<$ty>();
                        let best = (0..values.len())
                            .filter(|&index| values.is_valid(index))
                            .map(|index| values.value(index))
                            .reduce(|left, right| {
                                if (take_max && right > left) || (!take_max && right < left) {
                                    right
                                } else {
                                    left
                                }
                            });
                        result.append_option(best);
                    }
                    Ok(Arc::new(result.finish()))
                }};
            }
            match list.value_type() {
                DataType::Int64 => {
                    let mut result = Int64Builder::new();
                    for row in 0..num_rows {
                        if list.is_null(row) {
                            result.append_null();
                            continue;
                        }
                        let values = list.value(row);
                        let values = values.as_primitive::<Int64Type>();
                        let best = (0..values.len())
                            .filter(|&index| values.is_valid(index))
                            .map(|index| values.value(index))
                            .reduce(|left, right| {
                                if (take_max && right > left) || (!take_max && right < left) {
                                    right
                                } else {
                                    left
                                }
                            });
                        result.append_option(best);
                    }
                    Ok(Arc::new(result.finish()))
                }
                DataType::Float64 => {
                    let mut result = Float64Builder::new();
                    for row in 0..num_rows {
                        if list.is_null(row) {
                            result.append_null();
                            continue;
                        }
                        let values = list.value(row);
                        let values = values.as_primitive::<Float64Type>();
                        let best = (0..values.len())
                            .filter(|&index| values.is_valid(index))
                            .map(|index| values.value(index))
                            .reduce(|left, right| {
                                if (take_max && right > left) || (!take_max && right < left) {
                                    right
                                } else {
                                    left
                                }
                            });
                        result.append_option(best);
                    }
                    Ok(Arc::new(result.finish()))
                }
                DataType::Date32 => temporal_extrema!(
                    Date32Type,
                    PrimitiveBuilder::<Date32Type>::new()
                ),
                DataType::Date64 => temporal_extrema!(
                    Date64Type,
                    PrimitiveBuilder::<Date64Type>::new()
                ),
                DataType::Time32(TimeUnit::Second) => temporal_extrema!(
                    Time32SecondType,
                    PrimitiveBuilder::<Time32SecondType>::new()
                ),
                DataType::Time32(TimeUnit::Millisecond) => temporal_extrema!(
                    Time32MillisecondType,
                    PrimitiveBuilder::<Time32MillisecondType>::new()
                ),
                DataType::Timestamp(TimeUnit::Microsecond, _) => temporal_extrema!(
                    TimestampMicrosecondType,
                    PrimitiveBuilder::<TimestampMicrosecondType>::new()
                ),
                DataType::Decimal128(precision, scale) => {
                    let mut result = Decimal128Builder::new()
                        .with_precision_and_scale(precision, scale)
                        .map_err(|error| KaveonError::Execution(error.to_string()))?;
                    for row in 0..num_rows {
                        if list.is_null(row) {
                            result.append_null();
                            continue;
                        }
                        let values = list.value(row);
                        let values = values.as_primitive::<arrow::datatypes::Decimal128Type>();
                        let best = (0..values.len())
                            .filter(|&index| values.is_valid(index))
                            .map(|index| values.value(index))
                            .reduce(|left, right| {
                                if (take_max && right > left) || (!take_max && right < left) {
                                    right
                                } else {
                                    left
                                }
                            });
                        result.append_option(best);
                    }
                    Ok(Arc::new(result.finish()))
                }
                DataType::Utf8 => {
                    let mut result = StringBuilder::new();
                    for row in 0..num_rows {
                        if list.is_null(row) {
                            result.append_null();
                            continue;
                        }
                        let values = list.value(row);
                        let values = values.as_string::<i32>();
                        let best = (0..values.len())
                            .filter(|&index| values.is_valid(index))
                            .map(|index| values.value(index))
                            .reduce(|left, right| {
                                if (take_max && right > left) || (!take_max && right < left) {
                                    right
                                } else {
                                    left
                                }
                            });
                        result.append_option(best);
                    }
                    Ok(Arc::new(result.finish()))
                }
                other => Err(KaveonError::Execution(format!(
                    "{name} does not support list element type {other}"
                ))),
            }
        }
        "ARRAYS_OVERLAP" => {
            check_arity(name, args, 2)?;
            if let (Some(left), Some(right)) = (
                args[0].as_any().downcast_ref::<LargeListArray>(),
                args[1].as_any().downcast_ref::<LargeListArray>(),
            ) {
                if matches!(
                    left.value_type(),
                    DataType::List(_)
                        | DataType::LargeList(_)
                        | DataType::Struct(_)
                        | DataType::Map(_, _)
                ) {
                    return arrays_overlap_nested(left, right);
                }
                if left.len() != right.len() || left.value_type() != right.value_type() {
                    return Err(KaveonError::Execution(
                        "ARRAYS_OVERLAP requires arrays with matching row counts and element types".into(),
                    ));
                }
                return match left.value_type() {
                    DataType::Int32 => arrays_overlap_primitive::<_, Int32Type>(left, right),
                    DataType::Int64 => arrays_overlap_primitive::<_, Int64Type>(left, right),
                    DataType::Float64 => arrays_overlap_primitive::<_, Float64Type>(left, right),
                    DataType::Date32 => arrays_overlap_primitive::<_, Date32Type>(left, right),
                    DataType::Date64 => arrays_overlap_primitive::<_, Date64Type>(left, right),
                    DataType::Time32(TimeUnit::Second) =>
                        arrays_overlap_primitive::<_, Time32SecondType>(left, right),
                    DataType::Time32(TimeUnit::Millisecond) =>
                        arrays_overlap_primitive::<_, Time32MillisecondType>(left, right),
                    DataType::Timestamp(TimeUnit::Microsecond, _) =>
                        arrays_overlap_primitive::<_, TimestampMicrosecondType>(left, right),
                    DataType::Boolean => arrays_overlap_boolean(left, right),
                    DataType::Utf8 => arrays_overlap_utf8(left, right),
                    other => Err(KaveonError::Execution(format!(
                        "ARRAYS_OVERLAP does not support list element type {other}"
                    ))),
                };
            }
            let left = args[0]
                .as_any()
                .downcast_ref::<ListArray>()
                .ok_or_else(|| KaveonError::Execution("ARRAYS_OVERLAP requires ARRAY/List values".into()))?;
            let right = args[1]
                .as_any()
                .downcast_ref::<ListArray>()
                .ok_or_else(|| KaveonError::Execution("ARRAYS_OVERLAP requires ARRAY/List values".into()))?;
            if left.len() != right.len() || left.value_type() != right.value_type() {
                return Err(KaveonError::Execution(
                    "ARRAYS_OVERLAP requires arrays with matching row counts and element types".into(),
                ));
            }
            if matches!(
                left.value_type(),
                DataType::List(_)
                    | DataType::LargeList(_)
                    | DataType::Struct(_)
                    | DataType::Map(_, _)
            ) {
                return arrays_overlap_nested(left, right);
            }
            macro_rules! overlap_primitive {
                ($ty:ty) => {
                    (0..num_rows)
                        .map(|row| {
                            if left.is_null(row) || right.is_null(row) {
                                return None;
                            }
                            primitive_arrays_overlap(
                                left.value(row).as_primitive::<$ty>(),
                                right.value(row).as_primitive::<$ty>(),
                            )
                        })
                        .collect::<Vec<_>>()
                };
            }
            let rows = match left.value_type() {
                DataType::Int64 => (0..num_rows)
                    .map(|row| {
                        if left.is_null(row) || right.is_null(row) {
                            return None;
                        }
                        primitive_arrays_overlap(
                            left.value(row).as_primitive::<Int64Type>(),
                            right.value(row).as_primitive::<Int64Type>(),
                        )
                    })
                    .collect::<Vec<_>>(),
                DataType::Float64 => (0..num_rows)
                    .map(|row| {
                        if left.is_null(row) || right.is_null(row) {
                            return None;
                        }
                        primitive_arrays_overlap(
                            left.value(row).as_primitive::<Float64Type>(),
                            right.value(row).as_primitive::<Float64Type>(),
                        )
                    })
                    .collect::<Vec<_>>(),
                DataType::Date32 => overlap_primitive!(Date32Type),
                DataType::Date64 => overlap_primitive!(Date64Type),
                DataType::Time32(TimeUnit::Second) => overlap_primitive!(Time32SecondType),
                DataType::Time32(TimeUnit::Millisecond) => {
                    overlap_primitive!(Time32MillisecondType)
                }
                DataType::Timestamp(TimeUnit::Microsecond, _) => {
                    overlap_primitive!(TimestampMicrosecondType)
                }
                DataType::Decimal128(_, _) => (0..num_rows)
                    .map(|row| {
                        if left.is_null(row) || right.is_null(row) {
                            return None;
                        }
                        primitive_arrays_overlap(
                            left.value(row).as_primitive::<arrow::datatypes::Decimal128Type>(),
                            right.value(row).as_primitive::<arrow::datatypes::Decimal128Type>(),
                        )
                    })
                    .collect::<Vec<_>>(),
                DataType::Boolean => (0..num_rows)
                    .map(|row| {
                        if left.is_null(row) || right.is_null(row) {
                            return None;
                        }
                        let left_inner = left.value(row);
                        let right_inner = right.value(row);
                        let left_values = left_inner.as_boolean();
                        let right_values = right_inner.as_boolean();
                        let saw_null = left_values.null_count() > 0 || right_values.null_count() > 0;
                        for left_index in 0..left_values.len() {
                            if left_values.is_null(left_index) {
                                continue;
                            }
                            for right_index in 0..right_values.len() {
                                if right_values.is_null(right_index) {
                                } else if left_values.value(left_index) == right_values.value(right_index) {
                                    return Some(true);
                                }
                            }
                        }
                        if saw_null { None } else { Some(false) }
                    })
                    .collect::<Vec<_>>(),
                DataType::Utf8 => (0..num_rows)
                    .map(|row| {
                        if left.is_null(row) || right.is_null(row) {
                            return None;
                        }
                        string_arrays_overlap(
                            left.value(row).as_string::<i32>(),
                            right.value(row).as_string::<i32>(),
                        )
                    })
                    .collect::<Vec<_>>(),
                other => {
                    return Err(KaveonError::Execution(format!(
                        "ARRAYS_OVERLAP does not support list element type {other}"
                    )))
                }
            };
            Ok(Arc::new(BooleanArray::from(rows)))
        }
        "ARRAY_JOIN" => {
            if args.len() != 2 && args.len() != 3 {
                return Err(KaveonError::Execution(
                    "ARRAY_JOIN requires 2 or 3 arguments".into(),
                ));
            }
            let list = args[0]
                .as_any()
                .downcast_ref::<ListArray>()
                .ok_or_else(|| KaveonError::Execution("ARRAY_JOIN requires an ARRAY/List value".into()))?;
            let delimiters = as_string_array(&args[1])?;
            let replacements = if args.len() == 3 {
                Some(as_string_array(&args[2])?)
            } else {
                None
            };
            let result: StringArray = (0..num_rows)
                .map(|row| {
                    if list.is_null(row) || delimiters.is_null(row) {
                        return Ok(None);
                    }
                    let replacement = replacements.as_ref().and_then(|values| {
                        (!values.is_null(row)).then(|| values.value(row))
                    });
                    let values = list.value(row);
                    let mut parts = Vec::with_capacity(values.len());
                    for index in 0..values.len() {
                        if values.is_null(index) {
                            if let Some(replacement) = replacement {
                                parts.push(replacement.to_owned());
                            }
                        } else {
                            parts.push(array_join_value(&values, index)?);
                        }
                    }
                    Ok(Some(parts.join(delimiters.value(row))))
                })
                .collect::<Result<Vec<_>>>()?
                .into();
            Ok(Arc::new(result))
        }
        "CARDINALITY" => {
            check_arity(name, args, 1)?;
            if let Some(map) = args[0].as_any().downcast_ref::<MapArray>() {
                let values = (0..num_rows)
                    .map(|row| (!map.is_null(row)).then(|| map.value_length(row) as i64))
                    .collect::<Vec<_>>();
                return Ok(Arc::new(Int64Array::from(values)));
            }
            let list = args[0]
                .as_any()
                .downcast_ref::<arrow::array::ListArray>()
                .ok_or_else(|| {
                    KaveonError::Execution(
                        "CARDINALITY currently supports ARRAY/List values only".into(),
                    )
                })?;
            let values = (0..num_rows)
                .map(|row| (!list.is_null(row)).then(|| list.value_length(row) as i64))
                .collect::<Vec<_>>();
            Ok(Arc::new(Int64Array::from(values)))
        }
        other => Err(KaveonError::Execution(format!(
            "unknown scalar function: {other}"
        ))),
    }
}

fn eval_extract(field: DateField, expr: &Expr, batch: &RecordBatch) -> Result<ArrayRef> {
    let arr = evaluate(expr, batch)?;
    let num_rows = arr.len();
    match arr.data_type() {
        DataType::Timestamp(_, _) => {
            let micros: Vec<i64> = match arr.data_type() {
                DataType::Timestamp(arrow::datatypes::TimeUnit::Microsecond, _) => {
                    let ts = arr.as_primitive::<arrow::datatypes::TimestampMicrosecondType>();
                    (0..num_rows).map(|i| ts.value(i)).collect()
                }
                DataType::Timestamp(arrow::datatypes::TimeUnit::Millisecond, _) => {
                    let ts = arr.as_primitive::<arrow::datatypes::TimestampMillisecondType>();
                    (0..num_rows).map(|i| ts.value(i) * 1000).collect()
                }
                DataType::Timestamp(arrow::datatypes::TimeUnit::Second, _) => {
                    let ts = arr.as_primitive::<arrow::datatypes::TimestampSecondType>();
                    (0..num_rows).map(|i| ts.value(i) * 1_000_000).collect()
                }
                DataType::Timestamp(arrow::datatypes::TimeUnit::Nanosecond, _) => {
                    let ts = arr.as_primitive::<arrow::datatypes::TimestampNanosecondType>();
                    (0..num_rows).map(|i| ts.value(i) / 1000).collect()
                }
                _ => unreachable!(),
            };
            let result: Vec<Option<i64>> = micros
                .iter()
                .enumerate()
                .map(|(i, &us)| {
                    if arr.is_null(i) {
                        None
                    } else {
                        Some(extract_from_micros(us, field))
                    }
                })
                .collect();
            Ok(Arc::new(Int64Array::from(result)))
        }
        DataType::Date32 => {
            let days = arr.as_primitive::<arrow::datatypes::Date32Type>();
            let result: Vec<Option<i64>> = (0..num_rows)
                .map(|i| {
                    if days.is_null(i) {
                        None
                    } else {
                        let d = days.value(i) as i64;
                        Some(extract_from_micros(d * 86_400_000_000, field))
                    }
                })
                .collect();
            Ok(Arc::new(Int64Array::from(result)))
        }
        DataType::Date64 => {
            let millis = arr.as_primitive::<arrow::datatypes::Date64Type>();
            let result: Vec<Option<i64>> = (0..num_rows)
                .map(|i| {
                    if millis.is_null(i) {
                        None
                    } else {
                        Some(extract_from_micros(millis.value(i) * 1000, field))
                    }
                })
                .collect();
            Ok(Arc::new(Int64Array::from(result)))
        }
        dt => Err(KaveonError::Execution(format!(
            "EXTRACT not supported for type {dt}"
        ))),
    }
}

/// Extract a scalar at a conservative SQL/JSON path. The supported path is
/// `$` followed by dot-separated object keys and numeric array subscripts (for
/// example `$.customer.orders[0].id`). Arrays and objects are deliberately
/// rejected by the scalar function rather than being silently stringified.
fn json_extract_scalar(document: &str, path: &str) -> Option<String> {
    let value = json_extract_value(document, path)?;
    match value {
        serde_json::Value::Null | serde_json::Value::Array(_) | serde_json::Value::Object(_) => {
            None
        }
        serde_json::Value::String(text) => Some(text),
        serde_json::Value::Bool(value) => Some(value.to_string()),
        serde_json::Value::Number(value) => Some(value.to_string()),
    }
}

/// Extract a JSON value at a conservative SQL/JSON path. The result is
/// serialized JSON text so objects and arrays remain usable JSON rather than
/// being silently coerced to a scalar string.
fn json_extract(document: &str, path: &str) -> Option<String> {
    let value = json_extract_value(document, path)?;
    serde_json::to_string(&value).ok()
}

fn json_extract_value(document: &str, path: &str) -> Option<serde_json::Value> {
    enum Segment {
        Key(String),
        Index(usize),
    }
    let mut remainder = path.strip_prefix('$')?;
    let mut segments = Vec::new();
    while !remainder.is_empty() {
        if let Some(after_dot) = remainder.strip_prefix('.') {
            let end = after_dot
                .find(['.', '['])
                .unwrap_or(after_dot.len());
            let key = &after_dot[..end];
            if key.is_empty() {
                return None;
            }
            segments.push(Segment::Key(key.to_owned()));
            remainder = &after_dot[end..];
            continue;
        }
        if let Some(after_open) = remainder.strip_prefix('[') {
            let close = after_open.find(']')?;
            let index = after_open[..close].parse::<usize>().ok()?;
            segments.push(Segment::Index(index));
            remainder = &after_open[close + 1..];
            continue;
        }
        return None;
    }
    let mut value = serde_json::from_str::<serde_json::Value>(document).ok()?;
    for segment in segments {
        value = match segment {
            Segment::Key(key) => value.get(key)?.clone(),
            Segment::Index(index) => value.get(index)?.clone(),
        };
    }
    Some(value)
}

fn array_join_value(array: &ArrayRef, index: usize) -> Result<String> {
    Ok(match array.data_type() {
        DataType::Boolean => array.as_any().downcast_ref::<BooleanArray>().expect("boolean list values").value(index).to_string(),
        DataType::Int64 => array.as_any().downcast_ref::<Int64Array>().expect("int64 list values").value(index).to_string(),
        DataType::Float64 => array.as_any().downcast_ref::<Float64Array>().expect("float64 list values").value(index).to_string(),
        DataType::Utf8 => array.as_any().downcast_ref::<StringArray>().expect("utf8 list values").value(index).to_owned(),
        DataType::LargeUtf8 => array.as_any().downcast_ref::<arrow::array::LargeStringArray>().expect("large utf8 list values").value(index).to_owned(),
        DataType::Decimal128(_, scale) => {
            let value = array.as_any().downcast_ref::<Decimal128Array>().expect("decimal list values").value(index);
            format_decimal_for_array_join(value, *scale)
        }
        other => return Err(KaveonError::Execution(format!("ARRAY_JOIN does not support list element type {other}"))),
    })
}

fn map_keys_equal(left: &ArrayRef, left_index: usize, right_index: usize) -> Result<bool> {
    if left.is_null(left_index) || left.is_null(right_index) {
        return Ok(false);
    }
    Ok(match left.data_type() {
        DataType::Boolean => {
            let values = left.as_any().downcast_ref::<BooleanArray>().unwrap();
            values.value(left_index) == values.value(right_index)
        }
        DataType::Int64 => {
            let values = left.as_any().downcast_ref::<Int64Array>().unwrap();
            values.value(left_index) == values.value(right_index)
        }
        DataType::Float64 => {
            let values = left.as_any().downcast_ref::<Float64Array>().unwrap();
            values.value(left_index) == values.value(right_index)
        }
        DataType::Utf8 => {
            let values = left.as_any().downcast_ref::<StringArray>().unwrap();
            values.value(left_index) == values.value(right_index)
        }
        other => {
            return Err(KaveonError::Execution(format!(
                "MAP_CONCAT does not support key type {other}"
            )))
        }
    })
}

fn format_decimal_for_array_join(value: i128, scale: i8) -> String {
    if scale <= 0 {
        return value.checked_mul(10_i128.pow((-scale) as u32)).map(|value| value.to_string()).unwrap_or_else(|| value.to_string());
    }
    let negative = value < 0;
    let digits = value.unsigned_abs().to_string();
    let scale = scale as usize;
    let body = if digits.len() <= scale {
        format!("0.{}{}", "0".repeat(scale - digits.len()), digits)
    } else {
        let split = digits.len() - scale;
        format!("{}.{}", &digits[..split], &digits[split..])
    };
    if negative { format!("-{body}") } else { body }
}

fn extract_from_micros(micros: i64, field: DateField) -> i64 {
    let secs = micros / 1_000_000;
    let days = secs / 86400;
    let time_of_day = ((secs % 86400) + 86400) % 86400;

    let (year, month, day) = days_to_ymd(days);

    match field {
        DateField::Year => year as i64,
        DateField::Month => month as i64,
        DateField::Day => day as i64,
        DateField::Hour => time_of_day / 3600,
        DateField::Minute => (time_of_day % 3600) / 60,
        DateField::Second => time_of_day % 60,
        DateField::DayOfWeek => (days % 7 + 4 + 7) % 7,
        DateField::DayOfYear => {
            let jan1 = ymd_to_days(year, 1, 1);
            days - jan1 + 1
        }
        DateField::Quarter => ((month - 1) / 3 + 1) as i64,
        DateField::Week => {
            let jan1 = ymd_to_days(year, 1, 1);
            let doy = days - jan1;
            doy / 7 + 1
        }
        DateField::Epoch => secs,
    }
}

fn days_to_ymd(mut days: i64) -> (i32, u32, u32) {
    days += 719468;
    let era = if days >= 0 { days } else { days - 146096 } / 146097;
    let doe = (days - era * 146097) as u32;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = if m <= 2 { y + 1 } else { y };
    (year as i32, m, d)
}

fn ymd_to_days(y: i32, m: u32, d: u32) -> i64 {
    let y = y as i64;
    let m = m as i64;
    let y = if m <= 2 { y - 1 } else { y };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = (y - era * 400) as u32;
    let m_adj = if m > 2 { m - 3 } else { m + 9 } as u32;
    let doy = (153 * m_adj + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146097 + doe as i64 - 719468
}

fn eval_date_trunc(unit_arr: &StringArray, ts: &ArrayRef, num_rows: usize) -> Result<ArrayRef> {
    match ts.data_type() {
        DataType::Date32 => {
            let values = ts.as_primitive::<Date32Type>();
            let result = (0..num_rows)
                .map(|i| {
                    if unit_arr.is_null(i) || values.is_null(i) {
                        return Ok(None);
                    }
                    let unit = unit_arr.value(i).to_uppercase();
                    if !matches!(unit.as_str(), "YEAR" | "QUARTER" | "MONTH" | "WEEK" | "DAY")
                    {
                        return Err(KaveonError::Execution(format!(
                            "DATE_TRUNC unit '{unit}' is not valid for DATE values"
                        )));
                    }
                    let micros = i64::from(values.value(i)).saturating_mul(86_400_000_000);
                    Ok(Some((truncate_micros(micros, &unit) / 86_400_000_000) as i32))
                })
                .collect::<Result<Vec<_>>>()?;
            Ok(Arc::new(Date32Array::from(result)))
        }
        DataType::Timestamp(TimeUnit::Second, tz) => {
            let values = ts.as_primitive::<TimestampSecondType>();
            let result = truncate_timestamp_values(
                unit_arr,
                num_rows,
                |i| values.value(i),
                |i| values.is_null(i),
                1_000_000,
            );
            Ok(Arc::new(
                TimestampSecondArray::from(result).with_timezone_opt(tz.clone()),
            ))
        }
        DataType::Timestamp(TimeUnit::Millisecond, tz) => {
            let values = ts.as_primitive::<TimestampMillisecondType>();
            let result = truncate_timestamp_values(
                unit_arr,
                num_rows,
                |i| values.value(i),
                |i| values.is_null(i),
                1_000,
            );
            Ok(Arc::new(
                TimestampMillisecondArray::from(result).with_timezone_opt(tz.clone()),
            ))
        }
        DataType::Timestamp(TimeUnit::Microsecond, tz) => {
            let values = ts.as_primitive::<TimestampMicrosecondType>();
            let result = truncate_timestamp_values(
                unit_arr,
                num_rows,
                |i| values.value(i),
                |i| values.is_null(i),
                1,
            );
            Ok(Arc::new(
                TimestampMicrosecondArray::from(result).with_timezone_opt(tz.clone()),
            ))
        }
        DataType::Timestamp(TimeUnit::Nanosecond, tz) => {
            let values = ts.as_primitive::<TimestampNanosecondType>();
            let result = truncate_timestamp_values(
                unit_arr,
                num_rows,
                |i| values.value(i),
                |i| values.is_null(i),
                -1_000,
            );
            Ok(Arc::new(
                TimestampNanosecondArray::from(result).with_timezone_opt(tz.clone()),
            ))
        }
        _ => Err(KaveonError::Execution(
            "DATE_TRUNC requires a DATE or timestamp value".into(),
        )),
    }
}

fn eval_date_add_days(source: &ArrayRef, delta: &ArrayRef, num_rows: usize) -> Result<ArrayRef> {
    let delta = delta
        .as_any()
        .downcast_ref::<Int64Array>()
        .ok_or_else(|| KaveonError::Execution("DATE_ADD_DAYS requires an integer day count".into()))?;
    let day_at = |i: usize| -> Option<i64> {
        if delta.is_null(i) {
            None
        } else {
            Some(delta.value(i))
        }
    };
    match source.data_type() {
        DataType::Date32 => {
            let values = source.as_primitive::<Date32Type>();
            let result = (0..num_rows)
                .map(|i| {
                    if values.is_null(i) {
                        None
                    } else {
                        day_at(i).and_then(|days| {
                            i32::try_from(days)
                                .ok()
                                .and_then(|days| values.value(i).checked_add(days))
                        })
                    }
                })
                .collect::<Vec<_>>();
            Ok(Arc::new(Date32Array::from(result)))
        }
        DataType::Date64 => {
            let values = source.as_primitive::<Date64Type>();
            let result = (0..num_rows)
                .map(|i| day_at(i).and_then(|days| {
                    values
                        .value(i)
                        .checked_add(days.checked_mul(86_400_000)?)
                }))
                .collect::<Vec<_>>();
            Ok(Arc::new(Date64Array::from(result)))
        }
        DataType::Timestamp(TimeUnit::Second, tz) => {
            add_timestamp_days::<TimestampSecondType, TimestampSecondArray>(
                source, delta, num_rows, 86_400, tz.clone()
            )
        }
        DataType::Timestamp(TimeUnit::Millisecond, tz) => {
            add_timestamp_days::<TimestampMillisecondType, TimestampMillisecondArray>(
                source, delta, num_rows, 86_400_000, tz.clone()
            )
        }
        DataType::Timestamp(TimeUnit::Microsecond, tz) => {
            add_timestamp_days::<TimestampMicrosecondType, TimestampMicrosecondArray>(
                source, delta, num_rows, 86_400_000_000, tz.clone()
            )
        }
        DataType::Timestamp(TimeUnit::Nanosecond, tz) => {
            add_timestamp_days::<TimestampNanosecondType, TimestampNanosecondArray>(
                source, delta, num_rows, 86_400_000_000_000, tz.clone()
            )
        }
        other => Err(KaveonError::Execution(format!(
            "DATE_ADD_DAYS requires DATE or timestamp input, got {other}"
        ))),
    }
}

fn eval_date_add_months(source: &ArrayRef, delta: &ArrayRef, num_rows: usize) -> Result<ArrayRef> {
    let delta = delta
        .as_any()
        .downcast_ref::<Int64Array>()
        .ok_or_else(|| KaveonError::Execution("DATE_ADD_MONTHS requires an integer month count".into()))?;
    match source.data_type() {
        DataType::Date32 => {
            let values = source.as_primitive::<Date32Type>();
            let result = (0..num_rows)
                .map(|i| {
                    if values.is_null(i) || delta.is_null(i) {
                        None
                    } else {
                        i32::try_from(add_months_to_days(values.value(i).into(), delta.value(i))).ok()
                    }
                })
                .collect::<Vec<_>>();
            Ok(Arc::new(Date32Array::from(result)))
        }
        DataType::Date64 => {
            let values = source.as_primitive::<Date64Type>();
            let result = (0..num_rows)
                .map(|i| {
                    if values.is_null(i) || delta.is_null(i) {
                        None
                    } else {
                        add_months_timestamp_value(values.value(i), delta.value(i), 86_400_000)
                    }
                })
                .collect::<Vec<_>>();
            Ok(Arc::new(Date64Array::from(result)))
        }
        DataType::Timestamp(TimeUnit::Second, tz) => add_timestamp_months::<TimestampSecondType, TimestampSecondArray>(
            source, delta, num_rows, 86_400, tz.clone()
        ),
        DataType::Timestamp(TimeUnit::Millisecond, tz) => add_timestamp_months::<TimestampMillisecondType, TimestampMillisecondArray>(
            source, delta, num_rows, 86_400_000, tz.clone()
        ),
        DataType::Timestamp(TimeUnit::Microsecond, tz) => add_timestamp_months::<TimestampMicrosecondType, TimestampMicrosecondArray>(
            source, delta, num_rows, 86_400_000_000, tz.clone()
        ),
        DataType::Timestamp(TimeUnit::Nanosecond, tz) => add_timestamp_months::<TimestampNanosecondType, TimestampNanosecondArray>(
            source, delta, num_rows, 86_400_000_000_000, tz.clone()
        ),
        other => Err(KaveonError::Execution(format!(
            "DATE_ADD_MONTHS requires DATE or timestamp input, got {other}"
        ))),
    }
}

fn eval_date_add_seconds(source: &ArrayRef, delta: &ArrayRef, num_rows: usize) -> Result<ArrayRef> {
    let delta = delta
        .as_any()
        .downcast_ref::<Int64Array>()
        .ok_or_else(|| KaveonError::Execution("DATE_ADD_SECONDS requires an integer second count".into()))?;
    match source.data_type() {
        DataType::Timestamp(TimeUnit::Second, tz) => add_timestamp_seconds::<TimestampSecondType, TimestampSecondArray>(source, delta, num_rows, 1, tz.clone()),
        DataType::Timestamp(TimeUnit::Millisecond, tz) => add_timestamp_seconds::<TimestampMillisecondType, TimestampMillisecondArray>(source, delta, num_rows, 1_000, tz.clone()),
        DataType::Timestamp(TimeUnit::Microsecond, tz) => add_timestamp_seconds::<TimestampMicrosecondType, TimestampMicrosecondArray>(source, delta, num_rows, 1_000_000, tz.clone()),
        DataType::Timestamp(TimeUnit::Nanosecond, tz) => add_timestamp_seconds::<TimestampNanosecondType, TimestampNanosecondArray>(source, delta, num_rows, 1_000_000_000, tz.clone()),
        other => Err(KaveonError::Execution(format!(
            "DATE_ADD_SECONDS requires a timestamp input, got {other}"
        ))),
    }
}

fn add_timestamp_seconds<T, A>(
    source: &ArrayRef,
    delta: &Int64Array,
    num_rows: usize,
    scale: i64,
    timezone: Option<Arc<str>>,
) -> Result<ArrayRef>
where
    T: arrow::datatypes::ArrowPrimitiveType<Native = i64>,
    A: From<Vec<Option<i64>>> + Array + 'static,
{
    let values = source.as_primitive::<T>();
    let result = (0..num_rows)
        .map(|i| {
            if values.is_null(i) || delta.is_null(i) {
                None
            } else {
                delta.value(i).checked_mul(scale).and_then(|offset| values.value(i).checked_add(offset))
            }
        })
        .collect::<Vec<_>>();
    let array: ArrayRef = Arc::new(A::from(result));
    Ok(match source.data_type() {
        DataType::Timestamp(TimeUnit::Second, _) => Arc::new(array.as_any().downcast_ref::<TimestampSecondArray>().unwrap().clone().with_timezone_opt(timezone)),
        DataType::Timestamp(TimeUnit::Millisecond, _) => Arc::new(array.as_any().downcast_ref::<TimestampMillisecondArray>().unwrap().clone().with_timezone_opt(timezone)),
        DataType::Timestamp(TimeUnit::Microsecond, _) => Arc::new(array.as_any().downcast_ref::<TimestampMicrosecondArray>().unwrap().clone().with_timezone_opt(timezone)),
        DataType::Timestamp(TimeUnit::Nanosecond, _) => Arc::new(array.as_any().downcast_ref::<TimestampNanosecondArray>().unwrap().clone().with_timezone_opt(timezone)),
        _ => array,
    })
}

fn add_months_timestamp_value(value: i64, months: i64, day_scale: i64) -> Option<i64> {
    let days = value.div_euclid(day_scale);
    let remainder = value.rem_euclid(day_scale);
    add_months_to_days(days, months)
        .checked_mul(day_scale)?
        .checked_add(remainder)
}

fn add_months_to_days(days: i64, months: i64) -> i64 {
    let (year, month, day) = days_to_ymd(days);
    let total = i64::from(year) * 12 + i64::from(month - 1) + months;
    let year = total.div_euclid(12) as i32;
    let month = total.rem_euclid(12) as u32 + 1;
    let day = day.min(days_in_month(year, month));
    ymd_to_days(year, month, day)
}

fn days_in_month(year: i32, month: u32) -> u32 {
    match month {
        2 if year % 4 == 0 && (year % 100 != 0 || year % 400 == 0) => 29,
        2 => 28,
        4 | 6 | 9 | 11 => 30,
        _ => 31,
    }
}

fn add_timestamp_months<T, A>(
    source: &ArrayRef,
    delta: &Int64Array,
    num_rows: usize,
    day_scale: i64,
    timezone: Option<Arc<str>>,
) -> Result<ArrayRef>
where
    T: arrow::datatypes::ArrowPrimitiveType<Native = i64>,
    A: From<Vec<Option<i64>>> + Array + 'static,
{
    let values = source.as_primitive::<T>();
    let result = (0..num_rows)
        .map(|i| {
            if values.is_null(i) || delta.is_null(i) {
                None
            } else {
                add_months_timestamp_value(values.value(i), delta.value(i), day_scale)
            }
        })
        .collect::<Vec<_>>();
    let array: ArrayRef = Arc::new(A::from(result));
    Ok(match source.data_type() {
        DataType::Timestamp(TimeUnit::Second, _) => Arc::new(
            array.as_any().downcast_ref::<TimestampSecondArray>().unwrap().clone().with_timezone_opt(timezone),
        ),
        DataType::Timestamp(TimeUnit::Millisecond, _) => Arc::new(
            array.as_any().downcast_ref::<TimestampMillisecondArray>().unwrap().clone().with_timezone_opt(timezone),
        ),
        DataType::Timestamp(TimeUnit::Microsecond, _) => Arc::new(
            array.as_any().downcast_ref::<TimestampMicrosecondArray>().unwrap().clone().with_timezone_opt(timezone),
        ),
        DataType::Timestamp(TimeUnit::Nanosecond, _) => Arc::new(
            array.as_any().downcast_ref::<TimestampNanosecondArray>().unwrap().clone().with_timezone_opt(timezone),
        ),
        _ => array,
    })
}

fn add_timestamp_days<T, A>(
    source: &ArrayRef,
    delta: &Int64Array,
    num_rows: usize,
    scale: i64,
    timezone: Option<Arc<str>>,
) -> Result<ArrayRef>
where
    T: arrow::datatypes::ArrowPrimitiveType<Native = i64>,
    A: From<Vec<Option<i64>>> + Array + 'static,
{
    let values = source.as_primitive::<T>();
    let result = (0..num_rows)
        .map(|i| {
            if values.is_null(i) || delta.is_null(i) {
                None
            } else {
                delta
                    .value(i)
                    .checked_mul(scale)
                    .and_then(|offset| values.value(i).checked_add(offset))
            }
        })
        .collect::<Vec<_>>();
    let array = A::from(result);
    let array: ArrayRef = Arc::new(array);
    if let DataType::Timestamp(_, _) = source.data_type() {
        // The concrete timestamp array already has its source unit; restore
        // its timezone metadata without changing the value representation.
        return Ok(match source.data_type() {
            DataType::Timestamp(TimeUnit::Second, _) => Arc::new(
                array
                    .as_any()
                    .downcast_ref::<TimestampSecondArray>()
                    .expect("timestamp array type")
                    .clone()
                    .with_timezone_opt(timezone),
            ),
            DataType::Timestamp(TimeUnit::Millisecond, _) => Arc::new(
                array
                    .as_any()
                    .downcast_ref::<TimestampMillisecondArray>()
                    .expect("timestamp array type")
                    .clone()
                    .with_timezone_opt(timezone),
            ),
            DataType::Timestamp(TimeUnit::Microsecond, _) => Arc::new(
                array
                    .as_any()
                    .downcast_ref::<TimestampMicrosecondArray>()
                    .expect("timestamp array type")
                    .clone()
                    .with_timezone_opt(timezone),
            ),
            DataType::Timestamp(TimeUnit::Nanosecond, _) => Arc::new(
                array
                    .as_any()
                    .downcast_ref::<TimestampNanosecondArray>()
                    .expect("timestamp array type")
                    .clone()
                    .with_timezone_opt(timezone),
            ),
            _ => unreachable!(),
        });
    }
    Ok(array)
}

/// Truncate timestamps at microsecond precision, then convert the result back
/// to the source unit. `scale` is the number of source units per microsecond;
/// nanoseconds use its negative reciprocal to avoid an overflowing scale.
fn truncate_timestamp_values(
    unit_arr: &StringArray,
    num_rows: usize,
    value: impl Fn(usize) -> i64,
    is_null: impl Fn(usize) -> bool,
    scale: i64,
) -> Vec<Option<i64>> {
    (0..num_rows)
        .map(|i| {
            if unit_arr.is_null(i) || is_null(i) {
                return None;
            }
            let raw = value(i);
            let micros = if scale == -1_000 {
                raw / 1_000
            } else {
                raw.saturating_mul(scale)
            };
            let truncated = truncate_micros(micros, &unit_arr.value(i).to_uppercase());
            Some(if scale == -1_000 {
                truncated.saturating_mul(1_000)
            } else {
                truncated / scale
            })
        })
        .collect()
}

fn truncate_micros(us: i64, unit: &str) -> i64 {
    // Calendar truncation is a floor operation. Euclidean division keeps
    // timestamps before 1970 in the preceding day/hour rather than rounding
    // them toward the epoch.
    let secs = us.div_euclid(1_000_000);
    let days = secs.div_euclid(86400);
    let (year, month, _day) = days_to_ymd(days);
    match unit {
        "YEAR" => ymd_to_days(year, 1, 1) * 86_400_000_000,
        "QUARTER" => {
            let q = (month - 1) / 3;
            ymd_to_days(year, q * 3 + 1, 1) * 86_400_000_000
        }
        "MONTH" => ymd_to_days(year, month, 1) * 86_400_000_000,
        "WEEK" => {
            let dow = (days % 7 + 4 + 7) % 7;
            (days - dow) * 86_400_000_000
        }
        "DAY" => days * 86_400_000_000,
        "HOUR" => secs.div_euclid(3600) * 3_600_000_000,
        "MINUTE" => secs.div_euclid(60) * 60_000_000,
        "SECOND" => secs * 1_000_000,
        _ => us,
    }
}

fn eval_date_part(field_arr: &StringArray, source: &ArrayRef, num_rows: usize) -> Result<ArrayRef> {
    let result: Vec<Option<i64>> = (0..num_rows)
        .map(|i| {
            if field_arr.is_null(i) {
                return Ok(None);
            }
            let field_str = field_arr.value(i).to_uppercase();
            let field = match field_str.as_str() {
                "YEAR" => DateField::Year,
                "MONTH" => DateField::Month,
                "DAY" => DateField::Day,
                "HOUR" => DateField::Hour,
                "MINUTE" => DateField::Minute,
                "SECOND" => DateField::Second,
                "DOW" | "DAYOFWEEK" => DateField::DayOfWeek,
                "DOY" | "DAYOFYEAR" => DateField::DayOfYear,
                "QUARTER" => DateField::Quarter,
                "WEEK" => DateField::Week,
                "EPOCH" => DateField::Epoch,
                _ => {
                    return Err(KaveonError::Execution(format!(
                        "unsupported DATE_PART field: {field_str}"
                    )));
                }
            };
            match source.data_type() {
                DataType::Timestamp(arrow::datatypes::TimeUnit::Microsecond, _) => {
                    let ts = source.as_primitive::<arrow::datatypes::TimestampMicrosecondType>();
                    if ts.is_null(i) {
                        Ok(None)
                    } else {
                        Ok(Some(extract_from_micros(ts.value(i), field)))
                    }
                }
                DataType::Date32 => {
                    let d = source.as_primitive::<arrow::datatypes::Date32Type>();
                    if d.is_null(i) {
                        Ok(None)
                    } else {
                        Ok(Some(extract_from_micros(
                            d.value(i) as i64 * 86_400_000_000,
                            field,
                        )))
                    }
                }
                _ => Err(KaveonError::Execution(format!(
                    "DATE_PART not supported for type {}",
                    source.data_type()
                ))),
            }
        })
        .collect::<Result<_>>()?;
    Ok(Arc::new(Int64Array::from(result)))
}

/// Return a temporal value as nanoseconds from the Unix epoch.  Keeping the
/// intermediate in `i128` lets DATE_DIFF compare any pair of supported Arrow
/// time units without overflowing while converting a nanosecond timestamp.
fn temporal_nanoseconds(source: &ArrayRef, row: usize) -> Result<Option<i128>> {
    let value = match source.data_type() {
        // DATE literals are represented as epoch-day INT64 values by the SQL
        // planner before a table schema is available.
        DataType::Int64 => {
            let values = source.as_primitive::<Int64Type>();
            values.is_null(row).then_some(None).unwrap_or_else(|| {
                Some((values.value(row) as i128) * 86_400_000_000_000i128)
            })
        }
        DataType::Date32 => {
            let values = source.as_primitive::<Date32Type>();
            values.is_null(row).then_some(None).unwrap_or_else(|| {
                Some((values.value(row) as i128) * 86_400_000_000_000i128)
            })
        }
        DataType::Date64 => {
            let values = source.as_primitive::<Date64Type>();
            values.is_null(row).then_some(None).unwrap_or_else(|| {
                Some((values.value(row) as i128) * 1_000_000i128)
            })
        }
        DataType::Timestamp(TimeUnit::Second, _) => {
            let values = source.as_primitive::<TimestampSecondType>();
            values.is_null(row).then_some(None).unwrap_or_else(|| {
                Some((values.value(row) as i128) * 1_000_000_000i128)
            })
        }
        DataType::Timestamp(TimeUnit::Millisecond, _) => {
            let values = source.as_primitive::<TimestampMillisecondType>();
            values.is_null(row).then_some(None).unwrap_or_else(|| {
                Some((values.value(row) as i128) * 1_000_000i128)
            })
        }
        DataType::Timestamp(TimeUnit::Microsecond, _) => {
            let values = source.as_primitive::<TimestampMicrosecondType>();
            values.is_null(row).then_some(None).unwrap_or_else(|| {
                Some((values.value(row) as i128) * 1_000i128)
            })
        }
        DataType::Timestamp(TimeUnit::Nanosecond, _) => {
            let values = source.as_primitive::<TimestampNanosecondType>();
            values.is_null(row).then_some(None).unwrap_or_else(|| {
                Some(values.value(row) as i128)
            })
        }
        other => {
            return Err(KaveonError::Execution(format!(
                "DATE_DIFF requires DATE or timestamp inputs, got {other}"
            )))
        }
    };
    Ok(value)
}

fn eval_date_diff(
    unit_arr: &StringArray,
    start: &ArrayRef,
    end: &ArrayRef,
    num_rows: usize,
) -> Result<ArrayRef> {
    if start.len() != end.len() || start.len() != unit_arr.len() {
        return Err(KaveonError::Execution(
            "DATE_DIFF arguments must have the same length".into(),
        ));
    }
    let result = (0..num_rows)
        .map(|row| {
            if unit_arr.is_null(row) {
                return Ok(None);
            }
            let unit = unit_arr.value(row).to_ascii_uppercase();
            let divisor = match unit.as_str() {
                "NANOSECOND" => 1i128,
                "MICROSECOND" => 1_000i128,
                "MILLISECOND" => 1_000_000i128,
                "SECOND" => 1_000_000_000i128,
                "MINUTE" => 60_000_000_000i128,
                "HOUR" => 3_600_000_000_000i128,
                "DAY" => 86_400_000_000_000i128,
                "WEEK" => 604_800_000_000_000i128,
                unit => {
                    if matches!(unit, "MONTH" | "QUARTER" | "YEAR") {
                        let Some(start) = temporal_calendar_position(start, row)? else {
                            return Ok(None);
                        };
                        let Some(end) = temporal_calendar_position(end, row)? else {
                            return Ok(None);
                        };
                        let months = calendar_month_difference(start, end);
                        let result = match unit {
                            "MONTH" => months,
                            "QUARTER" => months / 3,
                            "YEAR" => months / 12,
                            _ => unreachable!(),
                        };
                        return i64::try_from(result).map(Some).map_err(|_| {
                            KaveonError::Execution("DATE_DIFF result overflows BIGINT".into())
                        });
                    }
                    return Err(KaveonError::Execution(format!(
                        "unsupported DATE_DIFF unit: {unit}"
                    )));
                }
            };
            let Some(start) = temporal_nanoseconds(start, row)? else {
                return Ok(None);
            };
            let Some(end) = temporal_nanoseconds(end, row)? else {
                return Ok(None);
            };
            let difference = (end - start) / divisor;
            i64::try_from(difference)
                .map(Some)
                .map_err(|_| KaveonError::Execution("DATE_DIFF result overflows BIGINT".into()))
        })
        .collect::<Result<Vec<_>>>()?;
    Ok(Arc::new(Int64Array::from(result)))
}

/// Calendar position used by DATE_DIFF's month-family units. The remainder
/// makes timestamps on the same day obey the same full-boundary rule as
/// dates: a boundary has not elapsed when the end's time is before the
/// start's time.
fn temporal_calendar_position(
    source: &ArrayRef,
    row: usize,
) -> Result<Option<(i32, u32, u32, i128)>> {
    let Some(nanoseconds) = temporal_nanoseconds(source, row)? else {
        return Ok(None);
    };
    let day = nanoseconds.div_euclid(86_400_000_000_000i128);
    let remainder = nanoseconds.rem_euclid(86_400_000_000_000i128);
    let day = i64::try_from(day)
        .map_err(|_| KaveonError::Execution("DATE_DIFF date is outside the supported range".into()))?;
    let (year, month, date) = days_to_ymd(day);
    Ok(Some((year, month, date, remainder)))
}

fn calendar_month_difference(
    start: (i32, u32, u32, i128),
    end: (i32, u32, u32, i128),
) -> i64 {
    let start_month = i64::from(start.0) * 12 + i64::from(start.1) - 1;
    let end_month = i64::from(end.0) * 12 + i64::from(end.1) - 1;
    let raw = end_month - start_month;
    let start_position = (start.2, start.3);
    let end_position = (end.2, end.3);
    if raw > 0 && end_position < start_position {
        raw - 1
    } else if raw < 0 && end_position > start_position {
        raw + 1
    } else {
        raw
    }
}

fn eval_to_char(source: &ArrayRef, _fmt: &StringArray, num_rows: usize) -> Result<ArrayRef> {
    match source.data_type() {
        DataType::Timestamp(arrow::datatypes::TimeUnit::Microsecond, _) => {
            let ts = source.as_primitive::<arrow::datatypes::TimestampMicrosecondType>();
            let result: StringArray = (0..num_rows)
                .map(|i| {
                    if ts.is_null(i) {
                        None
                    } else {
                        let us = ts.value(i);
                        let secs = us / 1_000_000;
                        let days = secs / 86400;
                        let tod = ((secs % 86400) + 86400) % 86400;
                        let (y, m, d) = days_to_ymd(days);
                        Some(format!(
                            "{:04}-{:02}-{:02} {:02}:{:02}:{:02}",
                            y,
                            m,
                            d,
                            tod / 3600,
                            (tod % 3600) / 60,
                            tod % 60
                        ))
                    }
                })
                .collect();
            Ok(Arc::new(result))
        }
        DataType::Date32 => {
            let d = source.as_primitive::<arrow::datatypes::Date32Type>();
            let result: StringArray = (0..num_rows)
                .map(|i| {
                    if d.is_null(i) {
                        None
                    } else {
                        let (y, m, day) = days_to_ymd(d.value(i) as i64);
                        Some(format!("{:04}-{:02}-{:02}", y, m, day))
                    }
                })
                .collect();
            Ok(Arc::new(result))
        }
        dt => Err(KaveonError::Execution(format!(
            "TO_CHAR not supported for type {dt}"
        ))),
    }
}

fn check_arity(name: &str, args: &[ArrayRef], expected: usize) -> Result<()> {
    if args.len() != expected {
        return Err(KaveonError::Execution(format!(
            "{name} requires {expected} argument(s), got {}",
            args.len()
        )));
    }
    Ok(())
}

fn unary_float_fn(arr: &ArrayRef, f: fn(f64) -> f64) -> Result<ArrayRef> {
    let casted = compute::cast(arr, &DataType::Float64)?;
    let vals = casted.as_primitive::<Float64Type>();
    let result: Float64Array = vals.iter().map(|v| v.map(f)).collect();
    Ok(Arc::new(result))
}

fn make_null_array(dt: &DataType, len: usize) -> Result<ArrayRef> {
    match dt {
        DataType::Int64 => Ok(Arc::new(Int64Array::new_null(len))),
        DataType::Float64 => Ok(Arc::new(Float64Array::new_null(len))),
        DataType::Int32 => Ok(Arc::new(Int32Array::new_null(len))),
        DataType::Utf8 => Ok(Arc::new(StringArray::new_null(len))),
        DataType::Boolean => Ok(Arc::new(BooleanArray::new_null(len))),
        _ => Err(KaveonError::Execution(format!(
            "cannot make null array for type {dt}"
        ))),
    }
}

/// Fold GREATEST/LEAST over a common Arrow type while preserving nulls.
///
/// The old implementation cast every argument to Float64.  Besides losing
/// integer, decimal, date, and text types, that made a NULL comparison choose
/// the non-null branch.  Trino returns NULL when any argument in the row is
/// NULL, so validity is applied after each pairwise comparison.
fn fold_extreme(rest: &[ArrayRef], first: &ArrayRef, kind: CompareKind) -> Result<ArrayRef> {
    let mut result = Arc::clone(first);
    for arg in rest {
        let (left, right) = coerce_numeric_pair(&result, arg)?;
        let choose_left = comparison(&left, &right, kind)?;
        let selected = zip_arrays(&choose_left, &left, &right)?;
        let left_valid = compute::is_not_null(&left)?;
        let right_valid = compute::is_not_null(&right)?;
        let valid = compute::and_kleene(&left_valid, &right_valid)?;
        let nulls = arrow::array::new_null_array(selected.data_type(), selected.len());
        result = zip_arrays(&valid, &selected, &nulls)?;
    }
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn in_list_uses_three_valued_null_logic() {
        let schema = Arc::new(arrow::datatypes::Schema::new(vec![
            arrow::datatypes::Field::new("x", DataType::Int64, true),
        ]));
        let input = RecordBatch::try_new(
            schema,
            vec![Arc::new(Int64Array::from(vec![Some(1), Some(2), None]))],
        )
        .unwrap();
        for negated in [false, true] {
            let expr = Expr::InList {
                expr: Box::new(Expr::Column("x".into())),
                list: vec![
                    Expr::Literal(ScalarValue::Int64(1)),
                    Expr::Literal(ScalarValue::Null),
                ],
                negated,
            };
            let values = evaluate(&expr, &input).unwrap();
            assert_eq!(
                values.as_boolean().iter().collect::<Vec<_>>(),
                vec![Some(!negated), None, None]
            );
        }
    }

    #[test]
    fn null_safe_comparison_case_semantics_are_total() {
        let schema = Arc::new(arrow::datatypes::Schema::new(vec![
            arrow::datatypes::Field::new("left", DataType::Int64, true),
            arrow::datatypes::Field::new("right", DataType::Int64, true),
        ]));
        let input = RecordBatch::try_new(
            schema,
            vec![
                Arc::new(Int64Array::from(vec![Some(1), None, Some(1), None])),
                Arc::new(Int64Array::from(vec![Some(1), None, None, Some(2)])),
            ],
        )
        .unwrap();
        let distinct = |not_distinct| Expr::Case {
            operand: None,
            when_then: vec![
                (
                    Expr::IsNull(Box::new(Expr::Column("left".into()))),
                    if not_distinct {
                        Expr::IsNull(Box::new(Expr::Column("right".into())))
                    } else {
                        Expr::IsNotNull(Box::new(Expr::Column("right".into())))
                    },
                ),
                (
                    Expr::IsNull(Box::new(Expr::Column("right".into()))),
                    Expr::Literal(ScalarValue::Bool(!not_distinct)),
                ),
            ],
            else_expr: Some(Box::new(Expr::BinaryOp {
                left: Box::new(Expr::Column("left".into())),
                op: if not_distinct {
                    BinaryOp::Eq
                } else {
                    BinaryOp::Ne
                },
                right: Box::new(Expr::Column("right".into())),
            })),
        };
        let values = evaluate(&distinct(false), &input).unwrap();
        assert_eq!(
            values.as_boolean().iter().collect::<Vec<_>>(),
            vec![Some(false), Some(false), Some(true), Some(true)]
        );
        let values = evaluate(&distinct(true), &input).unwrap();
        assert_eq!(
            values.as_boolean().iter().collect::<Vec<_>>(),
            vec![Some(true), Some(true), Some(false), Some(false)]
        );
    }

    #[test]
    fn try_cast_returns_null_only_for_invalid_values() {
        let schema = Arc::new(arrow::datatypes::Schema::new(vec![
            arrow::datatypes::Field::new("value", DataType::Utf8, true),
        ]));
        let input = RecordBatch::try_new(
            schema,
            vec![Arc::new(StringArray::from(vec![
                Some("12"),
                Some("bad"),
                None,
                Some("-4"),
            ]))],
        )
        .unwrap();
        let values = evaluate(
            &Expr::Function {
                name: "TRY_CAST_INT64".into(),
                args: vec![Expr::Column("value".into())],
            },
            &input,
        )
        .unwrap();
        assert_eq!(
            values
                .as_primitive::<Int64Type>()
                .iter()
                .collect::<Vec<_>>(),
            vec![Some(12), None, None, Some(-4)]
        );
    }

    #[test]
    fn date_trunc_preserves_timestamp_units_and_nulls() {
        let schema = Arc::new(arrow::datatypes::Schema::new(vec![
            arrow::datatypes::Field::new(
                "ts",
                DataType::Timestamp(TimeUnit::Millisecond, None),
                true,
            ),
        ]));
        let source_ms = 1_715_780_055_123_i64;
        let input = RecordBatch::try_new(
            schema,
            vec![Arc::new(
                TimestampMillisecondArray::from(vec![Some(source_ms), None, Some(-1)]),
            )],
        )
        .unwrap();
        let values = evaluate(
            &Expr::Function {
                name: "DATE_TRUNC".into(),
                args: vec![
                    Expr::Literal(ScalarValue::Utf8("DAY".into())),
                    Expr::Column("ts".into()),
                ],
            },
            &input,
        )
        .unwrap();
        let result = values
            .as_primitive::<TimestampMillisecondType>()
            .iter()
            .collect::<Vec<_>>();
        assert_eq!(result[0], Some(1_715_731_200_000));
        assert_eq!(result[1], None);
        assert_eq!(result[2], Some(-86_400_000));

        let nanosecond_schema = Arc::new(arrow::datatypes::Schema::new(vec![
            arrow::datatypes::Field::new(
                "ts",
                DataType::Timestamp(TimeUnit::Nanosecond, None),
                true,
            ),
        ]));
        let nanos = RecordBatch::try_new(
            nanosecond_schema,
            vec![Arc::new(TimestampNanosecondArray::from(vec![Some(
                source_ms * 1_000_000 + 999,
            )]))],
        )
        .unwrap();
        let values = evaluate(
            &Expr::Function {
                name: "DATE_TRUNC".into(),
                args: vec![
                    Expr::Literal(ScalarValue::Utf8("SECOND".into())),
                    Expr::Column("ts".into()),
                ],
            },
            &nanos,
        )
        .unwrap();
        assert_eq!(
            values
                .as_primitive::<TimestampNanosecondType>()
                .value(0),
            (source_ms / 1_000) * 1_000_000_000
        );

        let date_schema = Arc::new(arrow::datatypes::Schema::new(vec![
            arrow::datatypes::Field::new("day", DataType::Date32, true),
        ]));
        let dates = RecordBatch::try_new(
            date_schema,
            vec![Arc::new(Date32Array::from(vec![
                Some(19_858),
                None,
                Some(-1),
            ]))],
        )
        .unwrap();
        let values = evaluate(
            &Expr::Function {
                name: "DATE_TRUNC".into(),
                args: vec![
                    Expr::Literal(ScalarValue::Utf8("MONTH".into())),
                    Expr::Column("day".into()),
                ],
            },
            &dates,
        )
        .unwrap();
        let result = values.as_primitive::<Date32Type>();
        assert_eq!(result.value(0), 19_844);
        assert!(result.is_null(1));
        assert_eq!(result.value(2), -31);

        let error = evaluate(
            &Expr::Function {
                name: "DATE_TRUNC".into(),
                args: vec![
                    Expr::Literal(ScalarValue::Utf8("HOUR".into())),
                    Expr::Column("day".into()),
                ],
            },
            &dates,
        )
        .unwrap_err();
        assert!(error.to_string().contains("not valid for DATE"), "{error}");
    }

    #[test]
    fn date_add_days_preserves_temporal_type_and_nulls() {
        let schema = Arc::new(arrow::datatypes::Schema::new(vec![
            arrow::datatypes::Field::new("day", DataType::Date32, true),
        ]));
        let input = RecordBatch::try_new(
            schema,
            vec![Arc::new(Date32Array::from(vec![Some(19_858), None, Some(-1)]))],
        )
        .unwrap();
        let values = evaluate(
            &Expr::Function {
                name: "DATE_ADD_DAYS".into(),
                args: vec![
                    Expr::Column("day".into()),
                    Expr::Literal(ScalarValue::Int64(7)),
                ],
            },
            &input,
        )
        .unwrap();
        let result = values.as_primitive::<Date32Type>();
        assert_eq!(result.value(0), 19_865);
        assert!(result.is_null(1));
        assert_eq!(result.value(2), 6);

        let timestamp_schema = Arc::new(arrow::datatypes::Schema::new(vec![
            arrow::datatypes::Field::new(
                "ts",
                DataType::Timestamp(TimeUnit::Millisecond, None),
                true,
            ),
        ]));
        let timestamp = RecordBatch::try_new(
            timestamp_schema,
            vec![Arc::new(TimestampMillisecondArray::from(vec![Some(0), None]))],
        )
        .unwrap();
        let values = evaluate(
            &Expr::Function {
                name: "DATE_ADD_DAYS".into(),
                args: vec![
                    Expr::Column("ts".into()),
                    Expr::Literal(ScalarValue::Int64(-1)),
                ],
            },
            &timestamp,
        )
        .unwrap();
        let result = values.as_primitive::<TimestampMillisecondType>();
        assert_eq!(result.value(0), -86_400_000);
        assert!(result.is_null(1));
    }

    #[test]
    fn date_diff_supports_fixed_units_and_nulls() {
        let schema = Arc::new(arrow::datatypes::Schema::new(vec![
            arrow::datatypes::Field::new(
                "start",
                DataType::Timestamp(TimeUnit::Millisecond, None),
                true,
            ),
            arrow::datatypes::Field::new(
                "end",
                DataType::Timestamp(TimeUnit::Microsecond, None),
                true,
            ),
        ]));
        let input = RecordBatch::try_new(
            schema,
            vec![
                Arc::new(TimestampMillisecondArray::from(vec![
                    Some(0),
                    Some(3_600_000),
                    None,
                ])),
                Arc::new(TimestampMicrosecondArray::from(vec![
                    Some(86_400_000_000),
                    Some(7_200_000_000),
                    Some(1_000),
                ])),
            ],
        )
        .unwrap();
        let values = evaluate(
            &Expr::Function {
                name: "DATE_DIFF".into(),
                args: vec![
                    Expr::Literal(ScalarValue::Utf8("DAY".into())),
                    Expr::Column("start".into()),
                    Expr::Column("end".into()),
                ],
            },
            &input,
        )
        .unwrap();
        assert_eq!(
            values
                .as_primitive::<Int64Type>()
                .iter()
                .collect::<Vec<_>>(),
            vec![Some(1), Some(0), None]
        );

        let values = evaluate(
            &Expr::Function {
                name: "DATE_DIFF".into(),
                args: vec![
                    Expr::Literal(ScalarValue::Utf8("HOUR".into())),
                    Expr::Column("start".into()),
                    Expr::Column("end".into()),
                ],
            },
            &input,
        )
        .unwrap();
        assert_eq!(
            values
                .as_primitive::<Int64Type>()
                .iter()
                .collect::<Vec<_>>(),
            vec![Some(24), Some(1), None]
        );

        let values = evaluate(
            &Expr::Function {
                name: "DATE_DIFF".into(),
                args: vec![
                    Expr::Literal(ScalarValue::Utf8("MONTH".into())),
                    Expr::Column("start".into()),
                    Expr::Column("end".into()),
                ],
            },
            &input,
        )
        .unwrap();
        assert_eq!(
            values
                .as_primitive::<Int64Type>()
                .iter()
                .collect::<Vec<_>>(),
            vec![Some(0), Some(0), None]
        );

        let calendar_schema = Arc::new(arrow::datatypes::Schema::new(vec![
            arrow::datatypes::Field::new("start", DataType::Date32, true),
            arrow::datatypes::Field::new("end", DataType::Date32, true),
        ]));
        let calendar = RecordBatch::try_new(
            calendar_schema,
            vec![
                Arc::new(Date32Array::from(vec![
                    Some(ymd_to_days(2024, 1, 31) as i32),
                    Some(ymd_to_days(2024, 1, 31) as i32),
                ])),
                Arc::new(Date32Array::from(vec![
                    Some(ymd_to_days(2024, 2, 29) as i32),
                    Some(ymd_to_days(2024, 3, 31) as i32),
                ])),
            ],
        )
        .unwrap();
        let values = evaluate(
            &Expr::Function {
                name: "DATE_DIFF".into(),
                args: vec![
                    Expr::Literal(ScalarValue::Utf8("MONTH".into())),
                    Expr::Column("start".into()),
                    Expr::Column("end".into()),
                ],
            },
            &calendar,
        )
        .unwrap();
        assert_eq!(
            values
                .as_primitive::<Int64Type>()
                .iter()
                .collect::<Vec<_>>(),
            vec![Some(0), Some(2)]
        );

        let error = evaluate(
            &Expr::Function {
                name: "DATE_DIFF".into(),
                args: vec![
                    Expr::Literal(ScalarValue::Utf8("FORTNIGHT".into())),
                    Expr::Column("start".into()),
                    Expr::Column("end".into()),
                ],
            },
            &input,
        )
        .unwrap_err();
        assert!(error.to_string().contains("unsupported DATE_DIFF unit"), "{error}");
    }

    #[test]
    fn date_add_months_clamps_calendar_end_and_preserves_units() {
        let schema = Arc::new(arrow::datatypes::Schema::new(vec![
            arrow::datatypes::Field::new("day", DataType::Date32, true),
        ]));
        let input = RecordBatch::try_new(
            schema,
            vec![Arc::new(Date32Array::from(vec![Some(19_753), None, Some(19_782)]))],
        )
        .unwrap();
        let values = evaluate(
            &Expr::Function {
                name: "DATE_ADD_MONTHS".into(),
                args: vec![
                    Expr::Column("day".into()),
                    Expr::Literal(ScalarValue::Int64(1)),
                ],
            },
            &input,
        )
        .unwrap();
        let result = values.as_primitive::<Date32Type>();
        assert_eq!(result.value(0), 19_782); // 2024-01-31 -> 2024-02-29
        assert!(result.is_null(1));
        assert_eq!(result.value(2), 19_811); // 2024-02-29 -> 2024-03-29

        let timestamp_schema = Arc::new(arrow::datatypes::Schema::new(vec![
            arrow::datatypes::Field::new(
                "ts",
                DataType::Timestamp(TimeUnit::Millisecond, None),
                true,
            ),
        ]));
        let time_of_day = 12 * 3_600_000 + 34 * 60_000 + 56_789;
        let timestamp = RecordBatch::try_new(
            timestamp_schema,
            vec![Arc::new(TimestampMillisecondArray::from(vec![
                Some(19_753_i64 * 86_400_000 + time_of_day),
                None,
            ]))],
        )
        .unwrap();
        let values = evaluate(
            &Expr::Function {
                name: "DATE_ADD_MONTHS".into(),
                args: vec![
                    Expr::Column("ts".into()),
                    Expr::Literal(ScalarValue::Int64(1)),
                ],
            },
            &timestamp,
        )
        .unwrap();
        let result = values.as_primitive::<TimestampMillisecondType>();
        assert_eq!(result.value(0), 19_782_i64 * 86_400_000 + time_of_day);
        assert!(result.is_null(1));

        let values = evaluate(
            &Expr::Function {
                name: "DATE_ADD_SECONDS".into(),
                args: vec![
                    Expr::Column("ts".into()),
                    Expr::Literal(ScalarValue::Int64(7_200)),
                ],
            },
            &timestamp,
        )
        .unwrap();
        let result = values.as_primitive::<TimestampMillisecondType>();
        assert_eq!(result.value(0), 19_753_i64 * 86_400_000 + time_of_day + 7_200_000);
        assert!(result.is_null(1));
    }

    #[test]
    fn row_constructor_returns_a_struct_with_typed_fields() {
        let schema = Arc::new(arrow::datatypes::Schema::new(vec![
            arrow::datatypes::Field::new("id", DataType::Int64, true),
            arrow::datatypes::Field::new("name", DataType::Utf8, true),
        ]));
        let input = RecordBatch::try_new(
            schema,
            vec![
                Arc::new(Int64Array::from(vec![Some(1), None])),
                Arc::new(StringArray::from(vec![Some("a"), Some("b")])),
            ],
        )
        .unwrap();
        let value = evaluate(
            &Expr::Function {
                name: "ROW".into(),
                args: vec![Expr::Column("id".into()), Expr::Column("name".into())],
            },
            &input,
        )
        .unwrap();
        let row = value.as_any().downcast_ref::<StructArray>().unwrap();
        assert_eq!(row.len(), 2);
        assert_eq!(row.num_columns(), 2);
        assert_eq!(row.column(0).data_type(), &DataType::Int64);
        assert_eq!(row.column(1).data_type(), &DataType::Utf8);
        assert!(row.column(0).is_null(1));
    }

    #[test]
    fn row_field_returns_the_named_struct_child() {
        let schema = Arc::new(arrow::datatypes::Schema::new(vec![
            arrow::datatypes::Field::new("id", DataType::Int64, true),
        ]));
        let input = RecordBatch::try_new(
            schema,
            vec![Arc::new(Int64Array::from(vec![Some(7), Some(9)]))],
        )
        .unwrap();
        let value = evaluate(
            &Expr::Function {
                name: "ROW_FIELD".into(),
                args: vec![
                    Expr::Function {
                        name: "ROW".into(),
                        args: vec![Expr::Column("id".into())],
                    },
                    Expr::Literal(ScalarValue::Utf8("field0".into())),
                ],
            },
            &input,
        )
        .unwrap();
        assert_eq!(
            value.as_primitive::<Int64Type>().iter().collect::<Vec<_>>(),
            vec![Some(7), Some(9)]
        );
    }

    #[test]
    fn row_equality_compares_struct_fields_with_null_semantics() {
        let batch = RecordBatch::try_from_iter([(
            "id",
            Arc::new(Int64Array::from(vec![1, 2])) as ArrayRef,
        )])
        .unwrap();
        let row = |value| Expr::Function {
            name: "ROW".into(),
            args: vec![
                Expr::Literal(ScalarValue::Int64(value)),
                Expr::Column("id".into()),
            ],
        };
        let value = evaluate(
            &Expr::BinaryOp {
                left: Box::new(row(7)),
                op: BinaryOp::Eq,
                right: Box::new(row(7)),
            },
            &batch,
        )
        .unwrap();
        assert_eq!(
            value.as_boolean().iter().collect::<Vec<_>>(),
            vec![Some(true), Some(true)]
        );
    }

    #[test]
    fn row_ordering_is_lexicographic_with_null_propagation() {
        let batch = RecordBatch::try_from_iter([(
            "id",
            Arc::new(Int64Array::from(vec![1, 2, 3])) as ArrayRef,
        )])
        .unwrap();
        let row = |first: i64, second: Option<i64>| Expr::Function {
            name: "ROW".into(),
            args: vec![
                Expr::Literal(ScalarValue::Int64(first)),
                second
                    .map(ScalarValue::Int64)
                    .map(Expr::Literal)
                    .unwrap_or(Expr::Literal(ScalarValue::Null)),
            ],
        };
        let value = evaluate(
            &Expr::BinaryOp {
                left: Box::new(row(1, Some(5))),
                op: BinaryOp::Lt,
                right: Box::new(row(2, Some(0))),
            },
            &batch,
        )
        .unwrap();
        assert_eq!(value.as_boolean().iter().collect::<Vec<_>>(), vec![Some(true); 3]);

        let value = evaluate(
            &Expr::BinaryOp {
                left: Box::new(row(1, None)),
                op: BinaryOp::Lt,
                right: Box::new(row(1, Some(2))),
            },
            &batch,
        )
        .unwrap();
        assert_eq!(value.as_boolean().iter().collect::<Vec<_>>(), vec![None; 3]);
    }

    #[test]
    fn element_at_looks_up_map_keys_and_returns_null_when_absent() {
        let batch = RecordBatch::try_from_iter([(
            "id",
            Arc::new(Int64Array::from(vec![1, 2])) as ArrayRef,
        )])
        .unwrap();
        let map = Expr::Function {
            name: "MAP".into(),
            args: vec![
                Expr::Function {
                    name: "ARRAY".into(),
                    args: vec![
                        Expr::Literal(ScalarValue::Utf8("a".into())),
                        Expr::Literal(ScalarValue::Utf8("b".into())),
                    ],
                },
                Expr::Function {
                    name: "ARRAY".into(),
                    args: vec![
                        Expr::Literal(ScalarValue::Int64(1)),
                        Expr::Literal(ScalarValue::Int64(2)),
                    ],
                },
            ],
        };
        let value = evaluate(
            &Expr::Function {
                name: "ELEMENT_AT".into(),
                args: vec![map, Expr::Literal(ScalarValue::Utf8("b".into()))],
            },
            &batch,
        )
        .unwrap();
        assert_eq!(
            value
                .as_primitive::<Int64Type>()
                .iter()
                .collect::<Vec<_>>(),
            vec![Some(2), Some(2)]
        );
    }

    #[test]
    fn array_repeat_preserves_types_nulls_and_row_counts() {
        let batch = RecordBatch::try_from_iter([(
            "id",
            Arc::new(Int64Array::from(vec![Some(1), None, Some(3)])) as ArrayRef,
        )])
        .unwrap();
        let value = evaluate(
            &Expr::Function {
                name: "ARRAY_REPEAT".into(),
                args: vec![
                    Expr::Column("id".into()),
                    Expr::Literal(ScalarValue::Int64(2)),
                ],
            },
            &batch,
        )
        .unwrap();
        let arrays = value.as_any().downcast_ref::<ListArray>().unwrap();
        assert_eq!(arrays.value_type(), DataType::Int64);
        assert_eq!(arrays.value(0).as_primitive::<Int64Type>().values(), &[1, 1]);
        assert_eq!(arrays.value(1).null_count(), 2);
        assert_eq!(arrays.value(2).as_primitive::<Int64Type>().values(), &[3, 3]);

        let error = evaluate(
            &Expr::Function {
                name: "ARRAY_REPEAT".into(),
                args: vec![
                    Expr::Literal(ScalarValue::Int64(1)),
                    Expr::Literal(ScalarValue::Int64(-1)),
                ],
            },
            &batch,
        )
        .unwrap_err();
        assert!(error.to_string().contains("between 0 and 1000000"));
    }

    #[test]
    fn array_set_operations_deduplicate_and_match_nulls() {
        let batch = RecordBatch::try_from_iter([(
            "id",
            Arc::new(Int64Array::from(vec![1])) as ArrayRef,
        )])
        .unwrap();
        let left = Expr::Function {
            name: "ARRAY".into(),
            args: vec![
                Expr::Literal(ScalarValue::Int64(1)),
                Expr::Literal(ScalarValue::Int64(1)),
                Expr::Literal(ScalarValue::Null),
                Expr::Literal(ScalarValue::Int64(2)),
            ],
        };
        let right = Expr::Function {
            name: "ARRAY".into(),
            args: vec![
                Expr::Literal(ScalarValue::Int64(2)),
                Expr::Literal(ScalarValue::Null),
                Expr::Literal(ScalarValue::Int64(3)),
            ],
        };
        for (name, expected) in [
            ("ARRAY_UNION", vec![Some(1), None, Some(2), Some(3)]),
            ("ARRAY_INTERSECT", vec![None, Some(2)]),
            ("ARRAY_EXCEPT", vec![Some(1)]),
        ] {
            let value = evaluate(
                &Expr::Function {
                    name: name.into(),
                    args: vec![left.clone(), right.clone()],
                },
                &batch,
            )
            .unwrap();
            let arrays = value.as_any().downcast_ref::<ListArray>().unwrap();
            assert_eq!(arrays.value(0).as_primitive::<Int64Type>().iter().collect::<Vec<_>>(), expected);
        }
    }

    #[test]
    fn array_slice_preserves_bounds_stride_and_null_elements() {
        let batch = RecordBatch::try_from_iter([(
            "id",
            Arc::new(Int64Array::from(vec![1])) as ArrayRef,
        )])
        .unwrap();
        let value = evaluate(
            &Expr::Function {
                name: "ARRAY_SLICE".into(),
                args: vec![
                    Expr::Function {
                        name: "ARRAY".into(),
                        args: vec![
                            Expr::Literal(ScalarValue::Int64(10)),
                            Expr::Literal(ScalarValue::Null),
                            Expr::Literal(ScalarValue::Int64(30)),
                            Expr::Literal(ScalarValue::Int64(40)),
                        ],
                    },
                    Expr::Literal(ScalarValue::Int64(2)),
                    Expr::Literal(ScalarValue::Int64(4)),
                    Expr::Literal(ScalarValue::Int64(2)),
                ],
            },
            &batch,
        )
        .unwrap();
        let list = value.as_any().downcast_ref::<ListArray>().unwrap();
        let values = list.value(0);
        let values = values.as_primitive::<Int64Type>();
        assert_eq!(values.iter().collect::<Vec<_>>(), vec![None, Some(40)]);

        let value = evaluate(
            &Expr::Function {
                name: "ARRAY_SLICE".into(),
                args: vec![
                    Expr::Function {
                        name: "ARRAY".into(),
                        args: vec![
                            Expr::Literal(ScalarValue::Int64(10)),
                            Expr::Literal(ScalarValue::Int64(20)),
                            Expr::Literal(ScalarValue::Int64(30)),
                            Expr::Literal(ScalarValue::Int64(40)),
                        ],
                    },
                    Expr::Literal(ScalarValue::Null),
                    Expr::Literal(ScalarValue::Null),
                    Expr::Literal(ScalarValue::Int64(-2)),
                ],
            },
            &batch,
        )
        .unwrap();
        let list = value.as_any().downcast_ref::<ListArray>().unwrap();
        let values = list.value(0).as_primitive::<Int64Type>().clone();
        assert_eq!(values.iter().collect::<Vec<_>>(), vec![Some(40), Some(20)]);
    }

    #[test]
    fn array_distinct_keeps_first_occurrence_and_one_null() {
        let batch = RecordBatch::try_from_iter([(
            "id",
            Arc::new(Int64Array::from(vec![1])) as ArrayRef,
        )])
        .unwrap();
        let value = evaluate(
            &Expr::Function {
                name: "ARRAY_DISTINCT".into(),
                args: vec![Expr::Function {
                    name: "ARRAY".into(),
                    args: vec![
                        Expr::Literal(ScalarValue::Int64(2)),
                        Expr::Literal(ScalarValue::Null),
                        Expr::Literal(ScalarValue::Int64(1)),
                        Expr::Literal(ScalarValue::Int64(2)),
                        Expr::Literal(ScalarValue::Null),
                        Expr::Literal(ScalarValue::Int64(1)),
                    ],
                }],
            },
            &batch,
        )
        .unwrap();
        let list = value.as_any().downcast_ref::<ListArray>().unwrap();
        let inner = list.value(0);
        let values = inner.as_primitive::<Int64Type>();
        assert_eq!(values.iter().collect::<Vec<_>>(), vec![Some(2), None, Some(1)]);

        let value = evaluate(
            &Expr::Function {
                name: "ARRAY_DISTINCT".into(),
                args: vec![Expr::Function {
                    name: "ARRAY".into(),
                    args: vec![
                        Expr::Literal(ScalarValue::Utf8("b".into())),
                        Expr::Literal(ScalarValue::Utf8("a".into())),
                        Expr::Literal(ScalarValue::Utf8("b".into())),
                    ],
                }],
            },
            &batch,
        )
        .unwrap();
        let list = value.as_any().downcast_ref::<ListArray>().unwrap();
        let inner = list.value(0);
        let values = inner.as_string::<i32>();
        assert_eq!(values.iter().collect::<Vec<_>>(), vec![Some("b"), Some("a")]);
    }

    #[test]
    fn array_distinct_preserves_temporal_element_type() {
        let batch = RecordBatch::try_from_iter([
            (
                "first_day",
                Arc::new(arrow::array::Date32Array::from(vec![Some(10)])) as ArrayRef,
            ),
            (
                "second_day",
                Arc::new(arrow::array::Date32Array::from(vec![Some(20)])) as ArrayRef,
            ),
        ])
        .unwrap();
        let value = evaluate(
            &Expr::Function {
                name: "ARRAY_DISTINCT".into(),
                args: vec![Expr::Function {
                    name: "ARRAY".into(),
                    args: vec![
                        Expr::Column("first_day".into()),
                        Expr::Column("second_day".into()),
                        Expr::Column("first_day".into()),
                    ],
                }],
            },
            &batch,
        )
        .unwrap();
        let list = value.as_any().downcast_ref::<ListArray>().unwrap();
        assert_eq!(list.value_type(), DataType::Date32);
        let inner = list.value(0);
        let values = inner.as_primitive::<Date32Type>();
        assert_eq!(values.iter().collect::<Vec<_>>(), vec![Some(10), Some(20)]);
    }

    #[test]
    fn array_constructor_preserves_non_microsecond_timestamp_units() {
        let batch = RecordBatch::try_from_iter([
            (
                "first_second",
                Arc::new(TimestampSecondArray::from(vec![Some(10)])) as ArrayRef,
            ),
            (
                "second_second",
                Arc::new(TimestampSecondArray::from(vec![Some(20)])) as ArrayRef,
            ),
            (
                "first_millis",
                Arc::new(TimestampMillisecondArray::from(vec![Some(1000)])) as ArrayRef,
            ),
            (
                "second_millis",
                Arc::new(TimestampMillisecondArray::from(vec![Some(2000)])) as ArrayRef,
            ),
            (
                "first_nanos",
                Arc::new(TimestampNanosecondArray::from(vec![Some(1_000)])) as ArrayRef,
            ),
            (
                "second_nanos",
                Arc::new(TimestampNanosecondArray::from(vec![Some(2_000)])) as ArrayRef,
            ),
        ])
        .unwrap();

        for (left, right, expected) in [
            ("first_second", "second_second", DataType::Timestamp(TimeUnit::Second, None)),
            (
                "first_millis",
                "second_millis",
                DataType::Timestamp(TimeUnit::Millisecond, None),
            ),
            (
                "first_nanos",
                "second_nanos",
                DataType::Timestamp(TimeUnit::Nanosecond, None),
            ),
        ] {
            let value = evaluate(
                &Expr::Function {
                    name: "ARRAY".into(),
                    args: vec![
                        Expr::Column(left.into()),
                        Expr::Column(right.into()),
                    ],
                },
                &batch,
            )
            .unwrap();
            let list = value.as_any().downcast_ref::<ListArray>().unwrap();
            assert_eq!(list.value_type(), expected);
        }
    }

    #[test]
    fn array_distinct_preserves_int32_element_type() {
        let batch = RecordBatch::try_from_iter([(
            "value",
            Arc::new(arrow::array::Int32Array::from(vec![Some(2)])) as ArrayRef,
        )])
        .unwrap();
        let value = evaluate(
            &Expr::Function {
                name: "ARRAY_DISTINCT".into(),
                args: vec![Expr::Function {
                    name: "ARRAY".into(),
                    args: vec![
                        Expr::Column("value".into()),
                        Expr::Column("value".into()),
                        Expr::Column("value".into()),
                    ],
                }],
            },
            &batch,
        )
        .unwrap();
        let list = value.as_any().downcast_ref::<ListArray>().unwrap();
        assert_eq!(list.value_type(), DataType::Int32);
        let inner = list.value(0);
        let values = inner.as_primitive::<Int32Type>();
        assert_eq!(values.iter().collect::<Vec<_>>(), vec![Some(2)]);
    }

    #[test]
    fn array_sort_orders_values_and_places_nulls_last() {
        let batch = RecordBatch::try_from_iter([(
            "id",
            Arc::new(Int64Array::from(vec![1])) as ArrayRef,
        )])
        .unwrap();
        let value = evaluate(
            &Expr::Function {
                name: "ARRAY_SORT".into(),
                args: vec![Expr::Function {
                    name: "ARRAY".into(),
                    args: vec![
                        Expr::Literal(ScalarValue::Int64(3)),
                        Expr::Literal(ScalarValue::Null),
                        Expr::Literal(ScalarValue::Int64(1)),
                        Expr::Literal(ScalarValue::Int64(2)),
                    ],
                }],
            },
            &batch,
        )
        .unwrap();
        let list = value.as_any().downcast_ref::<ListArray>().unwrap();
        let inner = list.value(0);
        let values = inner.as_primitive::<Int64Type>();
        assert_eq!(values.iter().collect::<Vec<_>>(), vec![Some(1), Some(2), Some(3), None]);
    }

    #[test]
    fn array_reverse_preserves_null_elements_and_reverses_order() {
        let batch = RecordBatch::try_from_iter([(
            "id",
            Arc::new(Int64Array::from(vec![1])) as ArrayRef,
        )])
        .unwrap();
        let value = evaluate(
            &Expr::Function {
                name: "ARRAY_REVERSE".into(),
                args: vec![Expr::Function {
                    name: "ARRAY".into(),
                    args: vec![
                        Expr::Literal(ScalarValue::Utf8("a".into())),
                        Expr::Literal(ScalarValue::Null),
                        Expr::Literal(ScalarValue::Utf8("b".into())),
                    ],
                }],
            },
            &batch,
        )
        .unwrap();
        let list = value.as_any().downcast_ref::<ListArray>().unwrap();
        let inner = list.value(0);
        let values = inner.as_string::<i32>();
        assert_eq!(values.iter().collect::<Vec<_>>(), vec![Some("b"), None, Some("a")]);
    }

    #[test]
    fn boolean_array_literal_preserves_boolean_elements() {
        let batch = RecordBatch::try_from_iter([(
            "id",
            Arc::new(Int64Array::from(vec![1, 2])) as ArrayRef,
        )])
        .unwrap();
        let value = evaluate(
            &Expr::Function {
                name: "ARRAY".into(),
                args: vec![
                    Expr::Literal(ScalarValue::Bool(true)),
                    Expr::Literal(ScalarValue::Null),
                    Expr::Literal(ScalarValue::Bool(false)),
                ],
            },
            &batch,
        )
        .unwrap();
        let list = value.as_any().downcast_ref::<ListArray>().unwrap();
        assert_eq!(list.value_type(), DataType::Boolean);
        assert_eq!(list.value(0).as_boolean().iter().collect::<Vec<_>>(), vec![Some(true), None, Some(false)]);

        let contains = evaluate(
            &Expr::Function {
                name: "CONTAINS".into(),
                args: vec![
                    Expr::Function {
                        name: "ARRAY".into(),
                        args: vec![Expr::Literal(ScalarValue::Bool(true))],
                    },
                    Expr::Literal(ScalarValue::Bool(true)),
                ],
            },
            &batch,
        )
        .unwrap();
        assert_eq!(contains.as_boolean().iter().collect::<Vec<_>>(), vec![Some(true), Some(true)]);

        let position = evaluate(
            &Expr::Function {
                name: "ARRAY_POSITION".into(),
                args: vec![
                    Expr::Function {
                        name: "ARRAY".into(),
                        args: vec![
                            Expr::Literal(ScalarValue::Bool(false)),
                            Expr::Literal(ScalarValue::Bool(true)),
                        ],
                    },
                    Expr::Literal(ScalarValue::Bool(true)),
                ],
            },
            &batch,
        )
        .unwrap();
        assert_eq!(position.as_primitive::<Int64Type>().values(), &[2, 2]);
    }

    #[test]
    fn cardinality_counts_map_entries() {
        let batch = RecordBatch::try_from_iter([(
            "id",
            Arc::new(Int64Array::from(vec![1, 2])) as ArrayRef,
        )])
        .unwrap();
        let value = evaluate(
            &Expr::Function {
                name: "CARDINALITY".into(),
                args: vec![Expr::Function {
                    name: "MAP".into(),
                    args: vec![
                        Expr::Function {
                            name: "ARRAY".into(),
                            args: vec![Expr::Literal(ScalarValue::Utf8("a".into()))],
                        },
                        Expr::Function {
                            name: "ARRAY".into(),
                            args: vec![Expr::Literal(ScalarValue::Int64(1))],
                        },
                    ],
                }],
            },
            &batch,
        )
        .unwrap();
        assert_eq!(value.as_primitive::<Int64Type>().values(), &[1, 1]);
    }

    #[test]
    fn map_equality_is_order_independent_and_null_safe() {
        let batch = RecordBatch::try_from_iter([(
            "id",
            Arc::new(Int64Array::from(vec![1, 2])) as ArrayRef,
        )])
        .unwrap();
        let map = |keys: &[&str], values: &[Option<i64>]| Expr::Function {
            name: "MAP".into(),
            args: vec![
                Expr::Function {
                    name: "ARRAY".into(),
                    args: keys
                        .iter()
                        .map(|key| Expr::Literal(ScalarValue::Utf8((*key).into())))
                        .collect(),
                },
                Expr::Function {
                    name: "ARRAY".into(),
                    args: values
                        .iter()
                        .map(|value| match value {
                            Some(value) => Expr::Literal(ScalarValue::Int64(*value)),
                            None => Expr::Literal(ScalarValue::Null),
                        })
                        .collect(),
                },
            ],
        };
        let value = evaluate(
            &Expr::BinaryOp {
                left: Box::new(map(&["a", "b"], &[Some(1), Some(2)])),
                op: BinaryOp::Eq,
                right: Box::new(map(&["b", "a"], &[Some(2), Some(1)])),
            },
            &batch,
        )
        .unwrap();
        assert_eq!(
            value.as_boolean().iter().collect::<Vec<_>>(),
            vec![Some(true), Some(true)]
        );

        let value = evaluate(
            &Expr::BinaryOp {
                left: Box::new(map(&["a"], &[None])),
                op: BinaryOp::Eq,
                right: Box::new(map(&["a"], &[None])),
            },
            &batch,
        )
        .unwrap();
        assert_eq!(
            value.as_boolean().iter().collect::<Vec<_>>(),
            vec![None, None]
        );
    }

    #[test]
    fn map_constructor_preserves_offsets_and_rejects_null_keys() {
        let input = RecordBatch::try_from_iter([(
            "dummy",
            Arc::new(Int64Array::from(vec![1])) as ArrayRef,
        )])
        .unwrap();
        let value = evaluate(
            &Expr::Function {
                name: "MAP".into(),
                args: vec![
                    Expr::Function {
                        name: "ARRAY".into(),
                        args: vec![Expr::Literal(ScalarValue::Utf8("a".into()))],
                    },
                    Expr::Function {
                        name: "ARRAY".into(),
                        args: vec![Expr::Literal(ScalarValue::Int64(1))],
                    },
                ],
            },
            &input,
        )
        .unwrap();
        let map = value.as_any().downcast_ref::<MapArray>().unwrap();
        assert_eq!(map.len(), 1);
        assert_eq!(map.entries().num_columns(), 2);

        let error = evaluate(
            &Expr::Function {
                name: "MAP".into(),
                args: vec![
                    Expr::Function {
                        name: "ARRAY".into(),
                        args: vec![Expr::Literal(ScalarValue::Null)],
                    },
                    Expr::Function {
                        name: "ARRAY".into(),
                        args: vec![Expr::Literal(ScalarValue::Int64(1))],
                    },
                ],
            },
            &input,
        )
        .unwrap_err();
        assert!(error.to_string().contains("MAP keys cannot contain NULL"));
    }

    #[test]
    fn split_to_map_builds_typed_maps_and_rejects_duplicate_keys() {
        let batch = RecordBatch::try_from_iter([(
            "dummy",
            Arc::new(StringArray::from(vec!["x", "y"])) as ArrayRef,
        )])
        .unwrap();
        let expression = Expr::Function {
            name: "SPLIT_TO_MAP".into(),
            args: vec![
                Expr::Literal(ScalarValue::Utf8("a:1,b:2".into())),
                Expr::Literal(ScalarValue::Utf8(",".into())),
                Expr::Literal(ScalarValue::Utf8(":".into())),
            ],
        };
        let result = evaluate(&expression, &batch).unwrap();
        let map = result.as_any().downcast_ref::<MapArray>().unwrap();
        assert_eq!(map.keys().as_string::<i32>().iter().collect::<Vec<_>>(), vec![Some("a"), Some("b"), Some("a"), Some("b")]);
        assert_eq!(map.values().as_string::<i32>().iter().collect::<Vec<_>>(), vec![Some("1"), Some("2"), Some("1"), Some("2")]);

        let duplicate = Expr::Function {
            name: "SPLIT_TO_MAP".into(),
            args: vec![
                Expr::Literal(ScalarValue::Utf8("a:1,a:2".into())),
                Expr::Literal(ScalarValue::Utf8(",".into())),
                Expr::Literal(ScalarValue::Utf8(":".into())),
            ],
        };
        assert!(evaluate(&duplicate, &batch)
            .unwrap_err()
            .to_string()
            .contains("duplicate key"));
    }

    #[test]
    fn split_to_multimap_groups_duplicate_keys_in_order() {
        let batch = RecordBatch::try_from_iter([(
            "dummy",
            Arc::new(StringArray::from(vec!["x", "y"])) as ArrayRef,
        )])
        .unwrap();
        let expression = Expr::Function {
            name: "SPLIT_TO_MULTIMAP".into(),
            args: vec![
                Expr::Literal(ScalarValue::Utf8("a:1,b:2,a:3".into())),
                Expr::Literal(ScalarValue::Utf8(",".into())),
                Expr::Literal(ScalarValue::Utf8(":".into())),
            ],
        };
        let result = evaluate(&expression, &batch).unwrap();
        let map = result.as_any().downcast_ref::<MapArray>().unwrap();
        assert_eq!(map.keys().as_string::<i32>().iter().collect::<Vec<_>>(), vec![Some("a"), Some("b"), Some("a"), Some("b")]);
        let values = map.values().as_any().downcast_ref::<ListArray>().unwrap();
        assert_eq!(values.value(0).as_string::<i32>().iter().collect::<Vec<_>>(), vec![Some("1"), Some("3")]);
        assert_eq!(values.value(1).as_string::<i32>().iter().collect::<Vec<_>>(), vec![Some("2")]);
    }

    #[test]
    fn map_concat_uses_later_values_and_preserves_rows() {
        let input = RecordBatch::try_from_iter([(
            "dummy",
            Arc::new(Int64Array::from(vec![1])) as ArrayRef,
        )])
        .unwrap();
        let map = |keys: &[&str], values: &[i64]| Expr::Function {
            name: "MAP".into(),
            args: vec![
                Expr::Function {
                    name: "ARRAY".into(),
                    args: keys
                        .iter()
                        .map(|key| Expr::Literal(ScalarValue::Utf8((*key).into())))
                        .collect(),
                },
                Expr::Function {
                    name: "ARRAY".into(),
                    args: values
                        .iter()
                        .map(|value| Expr::Literal(ScalarValue::Int64(*value)))
                        .collect(),
                },
            ],
        };
        let result = evaluate(
            &Expr::Function {
                name: "MAP_CONCAT".into(),
                args: vec![map(&["a", "b"], &[1, 2]), map(&["b", "c"], &[20, 3])],
            },
            &input,
        )
        .unwrap();
        let result = result.as_any().downcast_ref::<MapArray>().unwrap();
        assert_eq!(result.keys().as_string::<i32>().iter().collect::<Vec<_>>(),
            vec![Some("a"), Some("b"), Some("c")]);
        assert_eq!(result.values().as_primitive::<Int64Type>().iter().collect::<Vec<_>>(),
            vec![Some(1), Some(20), Some(3)]);
    }

    #[test]
    fn map_keys_and_values_preserve_map_offsets() {
        let mut key_builder = arrow::array::ListBuilder::new(arrow::array::StringBuilder::new());
        key_builder.values().append_value("a");
        key_builder.append(true);
        key_builder.append(false);
        let mut value_builder = arrow::array::ListBuilder::new(arrow::array::Float64Builder::new());
        value_builder.values().append_value(1.0);
        value_builder.append(true);
        value_builder.append(false);
        let input = RecordBatch::try_from_iter([
            ("keys", Arc::new(key_builder.finish()) as ArrayRef),
            ("values", Arc::new(value_builder.finish()) as ArrayRef),
        ])
        .unwrap();
        let map = Expr::Function {
            name: "MAP".into(),
            args: vec![Expr::Column("keys".into()), Expr::Column("values".into())],
        };
        let keys = evaluate(
            &Expr::Function {
                name: "MAP_KEYS".into(),
                args: vec![map.clone()],
            },
            &input,
        )
        .unwrap();
        let keys = keys.as_list::<i32>();
        assert_eq!(keys.len(), 2);
        assert_eq!(keys.value(0).as_string::<i32>().value(0), "a");
        assert!(keys.is_null(1));

        let values = evaluate(
            &Expr::Function {
                name: "MAP_VALUES".into(),
                args: vec![map],
            },
            &input,
        )
        .unwrap();
        let values = values.as_list::<i32>();
        assert_eq!(values.value(0).as_primitive::<Float64Type>().value(0), 1.0);
        assert!(values.is_null(1));
    }

    #[test]
    fn map_entries_preserves_typed_key_value_structs() {
        let input = RecordBatch::try_from_iter([(
            "dummy",
            Arc::new(Int64Array::from(vec![1])) as ArrayRef,
        )])
        .unwrap();
        let result = evaluate(
            &Expr::Function {
                name: "MAP_ENTRIES".into(),
                args: vec![Expr::Function {
                    name: "MAP".into(),
                    args: vec![
                        Expr::Function {
                            name: "ARRAY".into(),
                            args: vec![Expr::Literal(ScalarValue::Utf8("a".into()))],
                        },
                        Expr::Function {
                            name: "ARRAY".into(),
                            args: vec![Expr::Literal(ScalarValue::Int64(7))],
                        },
                    ],
                }],
            },
            &input,
        )
        .unwrap();
        let entries = result.as_any().downcast_ref::<ListArray>().unwrap();
        let entry_value = entries.value(0);
        let entry = entry_value.as_any().downcast_ref::<StructArray>().unwrap();
        assert_eq!(
            entry
                .column_by_name("key")
                .unwrap()
                .as_string::<i32>()
                .value(0),
            "a"
        );
        assert_eq!(
            entry
                .column_by_name("value")
                .unwrap()
                .as_primitive::<Int64Type>()
                .value(0),
            7
        );
    }

    #[test]
    fn ordered_map_comparison_fails_with_stable_diagnostic() {
        let batch = RecordBatch::try_from_iter([(
            "id",
            Arc::new(Int64Array::from(vec![1])) as ArrayRef,
        )])
        .unwrap();
        let map = || Expr::Function {
            name: "MAP".into(),
            args: vec![
                Expr::Function {
                    name: "ARRAY".into(),
                    args: vec![Expr::Literal(ScalarValue::Utf8("a".into()))],
                },
                Expr::Function {
                    name: "ARRAY".into(),
                    args: vec![Expr::Literal(ScalarValue::Int64(1))],
                },
            ],
        };
        let error = evaluate(
            &Expr::BinaryOp {
                left: Box::new(map()),
                op: BinaryOp::Lt,
                right: Box::new(map()),
            },
            &batch,
        )
        .unwrap_err();
        assert_eq!(
            error.to_string(),
            "execution: ordered comparison is not supported for MAP values"
        );
    }

    #[test]
    fn quantified_array_comparisons_follow_any_all_and_empty_null_rules() {
        let mut empty_values = ListBuilder::new(Int64Builder::new());
        empty_values.append(true);
        let empty_array = Arc::new(empty_values.finish()) as ArrayRef;
        let batch = RecordBatch::try_from_iter([
            (
                "id",
                Arc::new(Int64Array::from(vec![1])) as ArrayRef,
            ),
            ("empty_values", empty_array),
        ])
        .unwrap();
        let array = |values: Vec<Expr>| Expr::Function {
            name: "ARRAY".into(),
            args: values,
        };
        let evaluate_quantified = |name: &str, values: Vec<Expr>, needle| {
            evaluate(
                &Expr::Function {
                    name: name.into(),
                    args: vec![array(values), Expr::Literal(needle)],
                },
                &batch,
            )
            .unwrap()
        };

        let any = evaluate_quantified(
            "ARRAY_ANY_GT",
            vec![Expr::Literal(ScalarValue::Int64(1)), Expr::Literal(ScalarValue::Int64(3))],
            ScalarValue::Int64(2),
        );
        assert_eq!(any.as_boolean().iter().collect::<Vec<_>>(), vec![Some(true)]);

        let all = evaluate_quantified(
            "ARRAY_ALL_LT",
            vec![Expr::Literal(ScalarValue::Int64(1)), Expr::Literal(ScalarValue::Int64(2))],
            ScalarValue::Int64(3),
        );
        assert_eq!(all.as_boolean().iter().collect::<Vec<_>>(), vec![Some(true)]);

        let empty = evaluate(
            &Expr::Function {
                name: "ARRAY_ANY_GT".into(),
                args: vec![
                    Expr::Column("empty_values".into()),
                    Expr::Literal(ScalarValue::Int64(2)),
                ],
            },
            &batch,
        )
        .unwrap();
        assert_eq!(empty.as_boolean().iter().collect::<Vec<_>>(), vec![Some(false)]);
        let empty_all = evaluate(
            &Expr::Function {
                name: "ARRAY_ALL_GT".into(),
                args: vec![
                    Expr::Column("empty_values".into()),
                    Expr::Literal(ScalarValue::Int64(2)),
                ],
            },
            &batch,
        )
        .unwrap();
        assert_eq!(empty_all.as_boolean().iter().collect::<Vec<_>>(), vec![Some(true)]);

        let nullable = evaluate_quantified(
            "ARRAY_ANY_GT",
            vec![
                Expr::Literal(ScalarValue::Null),
                Expr::Literal(ScalarValue::Int64(1)),
            ],
            ScalarValue::Int64(2),
        );
        assert!(nullable.as_boolean().is_null(0));
    }

    #[test]
    fn nested_array_literals_materialize_as_lists_of_lists() {
        let batch = RecordBatch::try_from_iter([(
            "id",
            Arc::new(Int64Array::from(vec![1])) as ArrayRef,
        )])
        .unwrap();
        let array = |values: Vec<Expr>| Expr::Function {
            name: "ARRAY".into(),
            args: values,
        };
        let nested = evaluate(
            &array(vec![
                array(vec![
                    Expr::Literal(ScalarValue::Int64(1)),
                    Expr::Literal(ScalarValue::Int64(2)),
                ]),
                array(vec![Expr::Literal(ScalarValue::Int64(3))]),
            ]),
            &batch,
        )
        .unwrap();
        let outer = nested.as_any().downcast_ref::<ListArray>().unwrap();
        let inner_value = outer.value(0);
        let inner = inner_value.as_any().downcast_ref::<ListArray>().unwrap();
        assert_eq!(inner.len(), 2);
        assert_eq!(inner.value(0).as_primitive::<Int64Type>().values(), &[1, 2]);
        assert_eq!(inner.value(1).as_primitive::<Int64Type>().values(), &[3]);
    }

    #[test]
    fn arrays_of_rows_materialize_typed_struct_elements() {
        let batch = RecordBatch::try_from_iter([(
            "id",
            Arc::new(Int64Array::from(vec![1])) as ArrayRef,
        )])
        .unwrap();
        let row = |value: i64| Expr::Function {
            name: "ROW".into(),
            args: vec![Expr::Literal(ScalarValue::Int64(value))],
        };
        let result = evaluate(
            &Expr::Function {
                name: "ARRAY".into(),
                args: vec![row(7), row(9)],
            },
            &batch,
        )
        .unwrap();
        let outer = result.as_any().downcast_ref::<ListArray>().unwrap();
        let rows = outer.value(0);
        let rows = rows.as_any().downcast_ref::<StructArray>().unwrap();
        assert_eq!(rows.len(), 2);
        assert_eq!(rows.column(0).as_primitive::<Int64Type>().values(), &[7, 9]);
    }

    #[test]
    fn arrays_of_maps_materialize_typed_map_elements() {
        let batch = RecordBatch::try_from_iter([(
            "id",
            Arc::new(Int64Array::from(vec![1])) as ArrayRef,
        )])
        .unwrap();
        let map = |key: &str, value: i64| Expr::Function {
            name: "MAP".into(),
            args: vec![
                Expr::Function {
                    name: "ARRAY".into(),
                    args: vec![Expr::Literal(ScalarValue::Utf8(key.into()))],
                },
                Expr::Function {
                    name: "ARRAY".into(),
                    args: vec![Expr::Literal(ScalarValue::Int64(value))],
                },
            ],
        };
        let result = evaluate(
            &Expr::Function {
                name: "ARRAY".into(),
                args: vec![map("a", 1), map("b", 2)],
            },
            &batch,
        )
        .unwrap();
        let outer = result.as_any().downcast_ref::<ListArray>().unwrap();
        let maps = outer.value(0);
        let maps = maps.as_any().downcast_ref::<MapArray>().unwrap();
        assert_eq!(maps.len(), 2);
        let keys = maps.keys().as_string::<i32>();
        assert_eq!((0..keys.len()).map(|i| keys.value(i)).collect::<Vec<_>>(), ["a", "b"]);
        assert_eq!(maps.values().as_primitive::<Int64Type>().values(), &[1, 2]);
    }

    #[test]
    fn array_concat_preserves_nested_array_elements() {
        let batch = RecordBatch::try_from_iter([(
            "id",
            Arc::new(Int64Array::from(vec![1])) as ArrayRef,
        )])
        .unwrap();
        let array = |values: Vec<Expr>| Expr::Function {
            name: "ARRAY".into(),
            args: values,
        };
        let result = evaluate(
            &Expr::Function {
                name: "ARRAY_CONCAT".into(),
                args: vec![
                    array(vec![array(vec![Expr::Literal(ScalarValue::Int64(1))])]),
                    array(vec![array(vec![Expr::Literal(ScalarValue::Int64(2))])]),
                ],
            },
            &batch,
        )
        .unwrap();
        let outer = result.as_any().downcast_ref::<ListArray>().unwrap();
        let nested_value = outer.value(0);
        let nested = nested_value.as_any().downcast_ref::<ListArray>().unwrap();
        assert_eq!(nested.len(), 2);
        assert_eq!(nested.value(0).as_primitive::<Int64Type>().value(0), 1);
        assert_eq!(nested.value(1).as_primitive::<Int64Type>().value(0), 2);

        let row = |value: i64| Expr::Function {
            name: "ROW".into(),
            args: vec![Expr::Literal(ScalarValue::Int64(value))],
        };
        let rows = evaluate(
            &Expr::Function {
                name: "ARRAY_CONCAT".into(),
                args: vec![array(vec![row(3)]), array(vec![row(4)])],
            },
            &batch,
        )
        .unwrap();
        let rows = rows.as_any().downcast_ref::<ListArray>().unwrap();
        let rows_value = rows.value(0);
        let rows = rows_value.as_any().downcast_ref::<StructArray>().unwrap();
        assert_eq!(rows.len(), 2);
        assert_eq!(rows.column(0).as_primitive::<Int64Type>().values(), &[3, 4]);

        let map = |key: &str, value: i64| Expr::Function {
            name: "MAP".into(),
            args: vec![
                array(vec![Expr::Literal(ScalarValue::Utf8(key.into()))]),
                array(vec![Expr::Literal(ScalarValue::Int64(value))]),
            ],
        };
        let maps = evaluate(
            &Expr::Function {
                name: "ARRAY_CONCAT".into(),
                args: vec![array(vec![map("a", 5)]), array(vec![map("b", 6)])],
            },
            &batch,
        )
        .unwrap();
        let maps = maps.as_any().downcast_ref::<ListArray>().unwrap();
        let maps_value = maps.value(0);
        let maps = maps_value.as_any().downcast_ref::<MapArray>().unwrap();
        assert_eq!(maps.len(), 2);
        assert_eq!(maps.keys().as_string::<i32>().value(1), "b");

        let appended = evaluate(
            &Expr::Function {
                name: "ARRAY_APPEND".into(),
                args: vec![
                    array(vec![array(vec![Expr::Literal(ScalarValue::Int64(7))])]),
                    array(vec![Expr::Literal(ScalarValue::Int64(8))]),
                ],
            },
            &batch,
        )
        .unwrap();
        let appended = appended.as_any().downcast_ref::<ListArray>().unwrap();
        let appended_value = appended.value(0);
        let appended = appended_value.as_any().downcast_ref::<ListArray>().unwrap();
        assert_eq!(appended.len(), 2);
        assert_eq!(appended.value(1).as_primitive::<Int64Type>().value(0), 8);

        let removed = evaluate(
            &Expr::Function {
                name: "ARRAY_REMOVE".into(),
                args: vec![
                    array(vec![
                        array(vec![Expr::Literal(ScalarValue::Int64(9))]),
                        array(vec![Expr::Literal(ScalarValue::Int64(10))]),
                    ]),
                    array(vec![Expr::Literal(ScalarValue::Int64(9))]),
                ],
            },
            &batch,
        )
        .unwrap();
        let removed = removed.as_any().downcast_ref::<ListArray>().unwrap();
        let removed_value = removed.value(0);
        let removed = removed_value.as_any().downcast_ref::<ListArray>().unwrap();
        assert_eq!(removed.len(), 1);
        assert_eq!(removed.value(0).as_primitive::<Int64Type>().value(0), 10);
    }

    #[test]
    fn array_distinct_deduplicates_nested_array_elements_in_order() {
        let batch = RecordBatch::try_from_iter([(
            "id",
            Arc::new(Int64Array::from(vec![1])) as ArrayRef,
        )])
        .unwrap();
        let array = |values: Vec<Expr>| Expr::Function {
            name: "ARRAY".into(),
            args: values,
        };
        let input = array(vec![
            array(vec![Expr::Literal(ScalarValue::Int64(1))]),
            array(vec![Expr::Literal(ScalarValue::Int64(1))]),
            array(vec![Expr::Literal(ScalarValue::Int64(2))]),
        ]);
        let result = evaluate(
            &Expr::Function {
                name: "ARRAY_DISTINCT".into(),
                args: vec![input],
            },
            &batch,
        )
        .unwrap();
        let outer = result.as_any().downcast_ref::<ListArray>().unwrap();
        let values = outer.value(0);
        let values = values.as_any().downcast_ref::<ListArray>().unwrap();
        assert_eq!(values.len(), 2);
        assert_eq!(values.value(0).as_primitive::<Int64Type>().value(0), 1);
        assert_eq!(values.value(1).as_primitive::<Int64Type>().value(0), 2);
    }

    #[test]
    fn array_position_finds_nested_row_elements() {
        let batch = RecordBatch::try_from_iter([(
            "id",
            Arc::new(Int64Array::from(vec![1])) as ArrayRef,
        )])
        .unwrap();
        let row = |value: i64| Expr::Function {
            name: "ROW".into(),
            args: vec![Expr::Literal(ScalarValue::Int64(value))],
        };
        let result = evaluate(
            &Expr::Function {
                name: "ARRAY_POSITION".into(),
                args: vec![
                    Expr::Function {
                        name: "ARRAY".into(),
                        args: vec![row(4), row(9)],
                    },
                    row(9),
                ],
            },
            &batch,
        )
        .unwrap();
        assert_eq!(result.as_primitive::<Int64Type>().value(0), 2);
    }

    #[test]
    fn contains_matches_nested_row_elements_with_null_propagation() {
        let batch = RecordBatch::try_from_iter([(
            "id",
            Arc::new(Int64Array::from(vec![1])) as ArrayRef,
        )])
        .unwrap();
        let row = |value: i64| Expr::Function {
            name: "ROW".into(),
            args: vec![Expr::Literal(ScalarValue::Int64(value))],
        };
        let result = evaluate(
            &Expr::Function {
                name: "CONTAINS".into(),
                args: vec![
                    Expr::Function {
                        name: "ARRAY".into(),
                        args: vec![row(4), row(9)],
                    },
                    row(9),
                ],
            },
            &batch,
        )
        .unwrap();
        assert_eq!(result.as_boolean().value(0), true);
    }

    #[test]
    fn arrays_overlap_matches_nested_row_elements() {
        let batch = RecordBatch::try_from_iter([(
            "id",
            Arc::new(Int64Array::from(vec![1])) as ArrayRef,
        )])
        .unwrap();
        let row = |value: i64| Expr::Function {
            name: "ROW".into(),
            args: vec![Expr::Literal(ScalarValue::Int64(value))],
        };
        let array = |values: Vec<Expr>| Expr::Function {
            name: "ARRAY".into(),
            args: values,
        };
        let result = evaluate(
            &Expr::Function {
                name: "ARRAYS_OVERLAP".into(),
                args: vec![array(vec![row(1), row(2)]), array(vec![row(2), row(3)])],
            },
            &batch,
        )
        .unwrap();
        assert_eq!(result.as_boolean().value(0), true);
    }

    #[test]
    fn arrays_overlap_accepts_large_lists_of_nested_values() {
        use arrow::array::{Int64Builder, LargeListBuilder, ListBuilder};

        let mut left_builder =
            LargeListBuilder::new(ListBuilder::new(Int64Builder::new()));
        left_builder.values().values().append_value(1);
        left_builder.values().append(true);
        left_builder.values().values().append_value(2);
        left_builder.values().append(true);
        left_builder.append(true);
        let left = Arc::new(left_builder.finish()) as ArrayRef;

        let mut right_builder =
            LargeListBuilder::new(ListBuilder::new(Int64Builder::new()));
        right_builder.values().values().append_value(2);
        right_builder.values().append(true);
        right_builder.values().values().append_value(3);
        right_builder.values().append(true);
        right_builder.append(true);
        let right = Arc::new(right_builder.finish()) as ArrayRef;

        let result = eval_scalar_function("ARRAYS_OVERLAP", &[left, right], 1).unwrap();
        assert!(result.as_boolean().value(0));

        let mut primitive_left = LargeListBuilder::new(Int64Builder::new());
        primitive_left.values().append_value(7);
        primitive_left.append(true);
        let mut primitive_right = LargeListBuilder::new(Int64Builder::new());
        primitive_right.values().append_value(7);
        primitive_right.append(true);
        let result = eval_scalar_function(
            "ARRAYS_OVERLAP",
            &[
                Arc::new(primitive_left.finish()) as ArrayRef,
                Arc::new(primitive_right.finish()) as ArrayRef,
            ],
            1,
        )
        .unwrap();
        assert!(result.as_boolean().value(0));

        let mut needle_builder = ListBuilder::new(Int64Builder::new());
        needle_builder.values().append_value(2);
        needle_builder.append(true);
        let mut position_builder = LargeListBuilder::new(ListBuilder::new(Int64Builder::new()));
        position_builder.values().values().append_value(1);
        position_builder.values().append(true);
        position_builder.values().values().append_value(2);
        position_builder.values().append(true);
        position_builder.append(true);
        let result = eval_scalar_function(
            "ARRAY_POSITION",
            &[
                Arc::new(position_builder.finish()) as ArrayRef,
                Arc::new(needle_builder.finish()) as ArrayRef,
            ],
            1,
        )
        .unwrap();
        assert_eq!(result.as_primitive::<Int64Type>().value(0), 2);

        let mut contains_needle = ListBuilder::new(Int64Builder::new());
        contains_needle.values().append_value(2);
        contains_needle.append(true);
        let result = eval_scalar_function(
            "CONTAINS",
            &[
                Arc::new({
                    let mut builder = LargeListBuilder::new(ListBuilder::new(Int64Builder::new()));
                    builder.values().values().append_value(1);
                    builder.values().append(true);
                    builder.values().values().append_value(2);
                    builder.values().append(true);
                    builder.append(true);
                    builder.finish()
                }) as ArrayRef,
                Arc::new(contains_needle.finish()) as ArrayRef,
            ],
            1,
        )
        .unwrap();
        assert!(result.as_boolean().value(0));

        let mut distinct_builder = LargeListBuilder::new(ListBuilder::new(Int64Builder::new()));
        for value in [1_i64, 1, 2] {
            distinct_builder.values().values().append_value(value);
            distinct_builder.values().append(true);
        }
        distinct_builder.append(true);
        let result = eval_scalar_function(
            "ARRAY_DISTINCT",
            &[Arc::new(distinct_builder.finish()) as ArrayRef],
            1,
        )
        .unwrap();
        let result = result.as_any().downcast_ref::<LargeListArray>().unwrap();
        assert_eq!(result.value(0).len(), 2);

        let mut reverse_builder = LargeListBuilder::new(ListBuilder::new(Int64Builder::new()));
        for value in [1_i64, 2, 3] {
            reverse_builder.values().values().append_value(value);
            reverse_builder.values().append(true);
        }
        reverse_builder.append(true);
        let result = eval_scalar_function(
            "ARRAY_REVERSE",
            &[Arc::new(reverse_builder.finish()) as ArrayRef],
            1,
        )
        .unwrap();
        let result = result.as_any().downcast_ref::<LargeListArray>().unwrap();
        let reversed_values = result.value(0);
        let reversed_values = reversed_values.as_any().downcast_ref::<ListArray>().unwrap();
        assert_eq!(reversed_values.len(), 3);
        for (index, expected) in [3_i64, 2, 1].into_iter().enumerate() {
            let value = reversed_values.value(index);
            assert_eq!(value.as_primitive::<Int64Type>().value(0), expected);
        }

        let mut sort_builder = LargeListBuilder::new(Int64Builder::new());
        for value in [3_i64, 1, 2] {
            sort_builder.values().append_value(value);
        }
        sort_builder.append(true);
        let result = eval_scalar_function(
            "ARRAY_SORT",
            &[Arc::new(sort_builder.finish()) as ArrayRef],
            1,
        )
        .unwrap();
        let result = result.as_any().downcast_ref::<LargeListArray>().unwrap();
        let sorted_values = result.value(0);
        let sorted = sorted_values.as_primitive::<Int64Type>();
        assert_eq!(sorted.values(), &[1, 2, 3]);

        let mut slice_builder = LargeListBuilder::new(ListBuilder::new(Int64Builder::new()));
        for value in [1_i64, 2, 3] {
            slice_builder.values().values().append_value(value);
            slice_builder.values().append(true);
        }
        slice_builder.append(true);
        let result = eval_scalar_function(
            "ARRAY_SLICE",
            &[
                Arc::new(slice_builder.finish()) as ArrayRef,
                Arc::new(Int64Array::from(vec![1])) as ArrayRef,
                Arc::new(Int64Array::from(vec![3])) as ArrayRef,
                Arc::new(Int64Array::from(vec![2])) as ArrayRef,
            ],
            1,
        )
        .unwrap();
        let result = result.as_any().downcast_ref::<LargeListArray>().unwrap();
        let sliced = result.value(0);
        let sliced = sliced.as_any().downcast_ref::<ListArray>().unwrap();
        assert_eq!(sliced.len(), 2);
        assert_eq!(sliced.value(0).as_primitive::<Int64Type>().value(0), 1);
        assert_eq!(sliced.value(1).as_primitive::<Int64Type>().value(0), 3);

        let extrema = || {
            let mut builder = LargeListBuilder::new(Int64Builder::new());
            for value in [4_i64, 1, 9] {
                builder.values().append_value(value);
            }
            builder.append(true);
            Arc::new(builder.finish()) as ArrayRef
        };
        for (name, expected) in [("ARRAY_MIN", 1_i64), ("ARRAY_MAX", 9_i64)] {
            let result = eval_scalar_function(
                name,
                &[extrema()],
                1,
            )
            .unwrap();
            assert_eq!(result.as_primitive::<Int64Type>().value(0), expected);
        }
    }

    #[test]
    fn array_equality_is_elementwise_and_null_preserving() {
        let batch = RecordBatch::try_from_iter([(
            "id",
            Arc::new(Int64Array::from(vec![1])) as ArrayRef,
        )])
        .unwrap();
        let array = |values: Vec<Expr>| Expr::Function {
            name: "ARRAY".into(),
            args: values,
        };
        let equal = evaluate(
            &Expr::BinaryOp {
                left: Box::new(array(vec![
                    Expr::Literal(ScalarValue::Int64(1)),
                    Expr::Literal(ScalarValue::Int64(2)),
                ])),
                op: BinaryOp::Eq,
                right: Box::new(array(vec![
                    Expr::Literal(ScalarValue::Int64(1)),
                    Expr::Literal(ScalarValue::Int64(2)),
                ])),
            },
            &batch,
        )
        .unwrap();
        assert_eq!(equal.as_boolean().value(0), true);
        let different = evaluate(
            &Expr::BinaryOp {
                left: Box::new(array(vec![Expr::Literal(ScalarValue::Int64(1))])),
                op: BinaryOp::Ne,
                right: Box::new(array(vec![Expr::Literal(ScalarValue::Int64(2))])),
            },
            &batch,
        )
        .unwrap();
        assert_eq!(different.as_boolean().value(0), true);
        let nullable = evaluate(
            &Expr::BinaryOp {
                left: Box::new(array(vec![
                    Expr::Literal(ScalarValue::Int64(1)),
                    Expr::Literal(ScalarValue::Null),
                ])),
                op: BinaryOp::Eq,
                right: Box::new(array(vec![
                    Expr::Literal(ScalarValue::Int64(1)),
                    Expr::Literal(ScalarValue::Null),
                ])),
            },
            &batch,
        )
        .unwrap();
        assert!(nullable.as_boolean().is_null(0));
    }

    #[test]
    fn ordered_array_comparison_is_lexicographic_and_null_preserving() {
        let batch = RecordBatch::try_from_iter([(
            "id",
            Arc::new(Int64Array::from(vec![1])) as ArrayRef,
        )])
        .unwrap();
        let array = |values: Vec<Expr>| Expr::Function {
            name: "ARRAY".into(),
            args: values,
        };
        let less = evaluate(
            &Expr::BinaryOp {
                left: Box::new(array(vec![
                    Expr::Literal(ScalarValue::Int64(1)),
                    Expr::Literal(ScalarValue::Int64(2)),
                ])),
                op: BinaryOp::Lt,
                right: Box::new(array(vec![
                    Expr::Literal(ScalarValue::Int64(1)),
                    Expr::Literal(ScalarValue::Int64(3)),
                ])),
            },
            &batch,
        )
        .unwrap();
        assert!(less.as_boolean().value(0));
        let prefix = evaluate(
            &Expr::BinaryOp {
                left: Box::new(array(vec![Expr::Literal(ScalarValue::Int64(1))])),
                op: BinaryOp::Lt,
                right: Box::new(array(vec![
                    Expr::Literal(ScalarValue::Int64(1)),
                    Expr::Literal(ScalarValue::Int64(2)),
                ])),
            },
            &batch,
        )
        .unwrap();
        assert!(prefix.as_boolean().value(0));
        let nullable = evaluate(
            &Expr::BinaryOp {
                left: Box::new(array(vec![
                    Expr::Literal(ScalarValue::Int64(1)),
                    Expr::Literal(ScalarValue::Null),
                ])),
                op: BinaryOp::Lt,
                right: Box::new(array(vec![
                    Expr::Literal(ScalarValue::Int64(1)),
                    Expr::Literal(ScalarValue::Int64(2)),
                ])),
            },
            &batch,
        )
        .unwrap();
        assert!(nullable.as_boolean().is_null(0));
    }

    #[test]
    fn large_list_array_comparison_uses_the_same_semantics() {
        use arrow::array::{Int64Builder, LargeListBuilder, ListBuilder};

        let mut left_builder = LargeListBuilder::new(Int64Builder::new());
        left_builder.values().append_value(1);
        left_builder.values().append_value(2);
        left_builder.append(true);
        let left = Arc::new(left_builder.finish()) as ArrayRef;

        let mut right_builder = LargeListBuilder::new(Int64Builder::new());
        right_builder.values().append_value(1);
        right_builder.values().append_value(3);
        right_builder.append(true);
        let right = Arc::new(right_builder.finish()) as ArrayRef;

        let result = comparison(&left, &right, CompareKind::Lt).unwrap();
        assert!(result.value(0));

        let mut small_builder = ListBuilder::new(Int64Builder::new());
        small_builder.values().append_value(1);
        small_builder.values().append_value(2);
        small_builder.append(true);
        let small = Arc::new(small_builder.finish()) as ArrayRef;
        let mixed = comparison(&small, &right, CompareKind::Lt).unwrap();
        assert!(mixed.value(0));

        let batch = RecordBatch::try_from_iter([("arr", right.clone())]).unwrap();
        let cardinality = evaluate(
            &Expr::Function {
                name: "CARDINALITY".into(),
                args: vec![Expr::Column("arr".into())],
            },
            &batch,
        )
        .unwrap();
        assert_eq!(cardinality.as_primitive::<Int64Type>().value(0), 2);
    }

    #[test]
    fn expanding_strings_check_overflow_caps_and_query_budget_before_allocation() {
        let batch =
            RecordBatch::try_from_iter([("id", Arc::new(Int64Array::from(vec![1])) as ArrayRef)])
                .unwrap();
        let repeat = |count| Expr::Function {
            name: "REPEAT".into(),
            args: vec![
                Expr::Literal(ScalarValue::Utf8("xx".into())),
                Expr::Literal(ScalarValue::Int64(count)),
            ],
        };
        assert!(evaluate(&repeat(i64::MAX), &batch).is_err());
        assert!(evaluate(&repeat(100_000_000), &batch).is_err());
        let pool = kaveon_core::QueryMemoryPool::new("string-budget", 128).unwrap();
        let memory = pool.operator("expressions").unwrap();
        assert!(with_expression_memory(Some(&memory), || evaluate(&repeat(100), &batch)).is_err());
        assert_eq!(pool.snapshot().current_bytes, 0);
        // Error restores the thread-local scope; a later standalone expression
        // must not inherit the failed query's tiny budget.
        let value = evaluate(&repeat(100), &batch).unwrap();
        assert_eq!(value.as_string::<i32>().value(0).len(), 200);
        assert_eq!(
            evaluate(&repeat(-1), &batch)
                .unwrap()
                .as_string::<i32>()
                .value(0),
            ""
        );
    }
    use arrow::datatypes::{Field, Schema};

    fn batch() -> RecordBatch {
        RecordBatch::try_new(
            Arc::new(Schema::new(vec![Field::new(
                "value",
                DataType::Int32,
                false,
            )])),
            vec![Arc::new(Int32Array::from(vec![1, 2, 3]))],
        )
        .unwrap()
    }

    #[test]
    fn compares_dictionary_columns_with_literals_through_the_dictionary() {
        use arrow::array::{DictionaryArray, Int32Array};
        let schema = Arc::new(Schema::new(vec![Field::new(
            "day",
            DataType::Dictionary(Box::new(DataType::Int32), Box::new(DataType::Utf8)),
            true,
        )]));
        let days = DictionaryArray::<Int32Type>::new(
            Int32Array::from(vec![Some(0), Some(1), None, Some(2), Some(1)]),
            Arc::new(StringArray::from(vec![
                "2026-07-01",
                "2026-08-01",
                "2026-09-01",
            ])),
        );
        let batch = RecordBatch::try_new(schema, vec![Arc::new(days)]).unwrap();
        let expr = Expr::BinaryOp {
            left: Box::new(Expr::Column("day".into())),
            op: BinaryOp::Ge,
            right: Box::new(Expr::Literal(ScalarValue::Utf8("2026-08-01".into()))),
        };
        let mask = evaluate_predicate(&expr, &batch).unwrap();
        assert_eq!(
            (0..5)
                .map(|i| (!mask.is_null(i)).then(|| mask.value(i)))
                .collect::<Vec<_>>(),
            vec![Some(false), Some(true), None, Some(true), Some(true)]
        );
        // Mirrored: literal on the left flips the operator.
        let mirrored = Expr::BinaryOp {
            left: Box::new(Expr::Literal(ScalarValue::Utf8("2026-08-01".into()))),
            op: BinaryOp::Lt,
            right: Box::new(Expr::Column("day".into())),
        };
        let mask = evaluate_predicate(&mirrored, &batch).unwrap();
        assert_eq!(
            (0..5)
                .map(|i| (!mask.is_null(i)).then(|| mask.value(i)))
                .collect::<Vec<_>>(),
            vec![Some(false), Some(false), None, Some(true), Some(false)]
        );
    }

    #[test]
    fn string_functions_like_and_case_run_over_dictionary_columns() {
        use arrow::array::{DictionaryArray, Int32Array};
        let schema = Arc::new(Schema::new(vec![
            Field::new(
                "surface",
                DataType::Dictionary(Box::new(DataType::Int32), Box::new(DataType::Utf8)),
                true,
            ),
            Field::new(
                "region",
                DataType::Dictionary(Box::new(DataType::Int32), Box::new(DataType::Utf8)),
                true,
            ),
        ]));
        let surfaces = DictionaryArray::<Int32Type>::new(
            Int32Array::from(vec![Some(0), Some(1), None, Some(2), Some(1)]),
            Arc::new(StringArray::from(vec!["Chat", "Export", "api"])),
        );
        let regions = DictionaryArray::<Int32Type>::new(
            Int32Array::from(vec![Some(1), Some(0), Some(0), None, Some(1)]),
            Arc::new(StringArray::from(vec!["Asia", "Europe"])),
        );
        let batch =
            RecordBatch::try_new(schema, vec![Arc::new(surfaces), Arc::new(regions)]).unwrap();
        let strings = |array: ArrayRef| -> Vec<Option<String>> {
            let array = compute::cast(&array, &DataType::Utf8).unwrap();
            let array = array.as_string::<i32>();
            (0..array.len())
                .map(|i| (!array.is_null(i)).then(|| array.value(i).to_owned()))
                .collect()
        };

        // A unary function with a literal argument runs on the dictionary's
        // values, and its text result stays a dictionary over them.
        let upper = Expr::Function {
            name: "UPPER".into(),
            args: vec![Expr::Column("surface".into())],
        };
        let uppercased = evaluate(&upper, &batch).unwrap();
        assert_eq!(
            uppercased.data_type(),
            &DataType::Dictionary(Box::new(DataType::Int32), Box::new(DataType::Utf8))
        );
        assert_eq!(uppercased.as_dictionary::<Int32Type>().values().len(), 3);
        assert_eq!(
            strings(uppercased),
            vec![
                Some("CHAT".into()),
                Some("EXPORT".into()),
                None,
                Some("API".into()),
                Some("EXPORT".into())
            ]
        );
        // The dictionary result composes: a comparison, an IN list, a
        // concatenation and a nested function read it as its text.
        let uppercased_is_chat = Expr::BinaryOp {
            left: Box::new(upper.clone()),
            op: BinaryOp::Eq,
            right: Box::new(Expr::Literal(ScalarValue::Utf8("CHAT".into()))),
        };
        let mask = evaluate_predicate(&uppercased_is_chat, &batch).unwrap();
        assert_eq!(
            mask.iter().collect::<Vec<_>>(),
            vec![Some(true), Some(false), None, Some(false), Some(false)]
        );
        let uppercased_in = Expr::InList {
            expr: Box::new(upper.clone()),
            list: vec![
                Expr::Literal(ScalarValue::Utf8("API".into())),
                Expr::Literal(ScalarValue::Utf8("EXPORT".into())),
            ],
            negated: false,
        };
        let mask = evaluate_predicate(&uppercased_in, &batch).unwrap();
        assert_eq!(
            mask.iter().collect::<Vec<_>>(),
            vec![Some(false), Some(true), None, Some(true), Some(true)]
        );
        let uppercased_concat = Expr::BinaryOp {
            left: Box::new(upper.clone()),
            op: BinaryOp::StringConcat,
            right: Box::new(Expr::Literal(ScalarValue::Utf8("!".into()))),
        };
        assert_eq!(
            strings(evaluate(&uppercased_concat, &batch).unwrap()),
            vec![
                Some("CHAT!".into()),
                Some("EXPORT!".into()),
                None,
                Some("API!".into()),
                Some("EXPORT!".into())
            ]
        );
        let uppercased_length = Expr::Function {
            name: "LENGTH".into(),
            args: vec![upper.clone()],
        };
        let lengths = evaluate(&uppercased_length, &batch).unwrap();
        assert_eq!(
            lengths
                .as_primitive::<Int64Type>()
                .iter()
                .collect::<Vec<_>>(),
            vec![Some(4), Some(6), None, Some(3), Some(6)]
        );
        let left = Expr::Function {
            name: "LEFT".into(),
            args: vec![
                Expr::Column("surface".into()),
                Expr::Literal(ScalarValue::Int64(2)),
            ],
        };
        assert_eq!(
            strings(evaluate(&left, &batch).unwrap()),
            vec![
                Some("Ch".into()),
                Some("Ex".into()),
                None,
                Some("ap".into()),
                Some("Ex".into())
            ]
        );
        // Two dictionary columns decode and concatenate row by row.
        let concat = Expr::Function {
            name: "CONCAT".into(),
            args: vec![
                Expr::Column("surface".into()),
                Expr::Literal(ScalarValue::Utf8("/".into())),
                Expr::Column("region".into()),
            ],
        };
        let joined = strings(evaluate(&concat, &batch).unwrap());
        assert_eq!(joined[0].as_deref(), Some("Chat/Europe"));
        assert_eq!(joined[4].as_deref(), Some("Export/Europe"));

        // LIKE against a literal pattern matches the values once.
        let like = Expr::Like {
            expr: Box::new(Expr::Column("surface".into())),
            pattern: Box::new(Expr::Literal(ScalarValue::Utf8("Ex%".into()))),
            negated: false,
            case_insensitive: false,
        };
        let mask = evaluate_predicate(&like, &batch).unwrap();
        assert_eq!(
            (0..5)
                .map(|i| (!mask.is_null(i)).then(|| mask.value(i)))
                .collect::<Vec<_>>(),
            vec![Some(false), Some(true), None, Some(false), Some(true)]
        );
        let ilike = Expr::Like {
            expr: Box::new(Expr::Column("surface".into())),
            pattern: Box::new(Expr::Literal(ScalarValue::Utf8("API".into()))),
            negated: true,
            case_insensitive: true,
        };
        let mask = evaluate_predicate(&ilike, &batch).unwrap();
        assert_eq!(
            (0..5)
                .map(|i| (!mask.is_null(i)).then(|| mask.value(i)))
                .collect::<Vec<_>>(),
            vec![Some(true), Some(true), None, Some(false), Some(true)]
        );

        // CASE with a dictionary column in a branch produces plain text.
        let case = Expr::Case {
            operand: None,
            when_then: vec![(
                Expr::BinaryOp {
                    left: Box::new(Expr::Column("region".into())),
                    op: BinaryOp::Eq,
                    right: Box::new(Expr::Literal(ScalarValue::Utf8("Europe".into()))),
                },
                Expr::Column("surface".into()),
            )],
            else_expr: Some(Box::new(Expr::Literal(ScalarValue::Utf8("other".into())))),
        };
        assert_eq!(
            strings(evaluate(&case, &batch).unwrap()),
            vec![
                Some("Chat".into()),
                Some("other".into()),
                Some("other".into()),
                Some("other".into()),
                Some("Export".into())
            ]
        );
    }

    #[test]
    fn regexp_replace_extracts_hosts_with_capture_groups() {
        let schema = Arc::new(Schema::new(vec![Field::new(
            "referer",
            DataType::Utf8,
            true,
        )]));
        let batch = RecordBatch::try_new(
            schema,
            vec![Arc::new(StringArray::from(vec![
                Some("https://www.example.com/path?q=1"),
                Some("http://news.site.org/"),
                Some("not a url"),
                None,
            ]))],
        )
        .unwrap();
        let expr = Expr::Function {
            name: "REGEXP_REPLACE".into(),
            args: vec![
                Expr::Column("referer".into()),
                Expr::Literal(ScalarValue::Utf8(r"^https?://(?:www\.)?([^/]+)/.*$".into())),
                Expr::Literal(ScalarValue::Utf8("$1".into())),
            ],
        };
        let result = evaluate(&expr, &batch).unwrap();
        let result = as_string_array(&result).unwrap();
        assert_eq!(
            result.iter().collect::<Vec<_>>(),
            vec![
                Some("example.com"),
                Some("news.site.org"),
                Some("not a url"),
                None
            ]
        );
    }

    #[test]
    fn regexp_replace_keeps_unmatched_rows_and_grows_matched_ones() {
        // Rows without a match come back unchanged; a replacement longer
        // than its match grows the row; nulls stay null; and the same
        // pattern across batches hits the process-wide compiled cache.
        let schema = Arc::new(Schema::new(vec![Field::new("s", DataType::Utf8, true)]));
        let expr = Expr::Function {
            name: "REGEXP_REPLACE".into(),
            args: vec![
                Expr::Column("s".into()),
                Expr::Literal(ScalarValue::Utf8(r"a(\d)".into())),
                Expr::Literal(ScalarValue::Utf8("<$1$1$1>".into())),
            ],
        };
        for _ in 0..3 {
            let batch = RecordBatch::try_new(
                schema.clone(),
                vec![Arc::new(StringArray::from(vec![
                    Some("xa1ya2"),
                    Some("nothing here"),
                    None,
                    Some(""),
                ]))],
            )
            .unwrap();
            let result = evaluate(&expr, &batch).unwrap();
            let result = as_string_array(&result).unwrap();
            assert_eq!(
                result.iter().collect::<Vec<_>>(),
                vec![Some("x<111>y<222>"), Some("nothing here"), None, Some("")]
            );
        }
        assert!(compiled_regex("(").is_err());
    }

    #[test]
    fn regexp_replace_over_a_row_group_dictionary_runs_on_the_values_the_batch_uses() {
        use arrow::array::{DictionaryArray, Int32Array};
        // The dictionary a row group decodes to: two hundred values, of
        // which this batch's keys use four. The key under a null row is
        // arbitrary, as the reader leaves it, and must not be read.
        let values = (0..200)
            .map(|i| Some(format!("https://www.host-{i}.example.com/p/{i}")))
            .collect::<StringArray>();
        let keys = Int32Array::from(vec![
            Some(7),
            Some(150),
            None,
            Some(7),
            Some(199),
            Some(0),
            Some(150),
        ]);
        let keys = Int32Array::new(
            {
                let mut raw = keys.values().to_vec();
                raw[2] = 123_456;
                raw.into()
            },
            keys.nulls().cloned(),
        );
        let schema = Arc::new(Schema::new(vec![Field::new(
            "referer",
            DataType::Dictionary(Box::new(DataType::Int32), Box::new(DataType::Utf8)),
            true,
        )]));
        let column = DictionaryArray::<Int32Type>::new(keys, Arc::new(values));
        let batch = RecordBatch::try_new(schema, vec![Arc::new(column)]).unwrap();
        let expr = Expr::Function {
            name: "REGEXP_REPLACE".into(),
            args: vec![
                Expr::Column("referer".into()),
                Expr::Literal(ScalarValue::Utf8(r"^https?://(?:www\.)?([^/]+)/.*$".into())),
                Expr::Literal(ScalarValue::Utf8("$1".into())),
            ],
        };
        let result = evaluate(&expr, &batch).unwrap();
        let dictionary = result.as_dictionary::<Int32Type>();
        // Four regular expressions ran, not two hundred: the result's
        // dictionary holds the used values in first-seen order.
        assert_eq!(
            dictionary
                .values()
                .as_string::<i32>()
                .iter()
                .collect::<Vec<_>>(),
            vec![
                Some("host-7.example.com"),
                Some("host-150.example.com"),
                Some("host-199.example.com"),
                Some("host-0.example.com"),
            ]
        );
        let text = compute::cast(&result, &DataType::Utf8).unwrap();
        assert_eq!(
            text.as_string::<i32>().iter().collect::<Vec<_>>(),
            vec![
                Some("host-7.example.com"),
                Some("host-150.example.com"),
                None,
                Some("host-7.example.com"),
                Some("host-199.example.com"),
                Some("host-0.example.com"),
                Some("host-150.example.com"),
            ]
        );
        assert!(result.is_null(2));

        // LENGTH over the same column gathers a plain integer column.
        let length = Expr::Function {
            name: "LENGTH".into(),
            args: vec![Expr::Column("referer".into())],
        };
        let lengths = evaluate(&length, &batch).unwrap();
        assert_eq!(lengths.data_type(), &DataType::Int64);
        assert_eq!(
            lengths
                .as_primitive::<Int64Type>()
                .iter()
                .collect::<Vec<_>>(),
            vec![
                Some(34),
                Some(38),
                None,
                Some(34),
                Some(38),
                Some(34),
                Some(38)
            ]
        );

        // LIKE takes the same compaction.
        let like = Expr::Like {
            expr: Box::new(Expr::Column("referer".into())),
            pattern: Box::new(Expr::Literal(ScalarValue::Utf8("%host-1%".into()))),
            negated: false,
            case_insensitive: false,
        };
        let mask = evaluate_predicate(&like, &batch).unwrap();
        assert_eq!(
            mask.iter().collect::<Vec<_>>(),
            vec![
                Some(false),
                Some(true),
                None,
                Some(false),
                Some(true),
                Some(false),
                Some(true)
            ]
        );
    }

    #[test]
    fn regexp_replace_over_plain_text_runs_once_per_distinct_value() {
        // Eight thousand rows over fifty values, nulls and empty strings
        // among them, multi-byte hosts included: the distinct-value path
        // must answer exactly as the row path does, as plain text.
        let value = |i: usize| -> Option<String> {
            match i % 50 {
                0 => None,
                1 => Some(String::new()),
                2 => Some("https://пример.рф/путь/к/странице".into()),
                3 => Some("https://no-path.example.com".into()),
                k => Some(format!("http://www.site-{k}.example.com/a/{}", k % 3)),
            }
        };
        let rows = (0..8_192).map(value).collect::<StringArray>();
        let schema = Arc::new(Schema::new(vec![Field::new("s", DataType::Utf8, true)]));
        let batch = RecordBatch::try_new(schema.clone(), vec![Arc::new(rows)]).unwrap();
        let expr = Expr::Function {
            name: "REGEXP_REPLACE".into(),
            args: vec![
                Expr::Column("s".into()),
                Expr::Literal(ScalarValue::Utf8(r"^https?://(?:www\.)?([^/]+)/.*$".into())),
                Expr::Literal(ScalarValue::Utf8("$1".into())),
            ],
        };
        let result = evaluate(&expr, &batch).unwrap();
        assert_eq!(result.data_type(), &DataType::Utf8);
        let result = result.as_string::<i32>();
        let expected = |i: usize| -> Option<String> {
            match i % 50 {
                0 => None,
                1 => Some(String::new()),
                2 => Some("пример.рф".into()),
                3 => Some("https://no-path.example.com".into()),
                k => Some(format!("site-{k}.example.com")),
            }
        };
        for i in 0..8_192 {
            assert_eq!(
                (!result.is_null(i)).then(|| result.value(i).to_owned()),
                expected(i),
                "row {i}"
            );
        }
        assert_eq!(
            distinct_text_values(batch.column(0).as_string::<i32>())
                .map(|(values, _)| values.len()),
            Some(49)
        );

        // A batch that is mostly distinct is left to the row path and
        // answers the same.
        let rows = (0..2_048)
            .map(|i| Some(format!("https://site-{i}.example.com/{}", i % 2)))
            .collect::<StringArray>();
        assert!(distinct_text_values(&rows).is_none());
        let batch = RecordBatch::try_new(schema, vec![Arc::new(rows)]).unwrap();
        let result = evaluate(&expr, &batch).unwrap();
        let result = result.as_string::<i32>();
        assert_eq!(result.value(0), "site-0.example.com");
        assert_eq!(result.value(2_047), "site-2047.example.com");
    }

    #[test]
    fn regexp_replace_rejects_invalid_patterns_by_name() {
        let schema = Arc::new(Schema::new(vec![Field::new("s", DataType::Utf8, true)]));
        let batch = RecordBatch::try_new(
            schema,
            vec![Arc::new(StringArray::from(vec![Some("a"), Some("a")]))],
        )
        .unwrap();
        for pattern in ["(unclosed", r"(?-u)\xFF"] {
            let expr = Expr::Function {
                name: "REGEXP_REPLACE".into(),
                args: vec![
                    Expr::Column("s".into()),
                    Expr::Literal(ScalarValue::Utf8(pattern.into())),
                    Expr::Literal(ScalarValue::Utf8("x".into())),
                ],
            };
            let error = evaluate(&expr, &batch).unwrap_err().to_string();
            assert!(
                error.contains("REGEXP_REPLACE pattern") && error.contains(pattern),
                "{error}"
            );
        }
    }

    #[test]
    fn regexp_extract_returns_capture_groups_and_nulls_on_no_match() {
        let schema = Arc::new(Schema::new(vec![Field::new("s", DataType::Utf8, true)]));
        let batch = RecordBatch::try_new(
            schema,
            vec![Arc::new(StringArray::from(vec![
                Some("id=42 kind=book"),
                Some("no id here"),
                None,
            ]))],
        )
        .unwrap();
        let expr = Expr::Function {
            name: "REGEXP_EXTRACT".into(),
            args: vec![
                Expr::Column("s".into()),
                Expr::Literal(ScalarValue::Utf8(r"id=(\d+) kind=(\w+)".into())),
                Expr::Literal(ScalarValue::Int64(1)),
            ],
        };
        let result = evaluate(&expr, &batch).unwrap();
        let values = as_string_array(&result).unwrap();
        assert_eq!(values.value(0), "42");
        assert!(values.is_null(1));
        assert!(values.is_null(2));

        let second_group = Expr::Function {
            name: "REGEXP_EXTRACT".into(),
            args: vec![
                Expr::Column("s".into()),
                Expr::Literal(ScalarValue::Utf8(r"id=(\d+) kind=(\w+)".into())),
                Expr::Literal(ScalarValue::Int64(2)),
            ],
        };
        let result = evaluate(&second_group, &batch).unwrap();
        assert_eq!(as_string_array(&result).unwrap().value(0), "book");

        let invalid_group = Expr::Function {
            name: "REGEXP_EXTRACT".into(),
            args: vec![
                Expr::Column("s".into()),
                Expr::Literal(ScalarValue::Utf8("id=(\\d+)".into())),
                Expr::Literal(ScalarValue::Int64(-1)),
            ],
        };
        assert!(evaluate(&invalid_group, &batch)
            .unwrap_err()
            .to_string()
            .contains("group must be non-negative"));
    }

    #[test]
    fn regexp_split_preserves_trailing_empty_fields_and_nulls() {
        let schema = Arc::new(Schema::new(vec![Field::new("s", DataType::Utf8, true)]));
        let batch = RecordBatch::try_new(
            schema,
            vec![Arc::new(StringArray::from(vec![
                Some("one,two,"),
                Some("no delimiter"),
                None,
            ]))],
        )
        .unwrap();
        let expr = Expr::Function {
            name: "REGEXP_SPLIT".into(),
            args: vec![
                Expr::Column("s".into()),
                Expr::Literal(ScalarValue::Utf8(",".into())),
            ],
        };
        let result = evaluate(&expr, &batch).unwrap();
        let values = result.as_any().downcast_ref::<ListArray>().unwrap();
        let first = values.value(0);
        assert_eq!(
            first
                .as_any()
                .downcast_ref::<StringArray>()
                .unwrap()
                .iter()
                .collect::<Vec<_>>(),
            vec![Some("one"), Some("two"), Some("")]
        );
        assert_eq!(values.value(1).len(), 1);
        assert!(values.is_null(2));
    }

    #[test]
    fn rewrap_dictionary_moves_null_values_into_the_keys() {
        let keys = Int32Array::from(vec![Some(0), Some(1), None, Some(1)]);
        let values: ArrayRef = Arc::new(StringArray::from(vec![Some("a"), None]));
        let wrapped = rewrap_dictionary(&keys, values).unwrap();
        assert_eq!(
            (0..4).map(|i| wrapped.is_null(i)).collect::<Vec<_>>(),
            vec![false, true, true, true]
        );
        let text = compute::cast(&wrapped, &DataType::Utf8).unwrap();
        assert_eq!(
            text.as_string::<i32>().iter().collect::<Vec<_>>(),
            vec![Some("a"), None, None, None]
        );
        // Anything but text gathers by key.
        let keys = Int32Array::from(vec![Some(1), None, Some(0)]);
        let values: ArrayRef = Arc::new(Int64Array::from(vec![10, 20]));
        let gathered = rewrap_dictionary(&keys, values).unwrap();
        assert_eq!(
            gathered
                .as_primitive::<Int64Type>()
                .iter()
                .collect::<Vec<_>>(),
            vec![Some(20), None, Some(10)]
        );
    }

    /// The per-row cost of REGEXP_REPLACE and LIKE on the shape ClickBench
    /// q29 hands the projection: four million Referer-like URLs in 8192-row
    /// batches, about five percent of a batch distinct, as plain UTF-8, as
    /// the dictionary a fallback (plain-encoded) page decodes to — one
    /// dictionary per batch holding the batch's own values — and as the
    /// dictionary a dictionary-encoded row group decodes to — one values
    /// array shared by every batch of the row group. Ignored by default;
    /// run it as `cargo test --release -p kaveon-exec regex_rate -- --ignored
    /// --nocapture`.
    #[test]
    #[ignore = "benchmark: prints the per-row regex figures, run explicitly in release"]
    fn regex_rate_over_referer_like_strings() {
        use arrow::array::{DictionaryArray, Int32Array};
        const ROWS: usize = 4_000_000;
        const BATCH_ROWS: usize = 8_192;
        const DISTINCT_PER_BATCH: usize = 410;
        const ROW_GROUP_BATCHES: usize = 122;
        const HOSTS: usize = 4_000;
        let batches = ROWS / BATCH_ROWS;
        let rows_total = batches * BATCH_ROWS;
        // A URL shaped like a Referer: scheme, an optional `www.`, a host
        // from a pool, a path with a query; one in twelve has no path and
        // does not match q29's pattern, one in forty is empty.
        let url = |seed: usize| -> String {
            let host = seed % HOSTS;
            let scheme = if seed.is_multiple_of(3) {
                "http"
            } else {
                "https"
            };
            let www = if seed.is_multiple_of(2) { "www." } else { "" };
            if seed.is_multiple_of(40) {
                String::new()
            } else if seed.is_multiple_of(12) {
                format!("{scheme}://{www}site-{host}.example.com")
            } else {
                format!(
                    "{scheme}://{www}site-{host}.example.com/section/{}/page-{}.html?ref={}&q=kaveon",
                    seed % 97,
                    seed % 1_013,
                    seed % 7
                )
            }
        };
        // Row `i` of batch `b` takes value `b * 410 + (i * 7919) % 410`:
        // each batch draws on 410 values of its own, each used about twenty
        // times, spread over the batch.
        let value_of = |batch: usize, row: usize| {
            batch * DISTINCT_PER_BATCH + (row * 7_919) % DISTINCT_PER_BATCH
        };
        let schema_plain = Arc::new(Schema::new(vec![Field::new(
            "referer",
            DataType::Utf8,
            true,
        )]));
        let schema_dictionary = Arc::new(Schema::new(vec![Field::new(
            "referer",
            DataType::Dictionary(Box::new(DataType::Int32), Box::new(DataType::Utf8)),
            true,
        )]));
        let build_started = std::time::Instant::now();
        let plain = (0..batches)
            .map(|batch| {
                let values = (0..BATCH_ROWS)
                    .map(|row| Some(url(value_of(batch, row))))
                    .collect::<StringArray>();
                RecordBatch::try_new(schema_plain.clone(), vec![Arc::new(values)]).unwrap()
            })
            .collect::<Vec<_>>();
        let per_batch = (0..batches)
            .map(|batch| {
                let values = (0..DISTINCT_PER_BATCH)
                    .map(|slot| Some(url(batch * DISTINCT_PER_BATCH + slot)))
                    .collect::<StringArray>();
                let keys = (0..BATCH_ROWS)
                    .map(|row| Some((value_of(batch, row) - batch * DISTINCT_PER_BATCH) as i32))
                    .collect::<Int32Array>();
                let column = DictionaryArray::<Int32Type>::new(keys, Arc::new(values));
                RecordBatch::try_new(schema_dictionary.clone(), vec![Arc::new(column)]).unwrap()
            })
            .collect::<Vec<_>>();
        let shared = (0..batches)
            .scan(None::<(usize, ArrayRef)>, |group, batch| {
                let first = batch - batch % ROW_GROUP_BATCHES;
                if group.as_ref().is_none_or(|(start, _)| *start != first) {
                    let last = (first + ROW_GROUP_BATCHES).min(batches);
                    let values = (first * DISTINCT_PER_BATCH..last * DISTINCT_PER_BATCH)
                        .map(|seed| Some(url(seed)))
                        .collect::<StringArray>();
                    *group = Some((first, Arc::new(values) as ArrayRef));
                }
                let (first, values) = group.as_ref().expect("set above");
                let keys = (0..BATCH_ROWS)
                    .map(|row| Some((value_of(batch, row) - first * DISTINCT_PER_BATCH) as i32))
                    .collect::<Int32Array>();
                let column = DictionaryArray::<Int32Type>::new(keys, Arc::clone(values));
                Some(
                    RecordBatch::try_new(schema_dictionary.clone(), vec![Arc::new(column)])
                        .unwrap(),
                )
            })
            .collect::<Vec<_>>();
        // Every row its own value: the shape the distinct-value path must
        // step aside for.
        let unique = (0..batches)
            .map(|batch| {
                let values = (0..BATCH_ROWS)
                    .map(|row| Some(url(batch * BATCH_ROWS + row)))
                    .collect::<StringArray>();
                RecordBatch::try_new(schema_plain.clone(), vec![Arc::new(values)]).unwrap()
            })
            .collect::<Vec<_>>();
        println!(
            "built {} rows in {} batches ({} distinct per batch) in {:.2?}",
            rows_total,
            batches,
            DISTINCT_PER_BATCH,
            build_started.elapsed()
        );
        let regexp_replace = Expr::Function {
            name: "REGEXP_REPLACE".into(),
            args: vec![
                Expr::Column("referer".into()),
                Expr::Literal(ScalarValue::Utf8(r"^https?://(?:www\.)?([^/]+)/.*$".into())),
                Expr::Literal(ScalarValue::Utf8("$1".into())),
            ],
        };
        let like = Expr::Like {
            expr: Box::new(Expr::Column("referer".into())),
            pattern: Box::new(Expr::Literal(ScalarValue::Utf8("%site-1%".into()))),
            negated: false,
            case_insensitive: false,
        };
        // Hits: hosts extracted (no `/` left) for REGEXP_REPLACE, true rows
        // for LIKE; every shape must agree with the plain rows.
        let hits = |expr: &Expr, input: &[RecordBatch]| -> usize {
            input
                .iter()
                .map(|batch| {
                    let result = evaluate(expr, batch).unwrap();
                    match result.data_type() {
                        DataType::Boolean => result.as_boolean().true_count(),
                        _ => compute::cast(&result, &DataType::Utf8)
                            .unwrap()
                            .as_string::<i32>()
                            .iter()
                            .filter(|value| value.is_some_and(|value| !value.contains('/')))
                            .count(),
                    }
                })
                .sum()
        };
        for (label, expr) in [("REGEXP_REPLACE", &regexp_replace), ("LIKE", &like)] {
            let expected = hits(expr, &plain);
            for (shape, input) in [
                ("plain Utf8", &plain),
                ("dictionary per batch", &per_batch),
                ("dictionary per row group", &shared),
                ("plain Utf8, rows distinct", &unique),
            ] {
                if shape.ends_with("distinct") {
                    // Its own values: nothing to agree with.
                    assert!(hits(expr, input) > 0, "{label} over {shape}");
                } else {
                    assert_eq!(hits(expr, input), expected, "{label} over {shape}");
                }
                // Three rounds over the same batches: the first warms the
                // allocator, the best is the figure to record.
                let mut best = std::time::Duration::MAX;
                for _ in 0..3 {
                    let started = std::time::Instant::now();
                    let mut rows = 0;
                    for batch in input {
                        rows += evaluate(expr, batch).unwrap().len();
                    }
                    assert_eq!(rows, rows_total);
                    best = best.min(started.elapsed());
                }
                println!(
                    "{label:<15} {shape:<26} {:>7.1} ns/row  ({:.2?} for {} rows)",
                    best.as_nanos() as f64 / rows_total as f64,
                    best,
                    rows_total,
                );
            }
        }
    }

    #[test]
    fn arithmetic_with_a_literal_takes_the_scalar_kernel_with_the_same_types() {
        // An Int32 column with an integer literal meets as Int64 on either
        // side; with a float literal as Float64; a date as its day number;
        // division by zero is still an error, as on the array path.
        let schema = Arc::new(Schema::new(vec![
            Field::new("w", DataType::Int32, true),
            Field::new("d", DataType::Date32, false),
        ]));
        let batch = RecordBatch::try_new(
            schema,
            vec![
                Arc::new(Int32Array::from(vec![Some(10), None, Some(-3)])),
                Arc::new(arrow::array::Date32Array::from(vec![10, 11, 12])),
            ],
        )
        .unwrap();
        let op = |left: Expr, op: BinaryOp, right: Expr| Expr::BinaryOp {
            left: Box::new(left),
            op,
            right: Box::new(right),
        };
        let column = || Expr::Column("w".into());
        let int = |v: i64| Expr::Literal(ScalarValue::Int64(v));
        let plus = evaluate(&op(column(), BinaryOp::Plus, int(5)), &batch).unwrap();
        assert_eq!(
            plus.as_primitive::<Int64Type>().iter().collect::<Vec<_>>(),
            vec![Some(15), None, Some(2)]
        );
        let reversed = evaluate(&op(int(100), BinaryOp::Minus, column()), &batch).unwrap();
        assert_eq!(
            reversed
                .as_primitive::<Int64Type>()
                .iter()
                .collect::<Vec<_>>(),
            vec![Some(90), None, Some(103)]
        );
        let float = evaluate(
            &op(
                column(),
                BinaryOp::Multiply,
                Expr::Literal(ScalarValue::Float64(0.5)),
            ),
            &batch,
        )
        .unwrap();
        assert_eq!(
            float
                .as_primitive::<Float64Type>()
                .iter()
                .collect::<Vec<_>>(),
            vec![Some(5.0), None, Some(-1.5)]
        );
        let day = evaluate(
            &op(Expr::Column("d".into()), BinaryOp::Plus, int(1)),
            &batch,
        )
        .unwrap();
        assert_eq!(
            day.as_primitive::<Int64Type>().iter().collect::<Vec<_>>(),
            vec![Some(11), Some(12), Some(13)]
        );
        assert!(evaluate(&op(column(), BinaryOp::Divide, int(0)), &batch).is_err());
    }

    #[test]
    fn compares_integer_column_with_float_literal() {
        let expr = Expr::BinaryOp {
            left: Box::new(Expr::Column("value".into())),
            op: BinaryOp::Gt,
            right: Box::new(Expr::Literal(ScalarValue::Float64(1.5))),
        };
        assert_eq!(
            evaluate_predicate(&expr, &batch())
                .unwrap()
                .values()
                .iter()
                .collect::<Vec<_>>(),
            vec![false, true, true]
        );
    }

    #[test]
    fn adds_integer_column_and_float_literal() {
        let expr = Expr::BinaryOp {
            left: Box::new(Expr::Column("value".into())),
            op: BinaryOp::Plus,
            right: Box::new(Expr::Literal(ScalarValue::Float64(0.5))),
        };
        let result = evaluate(&expr, &batch()).unwrap();
        assert_eq!(
            result.as_primitive::<Float64Type>().values(),
            &[1.5, 2.5, 3.5]
        );
    }

    fn string_batch() -> RecordBatch {
        RecordBatch::try_new(
            Arc::new(Schema::new(vec![Field::new("name", DataType::Utf8, true)])),
            vec![Arc::new(StringArray::from(vec![
                Some("Hello"),
                Some("World"),
                None,
            ]))],
        )
        .unwrap()
    }

    #[test]
    fn evaluates_like_pattern() {
        let expr = Expr::Like {
            expr: Box::new(Expr::Column("name".into())),
            pattern: Box::new(Expr::Literal(ScalarValue::Utf8("H%".into()))),
            negated: false,
            case_insensitive: false,
        };
        let result = evaluate(&expr, &string_batch()).unwrap();
        let bools = as_boolean(&result).unwrap();
        assert!(bools.value(0));
        assert!(!bools.value(1));
        assert!(bools.is_null(2));
    }

    #[test]
    fn evaluates_case_expression() {
        let batch = RecordBatch::try_new(
            Arc::new(Schema::new(vec![Field::new("x", DataType::Int64, false)])),
            vec![Arc::new(Int64Array::from(vec![1, 2, 3]))],
        )
        .unwrap();
        let expr = Expr::Case {
            operand: None,
            when_then: vec![
                (
                    Expr::BinaryOp {
                        left: Box::new(Expr::Column("x".into())),
                        op: BinaryOp::Eq,
                        right: Box::new(Expr::Literal(ScalarValue::Int64(1))),
                    },
                    Expr::Literal(ScalarValue::Utf8("one".into())),
                ),
                (
                    Expr::BinaryOp {
                        left: Box::new(Expr::Column("x".into())),
                        op: BinaryOp::Eq,
                        right: Box::new(Expr::Literal(ScalarValue::Int64(2))),
                    },
                    Expr::Literal(ScalarValue::Utf8("two".into())),
                ),
            ],
            else_expr: Some(Box::new(Expr::Literal(ScalarValue::Utf8("other".into())))),
        };
        let result = evaluate(&expr, &batch).unwrap();
        let arr = as_string_array(&result).unwrap();
        assert_eq!(arr.value(0), "one");
        assert_eq!(arr.value(1), "two");
        assert_eq!(arr.value(2), "other");
    }

    #[test]
    fn evaluates_in_list() {
        let batch = RecordBatch::try_new(
            Arc::new(Schema::new(vec![Field::new("x", DataType::Int64, false)])),
            vec![Arc::new(Int64Array::from(vec![1, 2, 3, 4]))],
        )
        .unwrap();
        let expr = Expr::InList {
            expr: Box::new(Expr::Column("x".into())),
            list: vec![
                Expr::Literal(ScalarValue::Int64(1)),
                Expr::Literal(ScalarValue::Int64(3)),
            ],
            negated: false,
        };
        let result = evaluate_predicate(&expr, &batch).unwrap();
        assert_eq!(
            result.values().iter().collect::<Vec<_>>(),
            vec![true, false, true, false]
        );
    }

    #[test]
    fn evaluates_between() {
        let batch = RecordBatch::try_new(
            Arc::new(Schema::new(vec![Field::new("x", DataType::Int64, false)])),
            vec![Arc::new(Int64Array::from(vec![1, 5, 10, 15]))],
        )
        .unwrap();
        let expr = Expr::Between {
            expr: Box::new(Expr::Column("x".into())),
            low: Box::new(Expr::Literal(ScalarValue::Int64(5))),
            high: Box::new(Expr::Literal(ScalarValue::Int64(10))),
            negated: false,
        };
        let result = evaluate_predicate(&expr, &batch).unwrap();
        assert_eq!(
            result.values().iter().collect::<Vec<_>>(),
            vec![false, true, true, false]
        );
    }

    #[test]
    fn decimal_literal_arithmetic_stays_exact_and_bounds_a_double_column() {
        let batch = RecordBatch::try_new(
            Arc::new(Schema::new(vec![Field::new(
                "discount",
                DataType::Float64,
                false,
            )])),
            vec![Arc::new(Float64Array::from(vec![
                0.04, 0.05, 0.06, 0.07, 0.08,
            ]))],
        )
        .unwrap();
        let decimal = |value: i128| {
            Box::new(Expr::Literal(ScalarValue::Decimal128 {
                value,
                precision: 3,
                scale: 2,
            }))
        };
        let low = Expr::BinaryOp {
            left: decimal(6),
            op: BinaryOp::Minus,
            right: decimal(1),
        };
        let high = Expr::BinaryOp {
            left: decimal(6),
            op: BinaryOp::Plus,
            right: decimal(1),
        };
        let bound = evaluate(&low, &batch).unwrap();
        let DataType::Decimal128(_, scale) = bound.data_type() else {
            panic!(
                "decimal arithmetic keeps the decimal type: {}",
                bound.data_type()
            );
        };
        let values = bound.as_primitive::<arrow::datatypes::Decimal128Type>();
        assert_eq!(
            values.value(0),
            5 * 10_i128.pow(u32::from(*scale as u8) - 2)
        );
        let expr = Expr::Between {
            expr: Box::new(Expr::Column("discount".into())),
            low: Box::new(low),
            high: Box::new(high),
            negated: false,
        };
        let result = evaluate_predicate(&expr, &batch).unwrap();
        assert_eq!(
            result.values().iter().collect::<Vec<_>>(),
            vec![false, true, true, true, false]
        );
    }

    #[test]
    fn decimal_array_literals_preserve_exact_element_type() {
        let batch = RecordBatch::try_from_iter([(
            "id",
            Arc::new(Int64Array::from(vec![1])) as ArrayRef,
        )])
        .unwrap();
        let decimal = |value: i128| {
            Expr::Literal(ScalarValue::Decimal128 {
                value,
                precision: 20,
                scale: 2,
            })
        };
        let array = || Expr::Function {
            name: "ARRAY".into(),
            args: vec![decimal(9007199254740992), decimal(1)],
        };
        let element = Expr::Function {
            name: "ELEMENT_AT".into(),
            args: vec![array(), Expr::Literal(ScalarValue::Int64(1))],
        };
        let result = evaluate(&element, &batch).unwrap();
        assert_eq!(result.data_type(), &DataType::Decimal128(20, 2));
        assert_eq!(
            result
                .as_primitive::<arrow::datatypes::Decimal128Type>()
                .value(0),
            9007199254740992
        );
        let contains = Expr::Function {
            name: "CONTAINS".into(),
            args: vec![array(), decimal(1)],
        };
        assert!(evaluate(&contains, &batch).unwrap().as_boolean().value(0));
        let position = Expr::Function {
            name: "ARRAY_POSITION".into(),
            args: vec![array(), decimal(1)],
        };
        assert_eq!(
            evaluate(&position, &batch)
                .unwrap()
                .as_primitive::<Int64Type>()
                .value(0),
            2
        );
        let distinct = Expr::Function {
            name: "ARRAY_DISTINCT".into(),
            args: vec![Expr::Function {
                name: "ARRAY".into(),
                args: vec![decimal(1), decimal(1), decimal(2)],
            }],
        };
        let distinct = evaluate(&distinct, &batch).unwrap();
        let distinct = distinct
            .as_any()
            .downcast_ref::<ListArray>()
            .unwrap()
            .value(0);
        assert_eq!(distinct.len(), 2);
        assert_eq!(
            distinct
                .as_primitive::<arrow::datatypes::Decimal128Type>()
                .value(1),
            2
        );
        let sorted = Expr::Function {
            name: "ARRAY_SORT".into(),
            args: vec![Expr::Function {
                name: "ARRAY".into(),
                args: vec![decimal(2), decimal(1), Expr::Literal(ScalarValue::Null)],
            }],
        };
        let sorted = evaluate(&sorted, &batch).unwrap();
        let sorted = sorted
            .as_any()
            .downcast_ref::<ListArray>()
            .unwrap()
            .value(0);
        let sorted = sorted.as_primitive::<arrow::datatypes::Decimal128Type>();
        assert_eq!(sorted.value(0), 1);
        assert_eq!(sorted.value(1), 2);
        assert!(sorted.is_null(2));

        let reversed = Expr::Function {
            name: "ARRAY_REVERSE".into(),
            args: vec![Expr::Function {
                name: "ARRAY".into(),
                args: vec![decimal(2), decimal(1), Expr::Literal(ScalarValue::Null)],
            }],
        };
        let reversed = evaluate(&reversed, &batch).unwrap();
        let reversed = reversed
            .as_any()
            .downcast_ref::<ListArray>()
            .unwrap()
            .value(0);
        let reversed = reversed.as_primitive::<arrow::datatypes::Decimal128Type>();
        assert!(reversed.is_null(0));
        assert_eq!(reversed.value(1), 1);
        assert_eq!(reversed.value(2), 2);

        let sliced = Expr::Function {
            name: "ARRAY_SLICE".into(),
            args: vec![
                Expr::Function {
                    name: "ARRAY".into(),
                    args: vec![decimal(1), decimal(2), decimal(3)],
                },
                Expr::Literal(ScalarValue::Int64(2)),
                Expr::Literal(ScalarValue::Int64(3)),
                Expr::Literal(ScalarValue::Int64(1)),
            ],
        };
        let sliced = evaluate(&sliced, &batch).unwrap();
        let sliced = sliced
            .as_any()
            .downcast_ref::<ListArray>()
            .unwrap()
            .value(0);
        let sliced = sliced.as_primitive::<arrow::datatypes::Decimal128Type>();
        assert_eq!(sliced.len(), 2);
        assert_eq!(sliced.value(0), 2);
        assert_eq!(sliced.value(1), 3);

        for (name, expected) in [("ARRAY_MIN", 1_i128), ("ARRAY_MAX", 3_i128)] {
            let extremum = Expr::Function {
                name: name.into(),
                args: vec![Expr::Function {
                    name: "ARRAY".into(),
                    args: vec![decimal(3), Expr::Literal(ScalarValue::Null), decimal(1)],
                }],
            };
            let extremum = evaluate(&extremum, &batch).unwrap();
            assert_eq!(
                extremum
                    .as_primitive::<arrow::datatypes::Decimal128Type>()
                    .value(0),
                expected
            );
            assert_eq!(extremum.data_type(), &DataType::Decimal128(20, 2));
        }

        let overlap = Expr::Function {
            name: "ARRAYS_OVERLAP".into(),
            args: vec![
                Expr::Function {
                    name: "ARRAY".into(),
                    args: vec![decimal(1), decimal(2)],
                },
                Expr::Function {
                    name: "ARRAY".into(),
                    args: vec![Expr::Literal(ScalarValue::Null), decimal(2)],
                },
            ],
        };
        assert!(evaluate(&overlap, &batch).unwrap().as_boolean().value(0));

        let null_overlap = Expr::Function {
            name: "ARRAYS_OVERLAP".into(),
            args: vec![
                Expr::Function {
                    name: "ARRAY".into(),
                    args: vec![decimal(3), Expr::Literal(ScalarValue::Null)],
                },
                Expr::Function {
                    name: "ARRAY".into(),
                    args: vec![decimal(4)],
                },
            ],
        };
        assert!(evaluate(&null_overlap, &batch).unwrap().as_boolean().is_null(0));

        let concat = Expr::Function {
            name: "ARRAY_CONCAT".into(),
            args: vec![
                Expr::Function {
                    name: "ARRAY".into(),
                    args: vec![decimal(1), Expr::Literal(ScalarValue::Null)],
                },
                Expr::Function {
                    name: "ARRAY".into(),
                    args: vec![decimal(2)],
                },
            ],
        };
        let concat = evaluate(&concat, &batch).unwrap();
        let concat = concat
            .as_any()
            .downcast_ref::<ListArray>()
            .unwrap()
            .value(0);
        let concat = concat.as_primitive::<arrow::datatypes::Decimal128Type>();
        assert_eq!(concat.data_type(), &DataType::Decimal128(20, 2));
        assert_eq!(concat.value(0), 1);
        assert!(concat.is_null(1));
        assert_eq!(concat.value(2), 2);

        for (name, expected) in [
            ("ARRAY_APPEND", vec![1_i128, 2, 3]),
            ("ARRAY_PREPEND", vec![3_i128, 1, 2]),
        ] {
            let updated = Expr::Function {
                name: name.into(),
                args: vec![
                    Expr::Function {
                        name: "ARRAY".into(),
                        args: vec![decimal(1), decimal(2)],
                    },
                    decimal(3),
                ],
            };
            let updated = evaluate(&updated, &batch).unwrap();
            let updated = updated
                .as_any()
                .downcast_ref::<ListArray>()
                .unwrap()
                .value(0);
            let updated = updated.as_primitive::<arrow::datatypes::Decimal128Type>();
            assert_eq!(
                (0..updated.len()).map(|index| updated.value(index)).collect::<Vec<_>>(),
                expected
            );
        }

        let removed = Expr::Function {
            name: "ARRAY_REMOVE".into(),
            args: vec![
                Expr::Function {
                    name: "ARRAY".into(),
                    args: vec![decimal(1), decimal(2), decimal(1), Expr::Literal(ScalarValue::Null)],
                },
                decimal(1),
            ],
        };
        let removed = evaluate(&removed, &batch).unwrap();
        let removed = removed
            .as_any()
            .downcast_ref::<ListArray>()
            .unwrap()
            .value(0);
        let removed = removed.as_primitive::<arrow::datatypes::Decimal128Type>();
        assert_eq!(removed.len(), 2);
        assert_eq!(removed.value(0), 2);
        assert!(removed.is_null(1));
    }

    #[test]
    fn evaluates_upper_lower() {
        let expr_upper = Expr::Function {
            name: "UPPER".into(),
            args: vec![Expr::Column("name".into())],
        };
        let result = evaluate(&expr_upper, &string_batch()).unwrap();
        let arr = as_string_array(&result).unwrap();
        assert_eq!(arr.value(0), "HELLO");
        assert_eq!(arr.value(1), "WORLD");
        assert!(arr.is_null(2));
    }

    #[test]
    fn chr_and_codepoint_round_trip_unicode_and_reject_invalid_width() {
        let batch = RecordBatch::try_new(
            Arc::new(Schema::new(vec![
                Field::new("code", DataType::Int64, true),
                Field::new("text", DataType::Utf8, true),
            ])),
            vec![
                Arc::new(Int64Array::from(vec![Some(0x41), Some(0x1F600), None])),
                Arc::new(StringArray::from(vec![Some("A"), Some("😀"), None])),
            ],
        )
        .unwrap();
        let chr = Expr::Function {
            name: "CHR".into(),
            args: vec![Expr::Column("code".into())],
        };
        let result = evaluate(&chr, &batch).unwrap();
        let values = as_string_array(&result).unwrap();
        assert_eq!(values.value(0), "A");
        assert_eq!(values.value(1), "😀");
        assert!(values.is_null(2));

        let codepoint = Expr::Function {
            name: "CODEPOINT".into(),
            args: vec![Expr::Column("text".into())],
        };
        let result = evaluate(&codepoint, &batch).unwrap();
        let values = result.as_primitive::<Int64Type>();
        assert_eq!(values.value(0), 0x41);
        assert_eq!(values.value(1), 0x1F600);
        assert!(values.is_null(2));

        let invalid = Expr::Function {
            name: "CODEPOINT".into(),
            args: vec![Expr::Literal(ScalarValue::Utf8("AB".into()))],
        };
        assert!(evaluate(&invalid, &batch)
            .unwrap_err()
            .to_string()
            .contains("exactly one Unicode code point"));
    }

    #[test]
    fn split_part_uses_one_based_bounds_and_null_semantics() {
        let batch = RecordBatch::try_new(
            Arc::new(Schema::new(vec![Field::new("name", DataType::Utf8, true)])),
            vec![Arc::new(StringArray::from(vec![
                Some("Hello world"),
                Some("World"),
                None,
            ]))],
        )
        .unwrap();
        let expr = Expr::Function {
            name: "SPLIT_PART".into(),
            args: vec![
                Expr::Column("name".into()),
                Expr::Literal(ScalarValue::Utf8(" ".into())),
                Expr::Literal(ScalarValue::Int64(2)),
            ],
        };
        let result = evaluate(&expr, &batch).unwrap();
        let values = as_string_array(&result).unwrap();
        assert_eq!(values.value(0), "world");
        assert!(values.is_null(1));
        assert!(values.is_null(2));

        let out_of_range = Expr::Function {
            name: "SPLIT_PART".into(),
            args: vec![
                Expr::Column("name".into()),
                Expr::Literal(ScalarValue::Utf8(" ".into())),
                Expr::Literal(ScalarValue::Int64(3)),
            ],
        };
        let out_of_range_result = evaluate(&out_of_range, &batch).unwrap();
        let values = as_string_array(&out_of_range_result).unwrap();
        assert!(values.is_null(0));

        let invalid = Expr::Function {
            name: "SPLIT_PART".into(),
            args: vec![
                Expr::Column("name".into()),
                Expr::Literal(ScalarValue::Utf8(" ".into())),
                Expr::Literal(ScalarValue::Int64(0)),
            ],
        };
        assert!(evaluate(&invalid, &batch)
            .unwrap_err()
            .to_string()
            .contains("index must be greater than zero"));
    }

    #[test]
    fn strpos_supports_positive_and_negative_instances_with_unicode_positions() {
        let batch = RecordBatch::try_new(
            Arc::new(Schema::new(vec![Field::new("value", DataType::Utf8, true)])),
            vec![Arc::new(StringArray::from(vec![
                Some("é-é-é"),
                Some("abc"),
                None,
            ]))],
        )
        .unwrap();
        let expr = |instance| Expr::Function {
            name: "STRPOS".into(),
            args: vec![
                Expr::Column("value".into()),
                Expr::Literal(ScalarValue::Utf8("é".into())),
                Expr::Literal(ScalarValue::Int64(instance)),
            ],
        };
        let result = evaluate(&expr(2), &batch).unwrap();
        assert_eq!(result.as_primitive::<Int64Type>().value(0), 3);
        let result = evaluate(&expr(-1), &batch).unwrap();
        assert_eq!(result.as_primitive::<Int64Type>().value(0), 5);
        let result = evaluate(&expr(4), &batch).unwrap();
        assert_eq!(result.as_primitive::<Int64Type>().value(0), 0);
        assert!(evaluate(&expr(0), &batch)
            .unwrap_err()
            .to_string()
            .contains("instance must not be zero"));
    }

    #[test]
    fn split_returns_typed_lists_and_preserves_null_inputs() {
        let batch = RecordBatch::try_new(
            Arc::new(Schema::new(vec![
                Field::new("value", DataType::Utf8, true),
                Field::new("delimiter", DataType::Utf8, true),
            ])),
            vec![
                Arc::new(StringArray::from(vec![
                    Some("alpha,beta"),
                    Some("abc"),
                    None,
                ])),
                Arc::new(StringArray::from(vec![Some(","), Some(""), Some(",")])),
            ],
        )
        .unwrap();
        let expr = Expr::Function {
            name: "SPLIT".into(),
            args: vec![
                Expr::Column("value".into()),
                Expr::Column("delimiter".into()),
            ],
        };
        let result = evaluate(&expr, &batch).unwrap();
        let values = result.as_any().downcast_ref::<ListArray>().unwrap();
        let first = values.value(0);
        assert_eq!(
            first
                .as_any()
                .downcast_ref::<StringArray>()
                .unwrap()
                .iter()
                .collect::<Vec<_>>(),
            vec![Some("alpha"), Some("beta")]
        );
        let second = values.value(1);
        assert_eq!(
            second
                .as_any()
                .downcast_ref::<StringArray>()
                .unwrap()
                .iter()
                .collect::<Vec<_>>(),
            vec![Some("a"), Some("b"), Some("c")]
        );
        assert!(values.is_null(2));

        let limited = Expr::Function {
            name: "SPLIT".into(),
            args: vec![
                Expr::Literal(ScalarValue::Utf8("a,b,c".into())),
                Expr::Literal(ScalarValue::Utf8(",".into())),
                Expr::Literal(ScalarValue::Int64(2)),
            ],
        };
        let limited = evaluate(&limited, &batch).unwrap();
        let limited = limited.as_any().downcast_ref::<ListArray>().unwrap();
        let limited = limited.value(0);
        assert_eq!(
            limited
                .as_any()
                .downcast_ref::<StringArray>()
                .unwrap()
                .iter()
                .collect::<Vec<_>>(),
            vec![Some("a"), Some("b,c")]
        );

        let invalid_limit = Expr::Function {
            name: "SPLIT".into(),
            args: vec![
                Expr::Literal(ScalarValue::Utf8("a,b".into())),
                Expr::Literal(ScalarValue::Utf8(",".into())),
                Expr::Literal(ScalarValue::Int64(0)),
            ],
        };
        assert!(evaluate(&invalid_limit, &batch)
            .unwrap_err()
            .to_string()
            .contains("limit must be greater than zero"));
    }

    #[test]
    fn concat_ws_skips_null_values_and_propagates_null_separator() {
        let batch = RecordBatch::try_new(
            Arc::new(Schema::new(vec![
                Field::new("a", DataType::Utf8, true),
                Field::new("b", DataType::Utf8, true),
                Field::new("separator", DataType::Utf8, true),
            ])),
            vec![
                Arc::new(StringArray::from(vec![Some("a"), None, Some("c")])),
                Arc::new(StringArray::from(vec![Some("b"), Some("b"), None])),
                Arc::new(StringArray::from(vec![Some("-"), Some("-"), None])),
            ],
        )
        .unwrap();
        let expr = Expr::Function {
            name: "CONCAT_WS".into(),
            args: vec![
                Expr::Column("separator".into()),
                Expr::Column("a".into()),
                Expr::Column("b".into()),
            ],
        };
        let result = evaluate(&expr, &batch).unwrap();
        let values = as_string_array(&result).unwrap();
        assert_eq!(values.value(0), "a-b");
        assert_eq!(values.value(1), "b");
        assert!(values.is_null(2));
    }

    #[test]
    fn translate_maps_unicode_characters_and_removes_unmapped_targets() {
        let batch = RecordBatch::try_new(
            Arc::new(Schema::new(vec![Field::new("value", DataType::Utf8, true)])),
            vec![Arc::new(StringArray::from(vec![
                Some("aébc"),
                Some("aabc"),
                None,
            ]))],
        )
        .unwrap();
        let expr = Expr::Function {
            name: "TRANSLATE".into(),
            args: vec![
                Expr::Column("value".into()),
                Expr::Literal(ScalarValue::Utf8("aéb".into())),
                Expr::Literal(ScalarValue::Utf8("XY".into())),
            ],
        };
        let result = evaluate(&expr, &batch).unwrap();
        let values = as_string_array(&result).unwrap();
        assert_eq!(values.value(0), "XYc");
        assert_eq!(values.value(1), "XXc");
        assert!(values.is_null(2));
    }

    #[test]
    fn evaluates_coalesce() {
        let batch = RecordBatch::try_new(
            Arc::new(Schema::new(vec![
                Field::new("a", DataType::Int64, true),
                Field::new("b", DataType::Int64, true),
            ])),
            vec![
                Arc::new(Int64Array::from(vec![None, Some(2), None])),
                Arc::new(Int64Array::from(vec![Some(10), Some(20), None])),
            ],
        )
        .unwrap();
        let expr = Expr::Function {
            name: "COALESCE".into(),
            args: vec![Expr::Column("a".into()), Expr::Column("b".into())],
        };
        let result = evaluate(&expr, &batch).unwrap();
        let arr = result.as_primitive::<Int64Type>();
        assert_eq!(arr.value(0), 10);
        assert_eq!(arr.value(1), 2);
        assert!(arr.is_null(2));
    }

    #[test]
    fn greatest_and_least_preserve_type_and_nulls() {
        let batch = RecordBatch::try_new(
            Arc::new(Schema::new(vec![
                Field::new("a", DataType::Int64, true),
                Field::new("b", DataType::Int64, true),
            ])),
            vec![
                Arc::new(Int64Array::from(vec![Some(3), Some(2), None])),
                Arc::new(Int64Array::from(vec![Some(1), None, Some(4)])),
            ],
        )
        .unwrap();
        let greatest = Expr::Function {
            name: "GREATEST".into(),
            args: vec![Expr::Column("a".into()), Expr::Column("b".into())],
        };
        let least = Expr::Function {
            name: "LEAST".into(),
            args: vec![Expr::Column("a".into()), Expr::Column("b".into())],
        };
        let greatest = evaluate(&greatest, &batch).unwrap();
        let least = evaluate(&least, &batch).unwrap();
        assert_eq!(greatest.data_type(), &DataType::Int64);
        assert_eq!(least.data_type(), &DataType::Int64);
        let greatest = greatest.as_primitive::<Int64Type>();
        let least = least.as_primitive::<Int64Type>();
        assert_eq!(greatest.value(0), 3);
        assert!(greatest.is_null(1));
        assert!(greatest.is_null(2));
        assert_eq!(least.value(0), 1);
        assert!(least.is_null(1));
        assert!(least.is_null(2));

        let text_batch = RecordBatch::try_new(
            Arc::new(Schema::new(vec![
                Field::new("left_text", DataType::Utf8, true),
                Field::new("right_text", DataType::Utf8, true),
            ])),
            vec![
                Arc::new(StringArray::from(vec![Some("z"), Some("a")])),
                Arc::new(StringArray::from(vec![Some("b"), Some("c")])),
            ],
        )
        .unwrap();
        let text = Expr::Function {
            name: "GREATEST".into(),
            args: vec![
                Expr::Column("left_text".into()),
                Expr::Column("right_text".into()),
            ],
        };
        let text = evaluate(&text, &text_batch).unwrap();
        assert_eq!(text.data_type(), &DataType::Utf8);
        let text = text.as_any().downcast_ref::<StringArray>().unwrap();
        assert_eq!(text.value(0), "z");
        assert_eq!(text.value(1), "c");
    }

    #[test]
    fn evaluates_string_concat_operator() {
        let batch = RecordBatch::try_new(
            Arc::new(Schema::new(vec![
                Field::new("a", DataType::Utf8, false),
                Field::new("b", DataType::Utf8, false),
            ])),
            vec![
                Arc::new(StringArray::from(vec!["hello", "foo"])),
                Arc::new(StringArray::from(vec![" world", "bar"])),
            ],
        )
        .unwrap();
        let expr = Expr::BinaryOp {
            left: Box::new(Expr::Column("a".into())),
            op: BinaryOp::StringConcat,
            right: Box::new(Expr::Column("b".into())),
        };
        let result = evaluate(&expr, &batch).unwrap();
        let arr = as_string_array(&result).unwrap();
        assert_eq!(arr.value(0), "hello world");
        assert_eq!(arr.value(1), "foobar");
    }

    #[test]
    fn evaluates_cast_to_date32() {
        let batch = RecordBatch::try_new(
            Arc::new(Schema::new(vec![Field::new("day", DataType::Int32, true)])),
            vec![Arc::new(Int32Array::from(vec![Some(20_000), None]))],
        )
        .unwrap();
        let expr = Expr::Cast {
            expr: Box::new(Expr::Column("day".into())),
            data_type: CastTarget::Date32,
        };
        let result = evaluate(&expr, &batch).unwrap();
        let dates = result.as_primitive::<arrow::datatypes::Date32Type>();
        assert_eq!(dates.value(0), 20_000);
        assert!(dates.is_null(1));
    }

    #[test]
    fn evaluates_cast_to_timestamp_micros() {
        let batch = RecordBatch::try_new(
            Arc::new(Schema::new(vec![Field::new(
                "micros",
                DataType::Int64,
                true,
            )])),
            vec![Arc::new(Int64Array::from(vec![Some(20_000), None]))],
        )
        .unwrap();
        let expr = Expr::Cast {
            expr: Box::new(Expr::Column("micros".into())),
            data_type: CastTarget::TimestampMicros {
                with_timezone: false,
            },
        };
        let result = evaluate(&expr, &batch).unwrap();
        let timestamps = result.as_primitive::<arrow::datatypes::TimestampMicrosecondType>();
        assert_eq!(timestamps.value(0), 20_000);
        assert!(timestamps.is_null(1));

        let literal_batch = RecordBatch::try_from_iter([(
            "row",
            Arc::new(Int64Array::from(vec![1])) as ArrayRef,
        )])
        .unwrap();
        let result = evaluate(
            &Expr::Cast {
                expr: Box::new(Expr::Literal(ScalarValue::Utf8(
                    "2026-10-13 12:34:56.789".into(),
                ))),
                data_type: CastTarget::TimestampMicros {
                    with_timezone: false,
                },
            },
            &literal_batch,
        )
        .unwrap();
        assert_eq!(result.len(), 1);
        assert_eq!(result.data_type(), &DataType::Timestamp(TimeUnit::Microsecond, None));

        let result = evaluate(
            &Expr::Cast {
                expr: Box::new(Expr::Literal(ScalarValue::Utf8(
                    "2026-10-13 12:34:56Z".into(),
                ))),
                data_type: CastTarget::TimestampMicros {
                    with_timezone: true,
                },
            },
            &literal_batch,
        )
        .unwrap();
        assert_eq!(
            result.data_type(),
            &DataType::Timestamp(TimeUnit::Microsecond, Some("UTC".into()))
        );
        let utc_value = result
            .as_primitive::<TimestampMicrosecondType>()
            .value(0);

        // Offset-bearing input is accepted by the same timestamp parser and
        // retains the timezone-bearing output type.
        let result = evaluate(
            &Expr::Cast {
                expr: Box::new(Expr::Literal(ScalarValue::Utf8(
                    "2026-10-13 12:34:56+05:30".into(),
                ))),
                data_type: CastTarget::TimestampMicros {
                    with_timezone: true,
                },
            },
            &literal_batch,
        )
        .unwrap();
        assert_eq!(
            result.data_type(),
            &DataType::Timestamp(TimeUnit::Microsecond, Some("UTC".into()))
        );
        assert_eq!(
            result
                .as_primitive::<TimestampMicrosecondType>()
                .value(0),
            utc_value - 19_800_000_000
        );
    }

    #[test]
    fn evaluates_cast_to_decimal128() {
        let batch = RecordBatch::try_new(
            Arc::new(Schema::new(vec![Field::new(
                "amount",
                DataType::Int64,
                true,
            )])),
            vec![Arc::new(Int64Array::from(vec![Some(12), None]))],
        )
        .unwrap();
        let expr = Expr::Cast {
            expr: Box::new(Expr::Column("amount".into())),
            data_type: CastTarget::Decimal128 {
                precision: 10,
                scale: 2,
            },
        };
        let result = evaluate(&expr, &batch).unwrap();
        let decimals = result.as_primitive::<arrow::datatypes::Decimal128Type>();
        assert_eq!(decimals.value(0), 1_200);
        assert_eq!(decimals.value(1), 0);
        assert!(decimals.is_null(1));
        assert_eq!(result.data_type(), &DataType::Decimal128(10, 2));
    }

    #[test]
    fn evaluates_numeric_array_literal_as_float64_list() {
        let batch = RecordBatch::try_from_iter([(
            "id",
            Arc::new(Int64Array::from(vec![1, 2])) as ArrayRef,
        )])
        .unwrap();
        let expr = Expr::Function {
            name: "ARRAY".into(),
            args: vec![
                Expr::Literal(ScalarValue::Int64(1)),
                Expr::Literal(ScalarValue::Float64(2.5)),
            ],
        };
        let result = evaluate(&expr, &batch).unwrap();
        assert_eq!(
            result.data_type(),
            &DataType::List(Arc::new(Field::new("item", DataType::Float64, true)))
        );
        let lists = result.as_list::<i32>();
        assert_eq!(
            lists.value(0).as_primitive::<Float64Type>().values(),
            &[1.0, 2.5]
        );
        assert_eq!(
            lists.value(1).as_primitive::<Float64Type>().values(),
            &[1.0, 2.5]
        );
    }

    #[test]
    fn evaluates_homogeneous_integer_array_literal_as_int64_list() {
        let batch = RecordBatch::try_from_iter([(
            "id",
            Arc::new(Int64Array::from(vec![1, 2])) as ArrayRef,
        )])
        .unwrap();
        let expr = Expr::Function {
            name: "ARRAY".into(),
            args: vec![
                Expr::Literal(ScalarValue::Int64(1)),
                Expr::Literal(ScalarValue::Null),
                Expr::Literal(ScalarValue::Int64(3)),
            ],
        };
        let result = evaluate(&expr, &batch).unwrap();
        assert_eq!(
            result.data_type(),
            &DataType::List(Arc::new(Field::new("item", DataType::Int64, true)))
        );
        let lists = result.as_list::<i32>();
        let inner = lists.value(0);
        let values = inner.as_primitive::<Int64Type>();
        assert_eq!(values.iter().collect::<Vec<_>>(), vec![Some(1), None, Some(3)]);
    }

    #[test]
    fn evaluates_int32_array_elements() {
        let batch = RecordBatch::try_from_iter([(
            "value",
            Arc::new(Int32Array::from(vec![Some(7), None])) as ArrayRef,
        )])
        .unwrap();
        let result = evaluate(
            &Expr::Function {
                name: "ARRAY".into(),
                args: vec![Expr::Column("value".into())],
            },
            &batch,
        )
        .unwrap();
        let lists = result.as_list::<i32>();
        assert_eq!(lists.value(0).as_primitive::<Int32Type>().iter().collect::<Vec<_>>(), vec![Some(7)]);
        assert_eq!(lists.value(1).as_primitive::<Int32Type>().iter().collect::<Vec<_>>(), vec![None]);
    }

    #[test]
    fn evaluates_unsigned_integer_array_elements() {
        let batch = RecordBatch::try_from_iter([
            (
                "u32",
                Arc::new(UInt32Array::from(vec![Some(7), None])) as ArrayRef,
            ),
            (
                "u64",
                Arc::new(UInt64Array::from(vec![Some(9), None])) as ArrayRef,
            ),
        ])
        .unwrap();
        for (column, data_type) in [("u32", DataType::UInt32), ("u64", DataType::UInt64)] {
            let result = evaluate(
                &Expr::Function {
                    name: "ARRAY".into(),
                    args: vec![Expr::Column(column.into())],
                },
                &batch,
            )
            .unwrap();
            assert_eq!(result.data_type(), &DataType::List(Arc::new(Field::new(
                "item",
                data_type,
                true,
            ))));
            assert_eq!(result.as_list::<i32>().value(0).len(), 1);
            assert!(result.as_list::<i32>().value(1).is_null(0));
        }
    }

    #[test]
    fn evaluates_float32_array_elements() {
        let batch = RecordBatch::try_from_iter([(
            "value",
            Arc::new(Float32Array::from(vec![Some(1.25), None])) as ArrayRef,
        )])
        .unwrap();
        let result = evaluate(
            &Expr::Function {
                name: "ARRAY".into(),
                args: vec![Expr::Column("value".into())],
            },
            &batch,
        )
        .unwrap();
        assert_eq!(
            result.data_type(),
            &DataType::List(Arc::new(Field::new("item", DataType::Float32, true)))
        );
        let values = result.as_list::<i32>();
        assert_eq!(values.value(0).as_primitive::<Float32Type>().value(0), 1.25);
        assert!(values.value(1).is_null(0));
    }

    #[test]
    fn evaluates_narrow_integer_array_elements() {
        let batch = RecordBatch::try_from_iter([
            (
                "i8",
                Arc::new(Int8Array::from(vec![Some(-3), None])) as ArrayRef,
            ),
            (
                "i16",
                Arc::new(Int16Array::from(vec![Some(-300), None])) as ArrayRef,
            ),
            (
                "u8",
                Arc::new(UInt8Array::from(vec![Some(3), None])) as ArrayRef,
            ),
            (
                "u16",
                Arc::new(UInt16Array::from(vec![Some(300), None])) as ArrayRef,
            ),
        ])
        .unwrap();
        for (column, data_type) in [
            ("i8", DataType::Int8),
            ("i16", DataType::Int16),
            ("u8", DataType::UInt8),
            ("u16", DataType::UInt16),
        ] {
            let result = evaluate(
                &Expr::Function {
                    name: "ARRAY".into(),
                    args: vec![Expr::Column(column.into())],
                },
                &batch,
            )
            .unwrap();
            let lists = result.as_list::<i32>();
            assert_eq!(lists.value_type(), data_type);
            assert!(lists.value(1).is_null(0));
        }
    }

    #[test]
    fn evaluates_time64_array_elements() {
        let batch = RecordBatch::try_from_iter([
            (
                "micros",
                Arc::new(Time64MicrosecondArray::from(vec![Some(42), None])) as ArrayRef,
            ),
            (
                "nanos",
                Arc::new(Time64NanosecondArray::from(vec![Some(84), None])) as ArrayRef,
            ),
        ])
        .unwrap();
        for (column, data_type) in [
            ("micros", DataType::Time64(TimeUnit::Microsecond)),
            ("nanos", DataType::Time64(TimeUnit::Nanosecond)),
        ] {
            let result = evaluate(
                &Expr::Function {
                    name: "ARRAY".into(),
                    args: vec![Expr::Column(column.into())],
                },
                &batch,
            )
            .unwrap();
            let lists = result.as_list::<i32>();
            assert_eq!(lists.value_type(), data_type);
            assert!(lists.value(1).is_null(0));
        }
    }

    #[test]
    fn evaluates_date32_array_elements() {
        let batch = RecordBatch::try_from_iter([(
            "day",
            Arc::new(arrow::array::Date32Array::from(vec![Some(10), None])) as ArrayRef,
        )])
        .unwrap();
        let result = evaluate(
            &Expr::Function {
                name: "ARRAY".into(),
                args: vec![Expr::Column("day".into())],
            },
            &batch,
        )
        .unwrap();
        let lists = result.as_list::<i32>();
        assert_eq!(lists.value(0).as_primitive::<arrow::datatypes::Date32Type>().iter().collect::<Vec<_>>(), vec![Some(10)]);
        assert_eq!(lists.value(1).as_primitive::<arrow::datatypes::Date32Type>().iter().collect::<Vec<_>>(), vec![None]);
    }

    #[test]
    fn evaluates_date64_array_elements() {
        let batch = RecordBatch::try_from_iter([(
            "day",
            Arc::new(arrow::array::Date64Array::from(vec![Some(86_400_000), None]))
                as ArrayRef,
        )])
        .unwrap();
        let result = evaluate(
            &Expr::Function {
                name: "ARRAY".into(),
                args: vec![Expr::Column("day".into())],
            },
            &batch,
        )
        .unwrap();
        let lists = result.as_list::<i32>();
        assert_eq!(lists.value(0).as_primitive::<arrow::datatypes::Date64Type>().iter().collect::<Vec<_>>(), vec![Some(86_400_000)]);
        assert_eq!(lists.value(1).as_primitive::<arrow::datatypes::Date64Type>().iter().collect::<Vec<_>>(), vec![None]);
    }

    #[test]
    fn evaluates_time32_array_elements() {
        let batch = RecordBatch::try_from_iter([(
            "time",
            Arc::new(arrow::array::Time32SecondArray::from(vec![Some(42), None]))
                as ArrayRef,
        )])
        .unwrap();
        let result = evaluate(
            &Expr::Function {
                name: "ARRAY".into(),
                args: vec![Expr::Column("time".into())],
            },
            &batch,
        )
        .unwrap();
        let lists = result.as_list::<i32>();
        assert_eq!(lists.value(0).as_primitive::<Time32SecondType>().iter().collect::<Vec<_>>(), vec![Some(42)]);
        assert_eq!(lists.value(1).as_primitive::<Time32SecondType>().iter().collect::<Vec<_>>(), vec![None]);
    }

    #[test]
    fn evaluates_timestamp_array_elements() {
        let batch = RecordBatch::try_from_iter([(
            "ts",
            Arc::new(arrow::array::TimestampMicrosecondArray::from(vec![
                Some(1_000_000),
                None,
            ])) as ArrayRef,
        )])
        .unwrap();
        let result = evaluate(
            &Expr::Function {
                name: "ARRAY".into(),
                args: vec![Expr::Column("ts".into())],
            },
            &batch,
        )
        .unwrap();
        let lists = result.as_list::<i32>();
        assert_eq!(
            lists
                .value(0)
                .as_primitive::<arrow::datatypes::TimestampMicrosecondType>()
                .iter()
                .collect::<Vec<_>>(),
            vec![Some(1_000_000)]
        );
        assert_eq!(
            lists
                .value(1)
                .as_primitive::<arrow::datatypes::TimestampMicrosecondType>()
                .iter()
                .collect::<Vec<_>>(),
            vec![None]
        );
    }

    #[test]
    fn concatenates_temporal_array_elements_without_coercion() {
        let batch = RecordBatch::try_from_iter([
            (
                "left_day",
                Arc::new(arrow::array::Date32Array::from(vec![Some(10)])) as ArrayRef,
            ),
            (
                "right_day",
                Arc::new(arrow::array::Date32Array::from(vec![Some(20)])) as ArrayRef,
            ),
        ])
        .unwrap();
        let array = |column: &str| Expr::Function {
            name: "ARRAY".into(),
            args: vec![Expr::Column(column.into())],
        };
        let result = evaluate(
            &Expr::Function {
                name: "ARRAY_CONCAT".into(),
                args: vec![array("left_day"), array("right_day")],
            },
            &batch,
        )
        .unwrap();
        let values = result
            .as_any()
            .downcast_ref::<ListArray>()
            .unwrap()
            .value(0);
        let values = values.as_primitive::<arrow::datatypes::Date32Type>();
        assert_eq!(values.iter().collect::<Vec<_>>(), vec![Some(10), Some(20)]);
    }

    #[test]
    fn appends_and_removes_temporal_array_elements_without_coercion() {
        let batch = RecordBatch::try_from_iter([
            (
                "day",
                Arc::new(arrow::array::Date32Array::from(vec![Some(10)])) as ArrayRef,
            ),
            (
                "later_day",
                Arc::new(arrow::array::Date32Array::from(vec![Some(20)])) as ArrayRef,
            ),
        ])
        .unwrap();
        let array = Expr::Function {
            name: "ARRAY".into(),
            args: vec![Expr::Column("day".into())],
        };
        let append = evaluate(
            &Expr::Function {
                name: "ARRAY_APPEND".into(),
                args: vec![array.clone(), Expr::Column("day".into())],
            },
            &batch,
        )
        .unwrap();
        let values = append
            .as_any()
            .downcast_ref::<ListArray>()
            .unwrap()
            .value(0)
            .as_primitive::<arrow::datatypes::Date32Type>()
            .iter()
            .collect::<Vec<_>>();
        assert_eq!(values, vec![Some(10), Some(10)]);

        let remove = evaluate(
            &Expr::Function {
                name: "ARRAY_REMOVE".into(),
                args: vec![array, Expr::Column("day".into())],
            },
            &batch,
        )
        .unwrap();
        let values = remove
            .as_any()
            .downcast_ref::<ListArray>()
            .unwrap()
            .value(0);
        assert_eq!(values.len(), 0);
        assert_eq!(values.data_type(), &DataType::Date32);
    }

    #[test]
    fn finds_temporal_array_extrema_without_coercion() {
        let batch = RecordBatch::try_from_iter([
            (
                "day",
                Arc::new(arrow::array::Date32Array::from(vec![Some(10)])) as ArrayRef,
            ),
            (
                "later_day",
                Arc::new(arrow::array::Date32Array::from(vec![Some(20)])) as ArrayRef,
            ),
        ])
        .unwrap();
        let array = Expr::Function {
            name: "ARRAY".into(),
            args: vec![
                Expr::Column("day".into()),
                Expr::Column("later_day".into()),
            ],
        };
        for (name, expected) in [("ARRAY_MIN", 10), ("ARRAY_MAX", 20)] {
            let result = evaluate(
                &Expr::Function {
                    name: name.into(),
                    args: vec![array.clone()],
                },
                &batch,
            )
            .unwrap();
            assert_eq!(result.data_type(), &DataType::Date32);
            assert_eq!(result.as_primitive::<arrow::datatypes::Date32Type>().value(0), expected);
        }
    }

    #[test]
    fn reverses_temporal_array_elements_without_coercion() {
        let batch = RecordBatch::try_from_iter([
            (
                "first_day",
                Arc::new(arrow::array::Date32Array::from(vec![Some(10)])) as ArrayRef,
            ),
            (
                "second_day",
                Arc::new(arrow::array::Date32Array::from(vec![Some(20)])) as ArrayRef,
            ),
        ])
        .unwrap();
        let result = evaluate(
            &Expr::Function {
                name: "ARRAY_REVERSE".into(),
                args: vec![Expr::Function {
                    name: "ARRAY".into(),
                    args: vec![
                        Expr::Column("first_day".into()),
                        Expr::Column("second_day".into()),
                    ],
                }],
            },
            &batch,
        )
        .unwrap();
        let inner = result
            .as_any()
            .downcast_ref::<ListArray>()
            .unwrap()
            .value(0);
        let values = inner.as_primitive::<arrow::datatypes::Date32Type>();
        assert_eq!(values.iter().collect::<Vec<_>>(), vec![Some(20), Some(10)]);
    }

    #[test]
    fn sorts_temporal_array_elements_with_nulls_last() {
        let batch = RecordBatch::try_from_iter([
            (
                "first_day",
                Arc::new(arrow::array::Date32Array::from(vec![Some(20)])) as ArrayRef,
            ),
            (
                "second_day",
                Arc::new(arrow::array::Date32Array::from(vec![Some(10)])) as ArrayRef,
            ),
        ])
        .unwrap();
        let result = evaluate(
            &Expr::Function {
                name: "ARRAY_SORT".into(),
                args: vec![Expr::Function {
                    name: "ARRAY".into(),
                    args: vec![
                        Expr::Column("first_day".into()),
                        Expr::Literal(ScalarValue::Null),
                        Expr::Column("second_day".into()),
                    ],
                }],
            },
            &batch,
        )
        .unwrap();
        let inner = result
            .as_any()
            .downcast_ref::<ListArray>()
            .unwrap()
            .value(0);
        let values = inner.as_primitive::<arrow::datatypes::Date32Type>();
        assert_eq!(values.iter().collect::<Vec<_>>(), vec![Some(10), Some(20), None]);
    }

    #[test]
    fn slices_temporal_array_elements_with_stride() {
        let batch = RecordBatch::try_from_iter([
            (
                "first_day",
                Arc::new(arrow::array::Date32Array::from(vec![Some(10)])) as ArrayRef,
            ),
            (
                "second_day",
                Arc::new(arrow::array::Date32Array::from(vec![Some(20)])) as ArrayRef,
            ),
            (
                "third_day",
                Arc::new(arrow::array::Date32Array::from(vec![Some(30)])) as ArrayRef,
            ),
        ])
        .unwrap();
        let result = evaluate(
            &Expr::Function {
                name: "ARRAY_SLICE".into(),
                args: vec![
                    Expr::Function {
                        name: "ARRAY".into(),
                        args: vec![
                            Expr::Column("first_day".into()),
                            Expr::Column("second_day".into()),
                            Expr::Column("third_day".into()),
                        ],
                    },
                    Expr::Literal(ScalarValue::Int64(1)),
                    Expr::Literal(ScalarValue::Int64(3)),
                    Expr::Literal(ScalarValue::Int64(2)),
                ],
            },
            &batch,
        )
        .unwrap();
        let inner = result
            .as_any()
            .downcast_ref::<ListArray>()
            .unwrap()
            .value(0);
        let values = inner.as_primitive::<arrow::datatypes::Date32Type>();
        assert_eq!(values.iter().collect::<Vec<_>>(), vec![Some(10), Some(30)]);
    }

    #[test]
    fn evaluates_cardinality_for_array_lists() {
        let batch = RecordBatch::try_from_iter([(
            "id",
            Arc::new(Int64Array::from(vec![1, 2])) as ArrayRef,
        )])
        .unwrap();
        let expr = Expr::Function {
            name: "CARDINALITY".into(),
            args: vec![Expr::Function {
                name: "ARRAY".into(),
                args: vec![
                    Expr::Literal(ScalarValue::Int64(1)),
                    Expr::Literal(ScalarValue::Float64(2.5)),
                ],
            }],
        };
        let result = evaluate(&expr, &batch).unwrap();
        assert_eq!(result.as_primitive::<Int64Type>().values(), &[2, 2]);
    }

    #[test]
    fn evaluates_contains_for_numeric_array_lists() {
        let batch = RecordBatch::try_from_iter([(
            "id",
            Arc::new(Int64Array::from(vec![1, 2, 3])) as ArrayRef,
        )])
        .unwrap();
        let expr = Expr::Function {
            name: "CONTAINS".into(),
            args: vec![
                Expr::Function {
                    name: "ARRAY".into(),
                    args: vec![
                        Expr::Literal(ScalarValue::Int64(1)),
                        Expr::Literal(ScalarValue::Float64(2.5)),
                    ],
                },
                Expr::Literal(ScalarValue::Float64(2.5)),
            ],
        };
        let result = evaluate(&expr, &batch).unwrap();
        assert_eq!(
            result.as_boolean().iter().collect::<Vec<_>>(),
            vec![Some(true), Some(true), Some(true)]
        );
    }

    #[test]
    fn evaluates_contains_for_string_array_lists() {
        let batch = RecordBatch::try_from_iter([(
            "id",
            Arc::new(Int64Array::from(vec![1, 2])) as ArrayRef,
        )])
        .unwrap();
        let expr = Expr::Function {
            name: "CONTAINS".into(),
            args: vec![
                Expr::Function {
                    name: "ARRAY".into(),
                    args: vec![
                        Expr::Literal(ScalarValue::Utf8("red".into())),
                        Expr::Literal(ScalarValue::Utf8("blue".into())),
                    ],
                },
                Expr::Literal(ScalarValue::Utf8("blue".into())),
            ],
        };
        let result = evaluate(&expr, &batch).unwrap();
        assert_eq!(
            result.as_boolean().iter().collect::<Vec<_>>(),
            vec![Some(true), Some(true)]
        );
    }

    #[test]
    fn evaluates_contains_for_temporal_array_lists() {
        let batch = RecordBatch::try_from_iter([
            (
                "first_day",
                Arc::new(arrow::array::Date32Array::from(vec![Some(10)])) as ArrayRef,
            ),
            (
                "second_day",
                Arc::new(arrow::array::Date32Array::from(vec![Some(20)])) as ArrayRef,
            ),
        ])
        .unwrap();
        let result = evaluate(
            &Expr::Function {
                name: "CONTAINS".into(),
                args: vec![
                    Expr::Function {
                        name: "ARRAY".into(),
                        args: vec![
                            Expr::Column("first_day".into()),
                            Expr::Column("second_day".into()),
                        ],
                    },
                    Expr::Column("second_day".into()),
                ],
            },
            &batch,
        )
        .unwrap();
        assert_eq!(result.as_boolean().iter().collect::<Vec<_>>(), vec![Some(true)]);
    }

    #[test]
    fn evaluates_array_position_for_numeric_and_string_lists() {
        let batch = RecordBatch::try_from_iter([(
            "id",
            Arc::new(Int64Array::from(vec![1, 2])) as ArrayRef,
        )])
        .unwrap();
        let numeric = Expr::Function {
            name: "ARRAY_POSITION".into(),
            args: vec![
                Expr::Function {
                    name: "ARRAY".into(),
                    args: vec![
                        Expr::Literal(ScalarValue::Int64(4)),
                        Expr::Literal(ScalarValue::Float64(2.5)),
                        Expr::Literal(ScalarValue::Int64(4)),
                    ],
                },
                Expr::Literal(ScalarValue::Float64(2.5)),
            ],
        };
        let numeric_result = evaluate(&numeric, &batch).unwrap();
        assert_eq!(
            numeric_result
                .as_primitive::<Int64Type>()
                .iter()
                .collect::<Vec<_>>(),
            vec![Some(2), Some(2)]
        );

        let string = Expr::Function {
            name: "ARRAY_POSITION".into(),
            args: vec![
                Expr::Function {
                    name: "ARRAY".into(),
                    args: vec![
                        Expr::Literal(ScalarValue::Utf8("red".into())),
                        Expr::Literal(ScalarValue::Utf8("blue".into())),
                    ],
                },
                Expr::Literal(ScalarValue::Utf8("green".into())),
            ],
        };
        let string_result = evaluate(&string, &batch).unwrap();
        assert_eq!(
            string_result
                .as_primitive::<Int64Type>()
                .iter()
                .collect::<Vec<_>>(),
            vec![Some(0), Some(0)]
        );
    }

    #[test]
    fn evaluates_array_position_for_temporal_lists() {
        let batch = RecordBatch::try_from_iter([
            (
                "first_day",
                Arc::new(arrow::array::Date32Array::from(vec![Some(10)])) as ArrayRef,
            ),
            (
                "second_day",
                Arc::new(arrow::array::Date32Array::from(vec![Some(20)])) as ArrayRef,
            ),
        ])
        .unwrap();
        let result = evaluate(
            &Expr::Function {
                name: "ARRAY_POSITION".into(),
                args: vec![
                    Expr::Function {
                        name: "ARRAY".into(),
                        args: vec![
                            Expr::Column("first_day".into()),
                            Expr::Column("second_day".into()),
                        ],
                    },
                    Expr::Column("second_day".into()),
                ],
            },
            &batch,
        )
        .unwrap();
        assert_eq!(result.as_primitive::<Int64Type>().iter().collect::<Vec<_>>(), vec![Some(2)]);
    }

    #[test]
    fn evaluates_arrays_overlap_for_temporal_lists() {
        let batch = RecordBatch::try_from_iter([
            (
                "left_day",
                Arc::new(arrow::array::Date32Array::from(vec![Some(10)])) as ArrayRef,
            ),
            (
                "right_day",
                Arc::new(arrow::array::Date32Array::from(vec![Some(20)])) as ArrayRef,
            ),
        ])
        .unwrap();
        let result = evaluate(
            &Expr::Function {
                name: "ARRAYS_OVERLAP".into(),
                args: vec![
                    Expr::Function {
                        name: "ARRAY".into(),
                        args: vec![
                            Expr::Column("left_day".into()),
                            Expr::Column("right_day".into()),
                        ],
                    },
                    Expr::Function {
                        name: "ARRAY".into(),
                        args: vec![Expr::Column("right_day".into())],
                    },
                ],
            },
            &batch,
        )
        .unwrap();
        assert_eq!(result.as_boolean().iter().collect::<Vec<_>>(), vec![Some(true)]);
    }

    #[test]
    fn array_position_matches_null_elements() {
        let batch =
            RecordBatch::try_from_iter([("id", Arc::new(Int64Array::from(vec![1])) as ArrayRef)])
                .unwrap();
        let expr = Expr::Function {
            name: "ARRAY_POSITION".into(),
            args: vec![
                Expr::Function {
                    name: "ARRAY".into(),
                    args: vec![
                        Expr::Literal(ScalarValue::Int64(1)),
                        Expr::Literal(ScalarValue::Null),
                    ],
                },
                Expr::Literal(ScalarValue::Null),
            ],
        };
        let result = evaluate(&expr, &batch).unwrap();
        assert_eq!(
            result
                .as_primitive::<Int64Type>()
                .iter()
                .collect::<Vec<_>>(),
            vec![Some(2)]
        );
    }

    #[test]
    fn element_at_uses_one_based_and_negative_indexes() {
        let batch = RecordBatch::try_from_iter([(
            "id",
            Arc::new(Int64Array::from(vec![1, 2])) as ArrayRef,
        )])
        .unwrap();
        let array = Expr::Function {
            name: "ARRAY".into(),
            args: vec![
                Expr::Literal(ScalarValue::Int64(10)),
                Expr::Literal(ScalarValue::Int64(20)),
                Expr::Literal(ScalarValue::Int64(30)),
            ],
        };
        let first = evaluate(
            &Expr::Function {
                name: "ELEMENT_AT".into(),
                args: vec![array.clone(), Expr::Literal(ScalarValue::Int64(1))],
            },
            &batch,
        )
        .unwrap();
        assert_eq!(
            first
                .as_primitive::<Int64Type>()
                .iter()
                .collect::<Vec<_>>(),
            vec![Some(10), Some(10)]
        );
        let last = evaluate(
            &Expr::Function {
                name: "ELEMENT_AT".into(),
                args: vec![array, Expr::Literal(ScalarValue::Int64(-1))],
            },
            &batch,
        )
        .unwrap();
        assert_eq!(
            last.as_primitive::<Int64Type>()
                .iter()
                .collect::<Vec<_>>(),
            vec![Some(30), Some(30)]
        );
    }

    #[test]
    fn cardinality_rejects_non_list_values_explicitly() {
        let batch =
            RecordBatch::try_from_iter([("id", Arc::new(Int64Array::from(vec![1])) as ArrayRef)])
                .unwrap();
        let expr = Expr::Function {
            name: "CARDINALITY".into(),
            args: vec![Expr::Column("id".into())],
        };
        let error = evaluate(&expr, &batch).unwrap_err();
        assert!(
            error
                .to_string()
                .contains("CARDINALITY currently supports ARRAY/List values only")
        );
    }

    #[test]
    fn evaluates_json_extract_scalar_with_nested_keys_and_nulls() {
        let batch = RecordBatch::try_from_iter([
            (
                "document",
                Arc::new(StringArray::from(vec![
                    Some(r#"{"customer":{"id":42}}"#),
                    Some(r#"{"customer":{"id":null}}"#),
                    None,
                ])) as ArrayRef,
            ),
            (
                "path",
                Arc::new(StringArray::from(vec![
                    Some("$.customer.id"),
                    Some("$.customer.id"),
                    Some("$.customer.id"),
                ])) as ArrayRef,
            ),
        ])
        .unwrap();
        let expr = Expr::Function {
            name: "JSON_EXTRACT_SCALAR".into(),
            args: vec![Expr::Column("document".into()), Expr::Column("path".into())],
        };
        let result = evaluate(&expr, &batch).unwrap();
        let values = result.as_string::<i32>();
        assert_eq!(values.value(0), "42");
        assert!(values.is_null(1));
        assert!(values.is_null(2));
    }

    #[test]
    fn evaluates_json_extract_scalar_with_array_subscripts() {
        let batch = RecordBatch::try_from_iter([
            (
                "document",
                Arc::new(StringArray::from(vec![
                    Some(r#"{"orders":[{"id":"A-17"}]}"#),
                    Some(r#"{"orders":[]}"#),
                ])) as ArrayRef,
            ),
            (
                "path",
                Arc::new(StringArray::from(vec![
                    Some("$.orders[0].id"),
                    Some("$.orders[0].id"),
                ])) as ArrayRef,
            ),
        ])
        .unwrap();
        let expr = Expr::Function {
            name: "JSON_EXTRACT_SCALAR".into(),
            args: vec![Expr::Column("document".into()), Expr::Column("path".into())],
        };
        let result = evaluate(&expr, &batch).unwrap();
        let values = result.as_string::<i32>();
        assert_eq!(values.value(0), "A-17");
        assert!(values.is_null(1));
    }

    #[test]
    fn json_parse_validates_documents_and_try_parse_returns_null() {
        let batch = RecordBatch::try_from_iter([(
            "document",
            Arc::new(StringArray::from(vec![
                Some(r#"{"x":1}"#),
                Some("not-json"),
                None,
            ])) as ArrayRef,
        )])
        .unwrap();
        let parse = Expr::Function {
            name: "JSON_PARSE".into(),
            args: vec![Expr::Column("document".into())],
        };
        let error = evaluate(&parse, &batch).unwrap_err();
        assert!(error.to_string().contains("invalid JSON value at row 1"));

        let try_parse = Expr::Function {
            name: "TRY_JSON_PARSE".into(),
            args: vec![Expr::Column("document".into())],
        };
        let result = evaluate(&try_parse, &batch).unwrap();
        let values = result.as_string::<i32>();
        assert_eq!(values.value(0), r#"{"x":1}"#);
        assert!(values.is_null(1));
        assert!(values.is_null(2));
    }

    #[test]
    fn evaluates_json_extract_preserving_objects_arrays_and_nulls() {
        let batch = RecordBatch::try_from_iter([
            (
                "document",
                Arc::new(StringArray::from(vec![
                    Some(r#"{"customer":{"id":42,"tags":["a","b"]}}"#),
                    Some(r#"{"customer":{"id":null}}"#),
                    None,
                    Some("not-json"),
                ])) as ArrayRef,
            ),
            (
                "path",
                Arc::new(StringArray::from(vec![
                    Some("$.customer"),
                    Some("$.customer.id"),
                    Some("$.customer"),
                    Some("$.customer"),
                ])) as ArrayRef,
            ),
        ])
        .unwrap();
        let expr = Expr::Function {
            name: "JSON_EXTRACT".into(),
            args: vec![Expr::Column("document".into()), Expr::Column("path".into())],
        };
        let result = evaluate(&expr, &batch).unwrap();
        let values = result.as_string::<i32>();
        assert_eq!(values.value(0), r#"{"id":42,"tags":["a","b"]}"#);
        assert_eq!(values.value(1), "null");
        assert!(values.is_null(2));
        assert!(values.is_null(3));
    }

    #[test]
    fn evaluates_json_array_length_with_non_arrays_as_null() {
        let batch = RecordBatch::try_from_iter([(
            "document",
            Arc::new(StringArray::from(vec![
                Some(r#"[1,2,3]"#),
                Some(r#"{"a":1}"#),
                Some("invalid"),
                None,
            ])) as ArrayRef,
        )])
        .unwrap();
        let result = evaluate(
            &Expr::Function {
                name: "JSON_ARRAY_LENGTH".into(),
                args: vec![Expr::Column("document".into())],
            },
            &batch,
        )
        .unwrap();
        assert_eq!(
            result.as_primitive::<Int64Type>().iter().collect::<Vec<_>>(),
            vec![Some(3), None, None, None]
        );
    }

    #[test]
    fn evaluates_json_size_for_containers_and_scalars() {
        let batch = RecordBatch::try_from_iter([
            (
                "document",
                Arc::new(StringArray::from(vec![
                    Some(r#"{"items":[1,2],"name":"x"}"#),
                    Some(r#"{"items":1}"#),
                    Some("invalid"),
                ])) as ArrayRef,
            ),
            (
                "path",
                Arc::new(StringArray::from(vec![
                    Some("$.items"),
                    Some("$.items"),
                    Some("$.items"),
                ])) as ArrayRef,
            ),
        ])
        .unwrap();
        let result = evaluate(
            &Expr::Function {
                name: "JSON_SIZE".into(),
                args: vec![Expr::Column("document".into()), Expr::Column("path".into())],
            },
            &batch,
        )
        .unwrap();
        assert_eq!(
            result.as_primitive::<Int64Type>().iter().collect::<Vec<_>>(),
            vec![Some(2), Some(0), None]
        );
    }

    #[test]
    fn evaluates_array_join_with_null_replacement() {
        let mut items = ListBuilder::new(Int64Builder::new());
        items.values().append_value(1);
        items.values().append_null();
        items.values().append_value(3);
        items.append(true);
        items.append(false);
        let batch = RecordBatch::try_from_iter([
            ("items", Arc::new(items.finish()) as ArrayRef),
            ("delimiter", Arc::new(StringArray::from(vec![",", ";"])) as ArrayRef),
            (
                "replacement",
                Arc::new(StringArray::from(vec![Some("?"), Some("-")])) as ArrayRef,
            ),
        ])
        .unwrap();
        let expr = Expr::Function {
            name: "ARRAY_JOIN".into(),
            args: vec![
                Expr::Column("items".into()),
                Expr::Column("delimiter".into()),
                Expr::Column("replacement".into()),
            ],
        };
        let result = evaluate(&expr, &batch).unwrap();
        let values = result.as_string::<i32>();
        assert_eq!(values.value(0), "1,?,3");
        assert_eq!(values.value(1), "");
    }

    #[test]
    fn evaluates_regexp_like_with_nulls_and_invalid_patterns() {
        let batch = RecordBatch::try_from_iter([
            (
                "value",
                Arc::new(StringArray::from(vec![Some("alpha-17"), Some("beta"), None]))
                    as ArrayRef,
            ),
            (
                "pattern",
                Arc::new(StringArray::from(vec![Some(r"^alpha-\d+$"), Some("^alpha"), Some("^x")]))
                    as ArrayRef,
            ),
        ])
        .unwrap();
        let expr = Expr::Function {
            name: "REGEXP_LIKE".into(),
            args: vec![Expr::Column("value".into()), Expr::Column("pattern".into())],
        };
        let result = evaluate(&expr, &batch).unwrap();
        assert_eq!(
            result
                .as_any()
                .downcast_ref::<BooleanArray>()
                .unwrap()
                .iter()
                .collect::<Vec<_>>(),
            vec![Some(true), Some(false), None]
        );

        let invalid = RecordBatch::try_from_iter([
            ("value", Arc::new(StringArray::from(vec![Some("a")])) as ArrayRef),
            ("pattern", Arc::new(StringArray::from(vec![Some("[a")])) as ArrayRef),
        ])
        .unwrap();
        let error = evaluate(&expr, &invalid).unwrap_err();
        assert!(error.to_string().contains("pattern") && error.to_string().contains("invalid"));
    }

    #[test]
    fn evaluates_regexp_count_with_non_overlapping_matches_and_nulls() {
        let batch = RecordBatch::try_from_iter([
            (
                "value",
                Arc::new(StringArray::from(vec![Some("banana"), Some("aaaa"), None]))
                    as ArrayRef,
            ),
            (
                "pattern",
                Arc::new(StringArray::from(vec![Some("ana"), Some("aa"), Some("a")]))
                    as ArrayRef,
            ),
        ])
        .unwrap();
        let expr = Expr::Function {
            name: "REGEXP_COUNT".into(),
            args: vec![Expr::Column("value".into()), Expr::Column("pattern".into())],
        };
        let result = evaluate(&expr, &batch).unwrap();
        assert_eq!(
            result
                .as_any()
                .downcast_ref::<Int64Array>()
                .unwrap()
                .iter()
                .collect::<Vec<_>>(),
            vec![Some(1), Some(2), None]
        );

        let invalid = RecordBatch::try_from_iter([
            ("value", Arc::new(StringArray::from(vec![Some("a")])) as ArrayRef),
            ("pattern", Arc::new(StringArray::from(vec![Some("[")])) as ArrayRef),
        ])
        .unwrap();
        let error = evaluate(&expr, &invalid).unwrap_err();
        assert!(error.to_string().contains("REGEXP_COUNT") && error.to_string().contains("invalid"));
    }

    #[test]
    fn evaluates_regexp_extract_all_with_capture_groups_and_nulls() {
        let batch = RecordBatch::try_from_iter([
            (
                "value",
                Arc::new(StringArray::from(vec![Some("id=12 id=34"), Some("abc"), None]))
                    as ArrayRef,
            ),
            (
                "pattern",
                Arc::new(StringArray::from(vec![Some(r"id=(\d+)"), Some("."), Some("." )]))
                    as ArrayRef,
            ),
        ])
        .unwrap();
        let expr = Expr::Function {
            name: "REGEXP_EXTRACT_ALL".into(),
            args: vec![Expr::Column("value".into()), Expr::Column("pattern".into())],
        };
        let result = evaluate(&expr, &batch).unwrap();
        let values = result.as_any().downcast_ref::<ListArray>().unwrap();
        let first_value = values.value(0);
        let first = first_value.as_any().downcast_ref::<StringArray>().unwrap();
        assert_eq!(first.iter().collect::<Vec<_>>(), vec![Some("12"), Some("34")]);
        let second_value = values.value(1);
        let second = second_value.as_any().downcast_ref::<StringArray>().unwrap();
        assert_eq!(second.iter().collect::<Vec<_>>(), vec![Some("a"), Some("b"), Some("c")]);
        assert!(values.is_null(2));
    }
}
