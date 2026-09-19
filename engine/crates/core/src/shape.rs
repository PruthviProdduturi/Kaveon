//! The declared shape of a table: which columns are dimensions, which are
//! measures and under which aggregates, and an optional time dimension at
//! a grain. The shape is what the cube is built over — nothing declared,
//! no cube — and the counterpart of the DLM's per-dataset context spec
//! (dimensions, measures, additivity, breakdowns, combo cells) for tables
//! the Engine reads.

use crate::catalog::ColumnDefinition;
use crate::statistics::StatValue;
use arrow::datatypes::{DataType, TimeUnit};
use serde::{Deserialize, Serialize};

/// The distinct values a dimension may hold and still be an axis of the
/// cube, when the declaration gives no cap of its own.
pub const DEFAULT_DIMENSION_CAP: u64 = 10_000;
/// The buckets a day-grain time axis is planned at: ten years of days.
pub const DEFAULT_DAY_CAP: u64 = 3_660;
/// The buckets a month-grain time axis is planned at: ten years of months.
pub const DEFAULT_MONTH_CAP: u64 = 120;
/// Two axes form a grouping of the cube when the product of their caps is
/// at most this many cells.
pub const CUBE_PAIR_MAX_CELLS: u64 = 1_000_000;
/// The cells a cube may hold when the server sets no limit of its own
/// (`KAVEON_CUBE_MAX_CELLS`).
pub const DEFAULT_CUBE_MAX_CELLS: u64 = 1_000_000;

/// An aggregate a measure is declared under. `Sum`, `Count`, `Min` and
/// `Max` are additive: a file's contribution folds in and, from the
/// per-file partials, folds out. `CountDistinct` is not: it is kept as a
/// HyperLogLog sketch, mergeable but never subtractable.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MeasureAggregate {
    Sum,
    Count,
    Min,
    Max,
    CountDistinct,
}

impl MeasureAggregate {
    pub const ALL: [Self; 5] = [
        Self::Sum,
        Self::Count,
        Self::Min,
        Self::Max,
        Self::CountDistinct,
    ];

    /// The name the declaration spells: `sum`, `count`, `min`, `max`,
    /// `count_distinct`.
    pub const fn name(self) -> &'static str {
        match self {
            Self::Sum => "sum",
            Self::Count => "count",
            Self::Min => "min",
            Self::Max => "max",
            Self::CountDistinct => "count_distinct",
        }
    }

    pub fn parse(text: &str) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|aggregate| aggregate.name().eq_ignore_ascii_case(text.trim()))
    }

    /// Whether a removed file's contribution can be taken out again from
    /// the per-file partials.
    pub const fn is_additive(self) -> bool {
        !matches!(self, Self::CountDistinct)
    }

    /// Whether a column of `data_type` can be measured under this
    /// aggregate: sums over numbers, bounds over what orders, counts over
    /// anything, distinct counts over what a sketch hashes.
    pub fn accepts(self, data_type: &DataType) -> bool {
        let data_type = logical_type(data_type);
        match self {
            Self::Sum => is_integer(data_type) || is_float(data_type) || is_decimal(data_type),
            Self::Count => true,
            Self::Min | Self::Max => {
                is_integer(data_type)
                    || is_float(data_type)
                    || is_decimal(data_type)
                    || matches!(
                        data_type,
                        DataType::Utf8 | DataType::LargeUtf8 | DataType::Date32
                    )
            }
            Self::CountDistinct => crate::sketch::sketchable(data_type),
        }
    }
}

/// A dimension: a column the cube breaks measures down by, and the most
/// distinct values it may hold and still be an axis. A dimension over its
/// cap is left out of the cube (`TableCube::excluded`), not refused.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ShapeDimension {
    pub name: String,
    #[serde(default = "default_dimension_cap")]
    pub cap: u64,
}

const fn default_dimension_cap() -> u64 {
    DEFAULT_DIMENSION_CAP
}

impl ShapeDimension {
    /// Whether a column of `data_type` can be a dimension: booleans,
    /// integers, text, dates, timestamps and decimals — what groups by
    /// value; floating-point columns do not.
    pub fn accepts(data_type: &DataType) -> bool {
        let data_type = logical_type(data_type);
        crate::sketch::sketchable(data_type) && !is_float(data_type)
    }
}

