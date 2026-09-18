use arrow::array::{
    Array, ArrayRef, AsArray, BooleanArray, Decimal128Array, Float64Array, Int32Array,
    Int32DictionaryArray, Int64Array, StringArray, StringBuilder,
};
use arrow::compute;
use arrow::datatypes::{DataType, Float64Type, Int32Type, Int64Type};
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
            "REGEXP_REPLACE pattern {pattern:?} is invalid: {error}"
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
    rewrap_dictionary(&keys, over_values).map(Some)
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
    match name.to_uppercase().as_str() {
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
        "CONTAINS" | "STRPOS" | "POSITION" => {
            check_arity(name, args, 2)?;
            let arr = as_string_array(&args[0])?;
            let needle = as_string_array(&args[1])?;
            if name.eq_ignore_ascii_case("CONTAINS") {
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
            } else {
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
        // Every row its own value: no repetition to exploit.
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
}
