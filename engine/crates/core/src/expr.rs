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
        order_by: Vec<(Expr, bool)>,
        frame: Option<WindowFrame>,
    },
    Extract {
        field: DateField,
        expr: Box<Expr>,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum CastTarget {
    Boolean,
    Int32,
    Int64,
    Float64,
    Utf8,
    Decimal128 { precision: u8, scale: i8 },
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
    CurrentRow,
    Following(u64),
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
            Self::Int32 => arrow::datatypes::DataType::Int32,
            Self::Int64 => arrow::datatypes::DataType::Int64,
            Self::Float64 => arrow::datatypes::DataType::Float64,
            Self::Utf8 => arrow::datatypes::DataType::Utf8,
            Self::Decimal128 { precision, scale } => {
                arrow::datatypes::DataType::Decimal128(*precision, *scale)
            }
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