/// A measure: a column and the aggregates the cube keeps it under.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ShapeMeasure {
    pub column: String,
    pub aggregates: Vec<MeasureAggregate>,
}

/// The grain a time dimension is bucketed at.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TimeGrain {
    Day,
    Month,
}

impl TimeGrain {
    pub const fn name(self) -> &'static str {
        match self {
            Self::Day => "day",
            Self::Month => "month",
        }
    }

    pub fn parse(text: &str) -> Option<Self> {
        let text = text.trim();
        if text.eq_ignore_ascii_case("day") {
            Some(Self::Day)
        } else if text.eq_ignore_ascii_case("month") {
            Some(Self::Month)
        } else {
            None
        }
    }

    pub const fn default_cap(self) -> u64 {
        match self {
            Self::Day => DEFAULT_DAY_CAP,
            Self::Month => DEFAULT_MONTH_CAP,
        }
    }

    /// The value at the grain: a date at day grain is itself; a
    /// microsecond timestamp is truncated as `DATE_TRUNC` truncates it.
    /// `None` for a value the grain does not apply to (a date at month
    /// grain: the row path has no `DATE_TRUNC` over dates, so the shape
    /// refuses it).
    pub fn truncate(self, value: &StatValue) -> Option<StatValue> {
        match (self, value) {
            (Self::Day, StatValue::Date(days)) => Some(StatValue::Date(*days)),
            (
                grain,
                StatValue::Timestamp {
                    value,
                    unit: TimeUnit::Microsecond,
                    utc,
                },
            ) => Some(StatValue::Timestamp {
                value: truncate_micros(*value, grain),
                unit: TimeUnit::Microsecond,
                utc: *utc,
            }),
            _ => None,
        }
    }
}

/// A microsecond timestamp truncated to the grain, as the executor's
/// `DATE_TRUNC` truncates it (the same civil-day arithmetic, so a cube
/// bucket and a computed bucket are the same value).
pub fn truncate_micros(us: i64, grain: TimeGrain) -> i64 {
    let secs = us / 1_000_000;
    let days = secs / 86_400;
    match grain {
        TimeGrain::Day => days * 86_400_000_000,
        TimeGrain::Month => {
            let (year, month, _) = days_to_ymd(days);
            ymd_to_days(year, month, 1) * 86_400_000_000
        }
    }
}

fn days_to_ymd(mut days: i64) -> (i32, u32, u32) {
    days += 719_468;
    let era = if days >= 0 { days } else { days - 146_096 } / 146_097;
    let doe = (days - era * 146_097) as u32;
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let y = i64::from(yoe) + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = if m <= 2 { y + 1 } else { y };
    (year as i32, m, d)
}

fn ymd_to_days(y: i32, m: u32, d: u32) -> i64 {
    let y = i64::from(y);
    let m = i64::from(m);
    let y = if m <= 2 { y - 1 } else { y };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = (y - era * 400) as u32;
    let m_adj = if m > 2 { m - 3 } else { m + 9 } as u32;
    let doy = (153 * m_adj + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + i64::from(doe) - 719_468
}

/// The time dimension: a date or microsecond-timestamp column at a grain,
/// and the buckets it is planned at.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ShapeTime {
    pub column: String,
    pub grain: TimeGrain,
    pub cap: u64,
}

impl ShapeTime {
    /// Whether a column of `data_type` can be the time dimension at
    /// `grain`: a date at day grain, a microsecond timestamp at either.
    pub fn accepts(data_type: &DataType, grain: TimeGrain) -> bool {
        matches!(
            (logical_type(data_type), grain),
            (DataType::Date32, TimeGrain::Day) | (DataType::Timestamp(TimeUnit::Microsecond, _), _)
        )
    }
}

/// One axis of the cube: a dimension, or the time dimension at its grain.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ShapeAxis {
    pub column: String,
    pub cap: u64,
    /// `Some` for the time axis.
    pub grain: Option<TimeGrain>,
}

