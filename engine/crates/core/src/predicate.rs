use arrow::datatypes::DataType;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum ScalarValue {
    Null,
    Bool(bool),
    Int64(i64),
    Float64(f64),
    Utf8(String),
    Decimal128 {
        value: i128,
        precision: u8,
        scale: i8,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum CompareOp {
    Eq,
    Ne,
    Lt,
    Le,
    Gt,
    Ge,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum StoragePredicate {
    Compare {
        column: String,
        op: CompareOp,
        value: ScalarValue,
    },
    IsNull {
        column: String,
    },
    IsNotNull {
        column: String,
    },
    In {
        column: String,
        values: Vec<ScalarValue>,
    },
    /// `column [NOT] [I]LIKE pattern` with a literal pattern: SQL `%` and
    /// `_` wildcards, evaluated by the storage layer with the same Arrow
    /// kernel the executor uses.
    Like {
        column: String,
        pattern: String,
        negated: bool,
        case_insensitive: bool,
    },
    And(Vec<StoragePredicate>),
    Or(Vec<StoragePredicate>),
    Not(Box<StoragePredicate>),
}

impl ScalarValue {
    pub fn data_type(&self) -> DataType {
        match self {
            Self::Null => DataType::Null,
            Self::Bool(_) => DataType::Boolean,
            Self::Int64(_) => DataType::Int64,
            Self::Float64(_) => DataType::Float64,
            Self::Utf8(_) => DataType::Utf8,
            Self::Decimal128 {
                precision, scale, ..
            } => DataType::Decimal128(*precision, *scale),
        }
    }
}

/// Days since 1970-01-01 for an ISO `YYYY-MM-DD` text, or None when the
/// text is not a date. The one parser every literal-to-date coercion uses:
/// SQL DATE literals, and text literals compared against date columns.
pub fn date_literal_days(value: &str) -> Option<i64> {
    let mut parts = value.trim().splitn(3, '-');
    let year: i64 = parts.next()?.parse().ok()?;
    let month: i64 = parts.next()?.parse().ok()?;
    let day: i64 = parts.next()?.parse().ok()?;
    if !(1..=12).contains(&month) || !(1..=31).contains(&day) {
        return None;
    }
    let y = if month <= 2 { year - 1 } else { year };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let m = if month > 2 { month - 3 } else { month + 9 };
    let doy = (153 * m + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    Some(era * 146097 + doe - 719468)
}

impl ScalarValue {
    /// The value as a column of `data_type` would compare it: a text
    /// literal against a day-number date column becomes its day number,
    /// the way SQL coerces `date_col = '2026-07-20'`; an integer or
    /// decimal literal against a floating-point column becomes a double,
    /// the way `quantity < 24` and `discount > 0.05` read against double
    /// columns. Anything else is unchanged; the consumer keeps its own type
    /// checks.
    #[must_use]
    pub fn coerced_for(&self, data_type: &arrow::datatypes::DataType) -> ScalarValue {
        use arrow::datatypes::DataType;
        let data_type = match data_type {
            DataType::Dictionary(_, values) => values.as_ref(),
            other => other,
        };
        match (self, data_type) {
            (ScalarValue::Utf8(text), DataType::Date32) => match date_literal_days(text) {
                Some(days) => ScalarValue::Int64(days),
                None => self.clone(),
            },
            (ScalarValue::Int64(value), DataType::Float32 | DataType::Float64) => {
                ScalarValue::Float64(*value as f64)
            }
            (
                ScalarValue::Decimal128 { value, scale, .. },
                DataType::Float32 | DataType::Float64,
            ) => ScalarValue::Float64(*value as f64 / 10f64.powi(i32::from(*scale))),
            _ => self.clone(),
        }
    }
}

impl StoragePredicate {
    /// The columns the predicate reads, each once, in first-mention order.
    pub fn columns(&self) -> Vec<&str> {
        let mut columns = Vec::new();
        self.collect_columns(&mut columns);
        columns
    }

    fn collect_columns<'a>(&'a self, out: &mut Vec<&'a str>) {
        let mut push = |column: &'a str| {
            if !out.contains(&column) {
                out.push(column);
            }
        };
        match self {
            StoragePredicate::Compare { column, .. }
            | StoragePredicate::IsNull { column }
            | StoragePredicate::IsNotNull { column }
            | StoragePredicate::In { column, .. }
            | StoragePredicate::Like { column, .. } => push(column),
            StoragePredicate::And(children) | StoragePredicate::Or(children) => {
                for child in children {
                    child.collect_columns(out);
                }
            }
            StoragePredicate::Not(inner) => inner.collect_columns(out),
        }
    }

    /// The predicate with every literal coerced for its column's type.
    #[must_use]
    pub fn coerced_for(&self, schema: &arrow::datatypes::SchemaRef) -> StoragePredicate {
        let column_type = |name: &str| {
            schema
                .field_with_name(name)
                .ok()
                .map(|field| field.data_type().clone())
        };
        match self {
            StoragePredicate::Compare { column, op, value } => StoragePredicate::Compare {
                column: column.clone(),
                op: *op,
                value: column_type(column).map_or_else(|| value.clone(), |t| value.coerced_for(&t)),
            },
            StoragePredicate::In { column, values } => StoragePredicate::In {
                column: column.clone(),
                values: match column_type(column) {
                    Some(t) => values.iter().map(|value| value.coerced_for(&t)).collect(),
                    None => values.clone(),
                },
            },
            StoragePredicate::And(children) => {
                StoragePredicate::And(children.iter().map(|c| c.coerced_for(schema)).collect())
            }
            StoragePredicate::Or(children) => {
                StoragePredicate::Or(children.iter().map(|c| c.coerced_for(schema)).collect())
            }
            StoragePredicate::Not(inner) => {
                StoragePredicate::Not(Box::new(inner.coerced_for(schema)))
            }
            other => other.clone(),
        }
    }
}

#[cfg(test)]
mod coercion_tests {
    use super::*;
    use arrow::datatypes::{DataType, Field, Schema};
    use std::sync::Arc;

    #[test]
    fn text_literals_meet_date_columns_as_day_numbers() {
        assert_eq!(date_literal_days("1970-01-01"), Some(0));
        assert_eq!(date_literal_days("2026-07-20"), Some(20_654));
        assert_eq!(date_literal_days("2026-13-01"), None);
        assert_eq!(date_literal_days("July"), None);
        let text = ScalarValue::Utf8("2026-07-20".into());
        assert_eq!(
            text.coerced_for(&DataType::Date32),
            ScalarValue::Int64(20_654)
        );
        assert_eq!(text.coerced_for(&DataType::Utf8), text);
        assert_eq!(
            ScalarValue::Utf8("not a date".into()).coerced_for(&DataType::Date32),
            ScalarValue::Utf8("not a date".into())
        );
        let schema = Arc::new(Schema::new(vec![
            Field::new("day", DataType::Date32, false),
            Field::new("name", DataType::Utf8, true),
        ]));
        let predicate = StoragePredicate::And(vec![
            StoragePredicate::Compare {
                column: "day".into(),
                op: CompareOp::Ge,
                value: ScalarValue::Utf8("2026-07-20".into()),
            },
            StoragePredicate::In {
                column: "name".into(),
                values: vec![ScalarValue::Utf8("2026-07-20".into())],
            },
        ]);
        let StoragePredicate::And(children) = predicate.coerced_for(&schema) else {
            panic!("shape is kept");
        };
        assert_eq!(
            children[0],
            StoragePredicate::Compare {
                column: "day".into(),
                op: CompareOp::Ge,
                value: ScalarValue::Int64(20_654),
            }
        );
        assert_eq!(
            children[1],
            StoragePredicate::In {
                column: "name".into(),
                values: vec![ScalarValue::Utf8("2026-07-20".into())],
            }
        );
    }

    #[test]
    fn integer_and_decimal_literals_meet_double_columns_as_doubles() {
        assert_eq!(
            ScalarValue::Int64(24).coerced_for(&DataType::Float64),
            ScalarValue::Float64(24.0)
        );
        assert_eq!(
            ScalarValue::Int64(24).coerced_for(&DataType::Float32),
            ScalarValue::Float64(24.0)
        );
        assert_eq!(
            ScalarValue::Decimal128 {
                value: 5,
                precision: 3,
                scale: 2
            }
            .coerced_for(&DataType::Float64),
            ScalarValue::Float64(0.05)
        );
        assert_eq!(
            ScalarValue::Decimal128 {
                value: -1_250,
                precision: 5,
                scale: 3
            }
            .coerced_for(&DataType::Float64),
            ScalarValue::Float64(-1.25)
        );
        // Integer columns keep integer literals, and a dictionary column is
        // its value type.
        assert_eq!(
            ScalarValue::Int64(24).coerced_for(&DataType::Int32),
            ScalarValue::Int64(24)
        );
        assert_eq!(
            ScalarValue::Int64(24).coerced_for(&DataType::Dictionary(
                Box::new(DataType::Int32),
                Box::new(DataType::Float64)
            )),
            ScalarValue::Float64(24.0)
        );
        let schema = Arc::new(Schema::new(vec![Field::new(
            "quantity",
            DataType::Float64,
            false,
        )]));
        assert_eq!(
            StoragePredicate::Compare {
                column: "quantity".into(),
                op: CompareOp::Lt,
                value: ScalarValue::Int64(24),
            }
            .coerced_for(&schema),
            StoragePredicate::Compare {
                column: "quantity".into(),
                op: CompareOp::Lt,
                value: ScalarValue::Float64(24.0),
            }
        );
    }
}
