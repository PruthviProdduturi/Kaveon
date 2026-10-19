use serde::{Deserialize, Serialize};

use crate::predicate::ScalarValue;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum Expr {
    Column(String),
    Literal(ScalarValue),
    BinaryOp {
        left: Box<Expr>,
        op: BinaryOp,
        right: Box<Expr>,
    },
    IsNull(Box<Expr>),
    IsNotNull(Box<Expr>),
    Not(Box<Expr>),
    And(Box<Expr>, Box<Expr>),
    Or(Box<Expr>, Box<Expr>),
    Function {
        name: String,
        args: Vec<Expr>,
    },
    Star,
    Alias {
        expr: Box<Expr>,
        name: String,
    },
    Case {
        operand: Option<Box<Expr>>,
        when_then: Vec<(Expr, Expr)>,
        else_expr: Option<Box<Expr>>,
    },
    Like {
        expr: Box<Expr>,
        pattern: Box<Expr>,
        negated: bool,
        case_insensitive: bool,
    },
    Between {
        expr: Box<Expr>,
        low: Box<Expr>,
        high: Box<Expr>,
        negated: bool,
    },
    InList {
        expr: Box<Expr>,
        list: Vec<Expr>,
        negated: bool,
    },
    Cast {
        expr: Box<Expr>,
        data_type: CastTarget,
    },
    WindowFunction {
        name: String,
        args: Vec<Expr>,
        partition_by: Vec<Expr>,
        /// `(expression, ascending, nulls_first)` ordering for the window.
        order_by: Vec<(Expr, bool, bool)>,
        frame: Option<WindowFrame>,
    },
    Extract {
        field: DateField,
        expr: Box<Expr>,
    },
}

/// Whether an expression reads a relation column. This is used by planners
/// to distinguish constant table functions from input-bound lateral ones.
pub fn expression_contains_column(expr: &Expr) -> bool {
    expression_contains_column_scoped(expr, &[])
}