/// The declared shape. Empty (no dimensions, no measures, no time) means
/// none is declared.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TableShape {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub dimensions: Vec<ShapeDimension>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub measures: Vec<ShapeMeasure>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub time: Option<ShapeTime>,
}

fn shape_error(message: impl Into<String>) -> crate::KaveonError {
    crate::KaveonError::Execution(message.into())
}

impl TableShape {
    pub fn is_empty(&self) -> bool {
        self.dimensions.is_empty() && self.measures.is_empty() && self.time.is_none()
    }

    /// The shape from its declaration: `dimensions` entries `column` or
    /// `column:cap`, `measures` entries `column:agg[,agg…]`, `time` as
    /// `column:grain[:cap]`. Names are checked for form here and against
    /// the table's columns in [`TableShape::check_against`].
    pub fn parse(
        dimensions: &[String],
        measures: &[String],
        time: Option<&str>,
    ) -> crate::Result<Self> {
        let mut shape = Self::default();
        let mut seen = std::collections::HashSet::new();
        for entry in dimensions {
            let (name, cap) = match entry.rsplit_once(':') {
                Some((name, cap)) if cap.trim().parse::<u64>().is_ok() => {
                    let cap = cap.trim().parse::<u64>().expect("checked");
                    if cap == 0 {
                        return Err(shape_error(format!(
                            "dimension '{name}' has a cardinality cap of 0"
                        )));
                    }
                    (name, cap)
                }
                _ => (entry.as_str(), DEFAULT_DIMENSION_CAP),
            };
            let name = name.trim();
            if name.is_empty() {
                return Err(shape_error("a dimension names an empty column"));
            }
            if !seen.insert(name.to_owned()) {
                return Err(shape_error(format!("dimension '{name}' is declared twice")));
            }
            shape.dimensions.push(ShapeDimension {
                name: name.to_owned(),
                cap,
            });
        }
        let mut seen = std::collections::HashSet::new();
        for entry in measures {
            let Some((column, aggregates)) = entry.rsplit_once(':') else {
                return Err(shape_error(format!(
                    "measure '{entry}' needs column:aggregate[,aggregate…] with aggregates sum, count, min, max, count_distinct"
                )));
            };
            let column = column.trim();
            if column.is_empty() {
                return Err(shape_error("a measure names an empty column"));
            }
            if !seen.insert(column.to_owned()) {
                return Err(shape_error(format!(
                    "measure '{column}' is declared twice; list its aggregates in one entry"
                )));
            }
            let mut parsed = Vec::new();
            for aggregate in aggregates.split(',') {
                let Some(aggregate) = MeasureAggregate::parse(aggregate) else {
                    return Err(shape_error(format!(
                        "measure '{column}': aggregate '{}' is not one of sum, count, min, max, count_distinct",
                        aggregate.trim()
                    )));
                };
                if !parsed.contains(&aggregate) {
                    parsed.push(aggregate);
                }
            }
            shape.measures.push(ShapeMeasure {
                column: column.to_owned(),
                aggregates: parsed,
            });
        }
        if let Some(time) = time {
            let mut parts = time.split(':');
            let column = parts.next().unwrap_or("").trim();
            let grain = parts.next().map(str::trim);
            let cap = parts.next().map(str::trim);
            if column.is_empty() || grain.is_none() || parts.next().is_some() {
                return Err(shape_error(
                    "time needs column:grain[:cap] with grain day or month",
                ));
            }
            let grain = grain.and_then(TimeGrain::parse).ok_or_else(|| {
                shape_error(format!(
                    "time '{column}': grain '{}' is not day or month",
                    time.split(':').nth(1).unwrap_or("").trim()
                ))
            })?;
            let cap = match cap {
                Some(cap) => cap
                    .parse::<u64>()
                    .ok()
                    .filter(|cap| *cap > 0)
                    .ok_or_else(|| {
                        shape_error(format!(
                            "time '{column}': cap '{cap}' is not a positive integer"
                        ))
                    })?,
                None => grain.default_cap(),
            };
            if shape
                .dimensions
                .iter()
                .any(|dimension| dimension.name == column)
            {
                return Err(shape_error(format!(
                    "column '{column}' is both a dimension and the time dimension"
                )));
            }
            shape.time = Some(ShapeTime {
                column: column.to_owned(),
                grain,
                cap,
            });
        }
        Ok(shape)
    }

