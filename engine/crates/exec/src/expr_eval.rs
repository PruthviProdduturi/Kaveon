use arrow::array::{Array, ArrayRef, AsArray, BooleanArray, Float64Array, Int64Array, StringArray};
use arrow::compute;
use arrow::datatypes::{DataType, Float64Type, Int64Type};
use arrow::record_batch::RecordBatch;
use kaveon_core::predicate::ScalarValue;
use kaveon_core::{BinaryOp, Expr, KaveonError, Result};
use std::sync::Arc;

pub fn evaluate(expr: &Expr, batch: &RecordBatch) -> Result<ArrayRef> {
    match expr {
        Expr::Column(name) => {
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
        Expr::Star | Expr::Function { .. } | Expr::Alias { .. } => Err(KaveonError::Execution(
            format!("expression {expr:?} cannot be evaluated in filter context"),
        )),
    }
}

pub fn evaluate_predicate(expr: &Expr, batch: &RecordBatch) -> Result<BooleanArray> {
    let arr = evaluate(expr, batch)?;
    as_boolean(&arr).cloned()
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

fn eval_binary_op(left: &ArrayRef, op: BinaryOp, right: &ArrayRef) -> Result<ArrayRef> {
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

#[cfg(test)]
mod tests {
    use super::*;
    use arrow::array::Int32Array;
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
}