fn expression_contains_column_scoped(expr: &Expr, bound: &[String]) -> bool {
    match expr {
        Expr::Column(name) => !bound.iter().any(|bound| bound == name),
        Expr::Literal(_) | Expr::Star => false,
        Expr::BinaryOp { left, right, .. } | Expr::And(left, right) | Expr::Or(left, right) => {
            expression_contains_column_scoped(left, bound)
                || expression_contains_column_scoped(right, bound)
        }
        Expr::IsNull(inner)
        | Expr::IsNotNull(inner)
        | Expr::Not(inner)
        | Expr::Cast { expr: inner, .. }
        | Expr::Extract { expr: inner, .. } => expression_contains_column_scoped(inner, bound),
        Expr::Function { name, args } if name == "__KAVEON_LAMBDA" => {
            let mut lambda_bound = bound.to_vec();
            for parameter in args.iter().take(2) {
                if let Expr::Literal(ScalarValue::Utf8(name)) = parameter {
                    lambda_bound.push(name.clone());
                }
            }
            args.get(2)
                .is_some_and(|body| expression_contains_column_scoped(body, &lambda_bound))
        }
        Expr::Function { args, .. } => args
            .iter()
            .any(|arg| expression_contains_column_scoped(arg, bound)),
        Expr::Alias { expr, .. } => expression_contains_column_scoped(expr, bound),
        Expr::Case {
            operand,
            when_then,
            else_expr,
        } => {
            operand
                .as_deref()
                .is_some_and(|expr| expression_contains_column_scoped(expr, bound))
                || when_then.iter().any(|(when, then)| {
                    expression_contains_column_scoped(when, bound)
                        || expression_contains_column_scoped(then, bound)
                })
                || else_expr
                    .as_deref()
                    .is_some_and(|expr| expression_contains_column_scoped(expr, bound))
        }
        Expr::Like { expr, pattern, .. } => {
            expression_contains_column_scoped(expr, bound)
                || expression_contains_column_scoped(pattern, bound)
        }
        Expr::Between {
            expr, low, high, ..
        } => {
            expression_contains_column_scoped(expr, bound)
                || expression_contains_column_scoped(low, bound)
                || expression_contains_column_scoped(high, bound)
        }
        Expr::InList { expr, list, .. } => {
            expression_contains_column_scoped(expr, bound)
                || list
                    .iter()
                    .any(|item| expression_contains_column_scoped(item, bound))
        }
        Expr::WindowFunction {
            args,
            partition_by,
            order_by,
            ..
        } => {
            args.iter()
                .any(|arg| expression_contains_column_scoped(arg, bound))
                || partition_by
                    .iter()
                    .any(|expr| expression_contains_column_scoped(expr, bound))
                || order_by
                    .iter()
                    .any(|(expr, _, _)| expression_contains_column_scoped(expr, bound))
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum CastTarget {
    Boolean,
    Int8,
    Int16,
    Int32,
    Int64,
    UInt8,
    UInt16,
    UInt32,
    UInt64,
    /// Trino REAL uses IEEE-754 single precision (Arrow Float32), distinct
    /// from DOUBLE/Float64. Keeping this target explicit preserves the
    /// rounding boundary at the SQL type system rather than silently
    /// widening REAL values to DOUBLE.
    Float32,
    Float64,
    Utf8,
    Date32,
    TimestampMicros {
        with_timezone: bool,
    },
    Decimal128 {
        precision: u8,
        scale: i8,
    },
    /// A recursively typed array target, used for typed NULL arrays and
    /// row-producing UNNEST inputs where the value carries no Arrow type.
    Array(Box<CastTarget>),
    /// A recursively typed map target. This is primarily needed for typed
    /// NULL map literals, where the source carries no Arrow type to infer.
    Map {
        key: Box<CastTarget>,
        value: Box<CastTarget>,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum WindowFrameUnits {
    Rows,
    Range,
    Groups,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum WindowFrameBound {
    UnboundedPreceding,
    Preceding(u64),
    /// A calendar-day RANGE offset for DATE ordering.  This is kept distinct
    /// from row counts because Trino's temporal RANGE grammar uses INTERVAL.
    PrecedingDays(u64),
    /// A calendar-month RANGE offset for DATE ordering. Month ends clamp to
    /// the last valid day in the target month, matching SQL date arithmetic.
    PrecedingMonths(u64),
    /// A fixed-second RANGE offset for TIMESTAMP ordering.  The executor
    /// converts seconds to the timestamp column's physical time unit without
    /// applying calendar arithmetic.
    PrecedingSeconds(u64),
    /// A fractional fixed-second RANGE offset represented in nanoseconds.
    PrecedingNanos(u64),
    CurrentRow,
    Following(u64),
    FollowingDays(u64),
    FollowingMonths(u64),
    FollowingSeconds(u64),
    FollowingNanos(u64),
    UnboundedFollowing,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct WindowFrame {
    pub units: WindowFrameUnits,
    pub start: WindowFrameBound,
    pub end: WindowFrameBound,
}

impl CastTarget {
    pub fn to_arrow_type(&self) -> arrow::datatypes::DataType {
        match self {
            Self::Boolean => arrow::datatypes::DataType::Boolean,
            Self::Int8 => arrow::datatypes::DataType::Int8,
            Self::Int16 => arrow::datatypes::DataType::Int16,
            Self::Int32 => arrow::datatypes::DataType::Int32,
            Self::Int64 => arrow::datatypes::DataType::Int64,
            Self::UInt8 => arrow::datatypes::DataType::UInt8,
            Self::UInt16 => arrow::datatypes::DataType::UInt16,
            Self::UInt32 => arrow::datatypes::DataType::UInt32,
            Self::UInt64 => arrow::datatypes::DataType::UInt64,
            Self::Float32 => arrow::datatypes::DataType::Float32,
            Self::Float64 => arrow::datatypes::DataType::Float64,
            Self::Utf8 => arrow::datatypes::DataType::Utf8,
            Self::Date32 => arrow::datatypes::DataType::Date32,
            Self::TimestampMicros { with_timezone } => arrow::datatypes::DataType::Timestamp(
                arrow::datatypes::TimeUnit::Microsecond,
                with_timezone.then(|| "UTC".into()),
            ),
            Self::Decimal128 { precision, scale } => {
                arrow::datatypes::DataType::Decimal128(*precision, *scale)
            }
            Self::Array(element) => arrow::datatypes::DataType::List(std::sync::Arc::new(
                arrow::datatypes::Field::new("item", element.to_arrow_type(), true),
            )),
            Self::Map { key, value } => arrow::datatypes::DataType::Map(
                std::sync::Arc::new(arrow::datatypes::Field::new(
                    "entries",
                    arrow::datatypes::DataType::Struct(
                        vec![
                            arrow::datatypes::Field::new("key", key.to_arrow_type(), false),
                            arrow::datatypes::Field::new("value", value.to_arrow_type(), true),
                        ]
                        .into(),
                    ),
                    false,
                )),
                false,
            ),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum DateField {
    Year,
    Month,
    Day,
    Hour,
    Minute,
    Second,
    DayOfWeek,
    DayOfYear,
    Quarter,
    Week,
    Epoch,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum BinaryOp {
    Eq,
    Ne,
    Lt,
    Le,
    Gt,
    Ge,
    Plus,
    Minus,
    Multiply,
    Divide,
    Modulo,
    StringConcat,
}

/// The aggregate functions the SQL front end recognises, by their
/// canonical upper-case names. `APPROX_DISTINCT` is Trino's name for
/// `APPROX_COUNT_DISTINCT` and is normalised to it when parsed.
pub const AGGREGATE_FUNCTION_NAMES: &[&str] = &[
    "COUNT",
    "COUNT_IF",
    "SUM",
    "AVG",
    "MIN",
    "MAX",
    "APPROX_COUNT_DISTINCT",
    "APPROX_PERCENTILE",
    "APPROX_COUNT_DISTINCT_STATE",
    "COLUMN_STATISTICS",
];

/// Whether `name` (upper-case) is an aggregate function.
pub fn is_aggregate_function(name: &str) -> bool {
    AGGREGATE_FUNCTION_NAMES.contains(&name)
}

/// The output column an aggregate call is named by when the statement
/// gives it no alias: the function in lower case, an underscore, and the
/// arguments' labels — a column by its name, `*`, a number by its value
/// (`approx_percentile_latency, 0.5`), anything else `expr`. Every planner
/// names aggregate outputs with this, so a projection over the aggregate
/// binds to them by the same rule.
pub fn aggregate_output_name(function: &str, args: &[Expr]) -> String {
    let labels = args
        .iter()
        .map(aggregate_argument_label)
        .collect::<Vec<_>>()
        .join(", ");
    format!("{}_{labels}", function.to_ascii_lowercase())
}

fn aggregate_argument_label(expr: &Expr) -> String {
    match expr {
        Expr::Column(name) => name.clone(),
        Expr::Star => "*".to_owned(),
        Expr::Literal(ScalarValue::Int64(value)) => value.to_string(),
        Expr::Literal(ScalarValue::Float64(value)) => value.to_string(),
        Expr::Literal(ScalarValue::Decimal128 { value, scale, .. }) => {
            (*value as f64 / 10f64.powi(i32::from(*scale))).to_string()
        }
        Expr::Function { name, args } if name.eq_ignore_ascii_case("ARRAY") => format!(
            "array[{}]",
            args.iter()
                .map(aggregate_argument_label)
                .collect::<Vec<_>>()
                .join(", ")
        ),
        _ => "expr".to_owned(),
    }
}