    /// The declaration that recreates the shape: the `dimensions`,
    /// `measures` and `time` values of `SET SHAPE (…)`.
    pub fn render(&self) -> (Vec<String>, Vec<String>, Option<String>) {
        let dimensions = self
            .dimensions
            .iter()
            .map(|dimension| {
                if dimension.cap == DEFAULT_DIMENSION_CAP {
                    dimension.name.clone()
                } else {
                    format!("{}:{}", dimension.name, dimension.cap)
                }
            })
            .collect();
        let measures = self
            .measures
            .iter()
            .map(|measure| {
                format!(
                    "{}:{}",
                    measure.column,
                    measure
                        .aggregates
                        .iter()
                        .map(|aggregate| aggregate.name())
                        .collect::<Vec<_>>()
                        .join(",")
                )
            })
            .collect();
        let time = self.time.as_ref().map(|time| {
            if time.cap == time.grain.default_cap() {
                format!("{}:{}", time.column, time.grain.name())
            } else {
                format!("{}:{}:{}", time.column, time.grain.name(), time.cap)
            }
        });
        (dimensions, measures, time)
    }

    /// Every declared column must be a column of the table with a type
    /// its role accepts, and a shape with dimensions or a time dimension
    /// must declare at least one measure (the row count is always kept, so
    /// measures may be empty only when nothing else is).
    pub fn check_against(&self, columns: &[ColumnDefinition]) -> crate::Result<()> {
        let column_type = |name: &str| {
            columns
                .iter()
                .find(|column| column.name() == name)
                .map(ColumnDefinition::data_type)
                .ok_or_else(|| shape_error(format!("'{name}' is not a column of the table")))
        };
        for dimension in &self.dimensions {
            let data_type = column_type(&dimension.name)?;
            if !ShapeDimension::accepts(data_type) {
                return Err(shape_error(format!(
                    "dimension '{}' ({data_type}) cannot be grouped by; dimensions are boolean, integer, text, date, timestamp or decimal columns",
                    dimension.name
                )));
            }
        }
        for measure in &self.measures {
            let data_type = column_type(&measure.column)?;
            for aggregate in &measure.aggregates {
                if !aggregate.accepts(data_type) {
                    return Err(shape_error(format!(
                        "measure '{}' ({data_type}) cannot be kept under {}",
                        measure.column,
                        aggregate.name()
                    )));
                }
            }
        }
        if let Some(time) = &self.time {
            let data_type = column_type(&time.column)?;
            if !ShapeTime::accepts(data_type, time.grain) {
                return Err(shape_error(format!(
                    "time '{}' ({data_type}) cannot be bucketed at {} grain; a date column takes day grain, a microsecond timestamp day or month",
                    time.column,
                    time.grain.name()
                )));
            }
        }
        Ok(())
    }

    /// The axes of the cube: the dimensions in declared order, then the
    /// time dimension.
    pub fn axes(&self) -> Vec<ShapeAxis> {
        let mut axes: Vec<ShapeAxis> = self
            .dimensions
            .iter()
            .map(|dimension| ShapeAxis {
                column: dimension.name.clone(),
                cap: dimension.cap,
                grain: None,
            })
            .collect();
        if let Some(time) = &self.time {
            axes.push(ShapeAxis {
                column: time.column.clone(),
                cap: time.cap,
                grain: Some(time.grain),
            });
        }
        axes
    }

    /// The measure slots of the cube, `(column, aggregate)` in declared
    /// order.
    pub fn measure_slots(&self) -> Vec<(String, MeasureAggregate)> {
        self.measures
            .iter()
            .flat_map(|measure| {
                measure
                    .aggregates
                    .iter()
                    .map(|aggregate| (measure.column.clone(), *aggregate))
            })
            .collect()
    }

