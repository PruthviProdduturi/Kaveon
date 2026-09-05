use arrow::array::{
    Array, ArrayRef, AsArray, BooleanArray, Float64Array, Int32Array, Int64Array, StringArray,
};
use arrow::compute;
use arrow::datatypes::{DataType, Float64Type, Int32Type, Int64Type};
use arrow::record_batch::RecordBatch;
use kaveon_core::predicate::ScalarValue;
use kaveon_core::{BinaryOp, CastTarget, DateField, Expr, KaveonError, Result};
use std::sync::Arc;

pub fn evaluate(expr: &Expr, batch: &RecordBatch) -> Result<ArrayRef> {
    match expr {
        Expr::Column(name) => resolve_column(name, batch),
        Expr::Literal(value) => literal_to_array(value, batch.num_rows()),
        Expr::BinaryOp { left, op, right } => {
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
            let evaluated_args: Vec<ArrayRef> = args
                .iter()
                .map(|a| evaluate(a, batch))
                .collect::<Result<_>>()?;
            eval_scalar_function(name, &evaluated_args, batch.num_rows())
        }
        Expr::Alias { expr, .. } => evaluate(expr, batch),
        Expr::Extract { field, expr } => eval_extract(*field, expr, batch),
        Expr::WindowFunction { name, .. } => Err(KaveonError::Execution(format!(
            "window function {name} must be evaluated by the WindowOperator, not inline"
        ))),
        Expr::Star => Err(KaveonError::Execution(
            "star (*) cannot be evaluated as an expression".into(),
        )),
    }
}

pub fn evaluate_predicate(expr: &Expr, batch: &RecordBatch) -> Result<BooleanArray> {
    let arr = evaluate(expr, batch)?;
    as_boolean(&arr).cloned()
}

fn resolve_column(name: &str, batch: &RecordBatch) -> Result<ArrayRef> {
    let schema = batch.schema();
    let idx = match schema.index_of(name) {
        Ok(index) => index,
        Err(_) => {
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
                [index] => *index,
                [] => {
                    return Err(KaveonError::Execution(format!(
                        "column '{name}' not found in batch"
                    )));
                }
                _ => {
                    return Err(KaveonError::Execution(format!(
                        "column '{name}' is ambiguous in batch"
                    )));
                }
            }
        }
    };
    Ok(Arc::clone(batch.column(idx)))
}

fn literal_to_array(value: &ScalarValue, len: usize) -> Result<ArrayRef> {
    match value {
        ScalarValue::Null => Ok(Arc::new(BooleanArray::new_null(len))),
        ScalarValue::Bool(v) => Ok(Arc::new(BooleanArray::from(vec![*v; len]))),
        ScalarValue::Int64(v) => Ok(Arc::new(Int64Array::from(vec![*v; len]))),
        ScalarValue::Float64(v) => Ok(Arc::new(Float64Array::from(vec![*v; len]))),
        ScalarValue::Utf8(v) => Ok(Arc::new(StringArray::from(vec![v.as_str(); len]))),
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
        (l, r) => {
            return Err(KaveonError::Execution(format!(
                "arithmetic not supported between {l} and {r}"
            )));
        }
    };
    Ok(result)
}

fn coerce_numeric_pair(left: &ArrayRef, right: &ArrayRef) -> Result<(ArrayRef, ArrayRef)> {
    if left.data_type() == right.data_type() {
        return Ok((Arc::clone(left), Arc::clone(right)));
    }
    if is_numeric(left.data_type()) && is_numeric(right.data_type()) {
        return Ok((
            compute::cast(left, &DataType::Float64)?,
            compute::cast(right, &DataType::Float64)?,
        ));
    }
    Ok((Arc::clone(left), Arc::clone(right)))
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
    let patterns = evaluate(pattern, batch)?;
    let values = as_string_array(&values)?;
    let patterns = as_string_array(&patterns)?;

    let result: BooleanArray = (0..values.len())
        .map(|i| {
            if values.is_null(i) || patterns.is_null(i) {
                None
            } else {
                let text = values.value(i);
                let pat = patterns.value(i);
                let matched = if case_insensitive {
                    like_match(&text.to_lowercase(), &pat.to_lowercase())
                } else {
                    like_match(text, pat)
                };
                Some(if negated { !matched } else { matched })
            }
        })
        .collect();
    Ok(Arc::new(result))
}

fn like_match(text: &str, pattern: &str) -> bool {
    let text = text.as_bytes();
    let pattern = pattern.as_bytes();
    let (t_len, p_len) = (text.len(), pattern.len());

    let mut prev = vec![false; p_len + 1];
    let mut curr = vec![false; p_len + 1];
    prev[0] = true;
    for j in 1..=p_len {
        if pattern[j - 1] == b'%' {
            prev[j] = prev[j - 1];
        }
    }

    for i in 1..=t_len {
        curr[0] = false;
        for j in 1..=p_len {
            curr[j] = match pattern[j - 1] {
                b'%' => curr[j - 1] || prev[j],
                b'_' => prev[j - 1],
                c => prev[j - 1] && text[i - 1] == c,
            };
        }
        std::mem::swap(&mut prev, &mut curr);
    }
    prev[p_len]
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
        result = compute::or(&result, &eq)?;
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
            let string_args: Vec<StringArray> = args
                .iter()
                .map(|a| cast_to_string(a))
                .collect::<Result<_>>()?;
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
        "REPLACE" => {
            check_arity(name, args, 3)?;
            let arr = as_string_array(&args[0])?;
            let from = as_string_array(&args[1])?;
            let to = as_string_array(&args[2])?;
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
        DateField::DayOfWeek => ((days % 7 + 4 + 7) % 7) as i64,
        DateField::DayOfYear => {
            let jan1 = ymd_to_days(year, 1, 1);
            (days - jan1 + 1) as i64
        }
        DateField::Quarter => ((month - 1) / 3 + 1) as i64,
        DateField::Week => {
            let jan1 = ymd_to_days(year, 1, 1);
            let doy = days - jan1;
            (doy / 7 + 1) as i64
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
    let doy = (153 * m_adj + 2) / 5 + d as u32 - 1;
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
            let dow = ((days % 7 + 4 + 7) % 7) as i64;
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
        assert_eq!(bools.value(0), true);
        assert_eq!(bools.value(1), false);
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
