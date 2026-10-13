use arrow::array::{
    Array, ArrayRef, AsArray, BooleanArray, BooleanBuilder, Date32Array, Date64Array,
    Decimal128Array,
    Decimal128Builder,
    Float64Array, Float64Builder,
    Int32Array, Int32Builder, Int32DictionaryArray, Int64Array, Int64Builder, ListArray,
    ListBuilder, MapArray,
    NullBuilder, PrimitiveArray, PrimitiveBuilder,
    StringArray,
    StringBuilder, StructArray,
    Time32MillisecondArray, Time32SecondArray,
    TimestampMicrosecondArray,
};
use arrow::buffer::{OffsetBuffer, ScalarBuffer};
use arrow::compute;
use arrow::datatypes::{
    ArrowPrimitiveType, DataType, Date32Type, Date64Type, Float64Type, Int32Type, Int64Type,
    Time32MillisecondType, Time32SecondType, TimestampMicrosecondType, TimeUnit,
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
                .map(|a| evaluate(a, batch).and_then(|array| decode_dictionary(&array)))
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
                .chain(order_by.iter().map(|(expr, _)| expr))
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
    let arrow_type = target.to_arrow_type();
    Ok(compute::cast(&arr, &arrow_type)?)
}

// ── Scalar functions ────────────────────────────────────────────────────────

fn eval_scalar_function(name: &str, args: &[ArrayRef], num_rows: usize) -> Result<ArrayRef> {
    let upper = name.to_uppercase();
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
            DataType::Timestamp(TimeUnit::Microsecond, _) => build_array!(
                PrimitiveBuilder::<TimestampMicrosecondType>::new(),
                TimestampMicrosecondArray,
                |values: &TimestampMicrosecondArray, row| values.value(row)
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
            if matches!(args[0].data_type(), DataType::List(_)) {
                let list = args[0]
                    .as_any()
                    .downcast_ref::<arrow::array::ListArray>()
                    .expect("list data type must be ListArray");
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
            let list = args[0]
                .as_any()
                .downcast_ref::<arrow::array::ListArray>()
                .ok_or_else(|| {
                    KaveonError::Execution(
                        "ARRAY_POSITION currently supports ARRAY/List values only".into(),
                    )
                })?;
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
            let list = args[0]
                .as_any()
                .downcast_ref::<ListArray>()
                .ok_or_else(|| KaveonError::Execution("ARRAY_DISTINCT requires an ARRAY/List value".into()))?;
            match list.value_type() {
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
            let list = args[0]
                .as_any()
                .downcast_ref::<ListArray>()
                .ok_or_else(|| KaveonError::Execution("ARRAY_SORT requires an ARRAY/List value".into()))?;
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
            let list = args[0]
                .as_any()
                .downcast_ref::<ListArray>()
                .ok_or_else(|| KaveonError::Execution("ARRAY_REVERSE requires an ARRAY/List value".into()))?;
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
            let list = args[0]
                .as_any()
                .downcast_ref::<ListArray>()
                .ok_or_else(|| KaveonError::Execution("ARRAY_SLICE requires an ARRAY/List value".into()))?;
            let lower = compute::cast(&args[1], &DataType::Int64)?;
            let upper = compute::cast(&args[2], &DataType::Int64)?;
            let stride = compute::cast(&args[3], &DataType::Int64)?;
            let lower = lower.as_primitive::<Int64Type>();
            let upper = upper.as_primitive::<Int64Type>();
            let stride = stride.as_primitive::<Int64Type>();
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
        "STRPOS" | "POSITION" => {
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
                                .find(needle.value(i))
                                .map(|p| p as i64 + 1)
                                .unwrap_or(0),
                        )
                    }
                })
                .collect();
            Ok(Arc::new(result))
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
            let mut result = compute::cast(&args[0], &DataType::Float64)?;
            for arg in &args[1..] {
                let other = compute::cast(arg, &DataType::Float64)?;
                let gt = comparison(&other, &result, CompareKind::Gt)?;
                result = zip_arrays(&gt, &other, &result)?;
            }
            Ok(result)
        }
        "LEAST" => {
            if args.len() < 2 {
                return Err(KaveonError::Execution(
                    "LEAST requires at least 2 arguments".into(),
                ));
            }
            let mut result = compute::cast(&args[0], &DataType::Float64)?;
            for arg in &args[1..] {
                let other = compute::cast(arg, &DataType::Float64)?;
                let lt = comparison(&other, &result, CompareKind::Lt)?;
                result = zip_arrays(&lt, &other, &result)?;
            }
            Ok(result)
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
            let list = args[0]
                .as_any()
                .downcast_ref::<ListArray>()
                .ok_or_else(|| {
                    KaveonError::Execution(format!("{name} requires an ARRAY/List value"))
                })?;
            let take_max = name == "ARRAY_MAX";
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
        DataType::Timestamp(arrow::datatypes::TimeUnit::Microsecond, tz) => {
            let ts_arr = ts.as_primitive::<arrow::datatypes::TimestampMicrosecondType>();
            let result: Vec<Option<i64>> = (0..num_rows)
                .map(|i| {
                    if ts_arr.is_null(i) || unit_arr.is_null(i) {
                        return None;
                    }
                    let us = ts_arr.value(i);
                    let unit = unit_arr.value(i).to_uppercase();
                    Some(truncate_micros(us, &unit))
                })
                .collect();
            Ok(Arc::new(
                arrow::array::TimestampMicrosecondArray::from(result).with_timezone_opt(tz.clone()),
            ))
        }
        _ => Err(KaveonError::Execution(
            "DATE_TRUNC currently supports Timestamp(Microsecond) only".into(),
        )),
    }
}

fn truncate_micros(us: i64, unit: &str) -> i64 {
    let secs = us / 1_000_000;
    let days = secs / 86400;
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
        "HOUR" => (secs / 3600) * 3_600_000_000,
        "MINUTE" => (secs / 60) * 60_000_000,
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
}