    pub fn has_non_additive(&self) -> bool {
        self.measures
            .iter()
            .any(|measure| measure.aggregates.iter().any(|a| !a.is_additive()))
    }

    /// The groupings of the cube over `axes` (indexes into
    /// [`TableShape::axes`]): the grand total, every single axis, and every
    /// pair whose caps multiply to at most [`CUBE_PAIR_MAX_CELLS`].
    /// Deterministic: singles in axis order, pairs in lexical order.
    pub fn groupings(&self) -> Vec<Vec<usize>> {
        let axes = self.axes();
        let mut groupings = vec![Vec::new()];
        for index in 0..axes.len() {
            groupings.push(vec![index]);
        }
        for left in 0..axes.len() {
            for right in left + 1..axes.len() {
                if axes[left].cap.saturating_mul(axes[right].cap) <= CUBE_PAIR_MAX_CELLS {
                    groupings.push(vec![left, right]);
                }
            }
        }
        groupings
    }

    /// The cells the cube is planned at: the caps' products over every
    /// grouping. What the declaration is held to the cell limit by.
    pub fn planned_cells(&self) -> u64 {
        let axes = self.axes();
        self.groupings()
            .iter()
            .map(|grouping| {
                grouping
                    .iter()
                    .fold(1u64, |cells, axis| cells.saturating_mul(axes[*axis].cap))
            })
            .fold(0u64, u64::saturating_add)
    }

    /// The shape held to `max_cells`: refused when its planned cells
    /// exceed it.
    pub fn check_cell_limit(&self, max_cells: u64) -> crate::Result<()> {
        let planned = self.planned_cells();
        if planned > max_cells {
            return Err(shape_error(format!(
                "the shape plans {planned} cube cells, above the limit of {max_cells} (KAVEON_CUBE_MAX_CELLS); lower the dimensions' caps or declare fewer"
            )));
        }
        Ok(())
    }
}

fn logical_type(data_type: &DataType) -> &DataType {
    match data_type {
        DataType::Dictionary(_, values) => values.as_ref(),
        other => other,
    }
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

fn is_float(data_type: &DataType) -> bool {
    matches!(data_type, DataType::Float32 | DataType::Float64)
}

fn is_decimal(data_type: &DataType) -> bool {
    matches!(data_type, DataType::Decimal128(_, _))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn columns() -> Vec<ColumnDefinition> {
        vec![
            ColumnDefinition::new("region", DataType::Utf8, true).unwrap(),
            ColumnDefinition::new("status", DataType::Utf8, true).unwrap(),
            ColumnDefinition::new("total", DataType::Int64, true).unwrap(),
            ColumnDefinition::new("user_id", DataType::Int64, true).unwrap(),
            ColumnDefinition::new("score", DataType::Float64, true).unwrap(),
            ColumnDefinition::new("order_date", DataType::Date32, true).unwrap(),
            ColumnDefinition::new("at", DataType::Timestamp(TimeUnit::Microsecond, None), true)
                .unwrap(),
        ]
    }

    #[test]
    fn a_declaration_parses_renders_and_checks_against_the_columns() {
        let shape = TableShape::parse(
            &["region".into(), "status:50".into()],
            &["total:sum,count".into(), "user_id:count_distinct".into()],
            Some("order_date:day"),
        )
        .unwrap();
        assert_eq!(shape.dimensions[0].cap, DEFAULT_DIMENSION_CAP);
        assert_eq!(shape.dimensions[1].cap, 50);
        assert_eq!(
            shape.measures[0].aggregates,
            vec![MeasureAggregate::Sum, MeasureAggregate::Count]
        );
        assert_eq!(shape.time.as_ref().unwrap().grain, TimeGrain::Day);
        assert_eq!(shape.time.as_ref().unwrap().cap, DEFAULT_DAY_CAP);
        shape.check_against(&columns()).unwrap();
        assert_eq!(
            shape.render(),
            (
                vec!["region".to_owned(), "status:50".to_owned()],
                vec![
                    "total:sum,count".to_owned(),
                    "user_id:count_distinct".to_owned()
                ],
                Some("order_date:day".to_owned())
            )
        );
        let again = TableShape::parse(
            &shape.render().0,
            &shape.render().1,
            shape.render().2.as_deref(),
        )
        .unwrap();
        assert_eq!(again, shape);
        assert!(shape.has_non_additive());
        assert_eq!(
            shape.measure_slots(),
            vec![
                ("total".to_owned(), MeasureAggregate::Sum),
                ("total".to_owned(), MeasureAggregate::Count),
                ("user_id".to_owned(), MeasureAggregate::CountDistinct)
            ]
        );
    }

    #[test]
    fn declarations_are_refused_for_form_columns_and_types() {
        let err = |d: &[&str], m: &[&str], t: Option<&str>| -> String {
            let d: Vec<String> = d.iter().map(|s| (*s).to_owned()).collect();
            let m: Vec<String> = m.iter().map(|s| (*s).to_owned()).collect();
            match TableShape::parse(&d, &m, t) {
                Err(error) => error.to_string(),
                Ok(shape) => shape.check_against(&columns()).unwrap_err().to_string(),
            }
        };
        assert!(err(&["region", "region"], &[], None).contains("declared twice"));
        assert!(err(&["region:0"], &[], None).contains("cap of 0"));
        assert!(err(&[], &["total"], None).contains("column:aggregate"));
        assert!(err(&[], &["total:avg"], None).contains("not one of"));
        assert!(err(&[], &["total:sum", "total:count"], None).contains("declared twice"));
        assert!(err(&[], &[], Some("order_date")).contains("column:grain"));
        assert!(err(&[], &[], Some("order_date:week")).contains("not day or month"));
        assert!(err(&["order_date"], &[], Some("order_date:day")).contains("both a dimension"));
        assert!(err(&["missing"], &[], None).contains("not a column"));
        assert!(err(&["score"], &[], None).contains("cannot be grouped by"));
        assert!(err(&[], &["region:sum"], None).contains("cannot be kept under sum"));
        assert!(err(&[], &[], Some("order_date:month")).contains("day grain"));
        assert!(err(&[], &[], Some("region:day")).contains("cannot be bucketed"));
        TableShape::parse(&[], &[], Some("at:month"))
            .unwrap()
            .check_against(&columns())
            .unwrap();
    }

    #[test]
    fn groupings_pair_axes_within_the_pair_limit_and_plan_cells() {
        let shape = TableShape::parse(
            &["a:10".into(), "b:100".into(), "c:20000".into()],
            &["total:sum".into()],
            Some("at:day:400"),
        )
        .unwrap();
        // a×b, a×time, b×time; c pairs with nothing (20 000 × 100 > limit
        // only with b? 2 000 000 > 1 000 000; with a 200 000 fits).
        assert_eq!(
            shape.groupings(),
            vec![
                vec![],
                vec![0],
                vec![1],
                vec![2],
                vec![3],
                vec![0, 1],
                vec![0, 2],
                vec![0, 3],
                vec![1, 3],
            ]
        );
        assert_eq!(
            shape.planned_cells(),
            1 + 10 + 100 + 20_000 + 400 + 1_000 + 200_000 + 4_000 + 40_000
        );
        assert!(shape.check_cell_limit(300_000).is_ok());
        assert!(
            shape
                .check_cell_limit(200_000)
                .unwrap_err()
                .to_string()
                .contains("KAVEON_CUBE_MAX_CELLS")
        );
    }

    #[test]
    fn time_truncation_matches_date_trunc() {
        // 2026-07-20T13:45:10Z in microseconds.
        let us = (20_654i64 * 86_400 + 13 * 3_600 + 45 * 60 + 10) * 1_000_000 + 123;
        assert_eq!(truncate_micros(us, TimeGrain::Day), 20_654 * 86_400_000_000);
        // 2026-07-01 is day 20 635.
        assert_eq!(
            truncate_micros(us, TimeGrain::Month),
            20_635 * 86_400_000_000
        );
        assert_eq!(
            TimeGrain::Day.truncate(&StatValue::Date(5)),
            Some(StatValue::Date(5))
        );
        assert_eq!(TimeGrain::Month.truncate(&StatValue::Date(5)), None);
    }
}
