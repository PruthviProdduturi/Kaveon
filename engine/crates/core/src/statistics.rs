//! Table statistics: what the engine knows about a table's data at one
//! source version — rows, bytes, files, per-column bounds, null counts,
//! distinct-count and quantile sketches, and per-file bounds for file
//! skipping. A first-class catalog object, versioned by the source version
//! it was computed from and stored beside the table definition.

use crate::sketch::{HllSketch, KllSketch};
use crate::{CompareOp, DataFormat, ScalarValue, StoragePredicate, TableId};
use arrow::datatypes::{DataType, TimeUnit};
use serde::{Deserialize, Serialize};
use std::cmp::Ordering;

/// The format version of a stored statistics document. Versions 1 (a row
/// count) and 2 (the metadata profile as loose JSON) were the product
/// catalog's `ANALYZE` documents; version 3 is this object, stored in the
/// durable catalog beside the table definition.
pub const TABLE_STATISTICS_VERSION: u32 = 3;

/// The most files a statistics document keeps per-file bounds for. Beyond
/// it the table-level facts are still complete and file skipping falls back
/// to the readers' own footer pruning.
pub const MAX_PER_FILE_STATISTICS: usize = 10_000;

/// A statistic value in its logical type, comparable within its variant and
/// renderable as JSON: numbers as numbers, text as text, dates and
/// timestamps as ISO 8601 strings, decimals as their exact decimal text.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum StatValue {
    Int(i128),
    Float(f64),
    Bool(bool),
    Text(String),
    Decimal {
        unscaled: i128,
        scale: i8,
    },
    /// Days since the epoch.
    Date(i32),
    /// Since the epoch in `unit`; `utc` renders a trailing `Z`.
    Timestamp {
        value: i64,
        unit: TimeUnit,
        utc: bool,
    },
    /// Since midnight in `unit`.
    Time {
        value: i64,
        unit: TimeUnit,
    },
}

impl PartialOrd for StatValue {
    /// Ordered within one variant (and one unit or scale); values of
    /// different kinds are incomparable.
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        use StatValue::*;
        match (self, other) {
            (Int(a), Int(b)) => Some(a.cmp(b)),
            (Float(a), Float(b)) => a.partial_cmp(b),
            (Bool(a), Bool(b)) => Some(a.cmp(b)),
            (Text(a), Text(b)) => Some(a.cmp(b)),
            (
                Decimal {
                    unscaled: a,
                    scale: sa,
                },
                Decimal {
                    unscaled: b,
                    scale: sb,
                },
            ) if sa == sb => Some(a.cmp(b)),
            (Date(a), Date(b)) => Some(a.cmp(b)),
            (
                Timestamp {
                    value: a, unit: ua, ..
                },
                Timestamp {
                    value: b, unit: ub, ..
                },
            ) if ua == ub => Some(a.cmp(b)),
            (Time { value: a, unit: ua }, Time { value: b, unit: ub }) if ua == ub => {
                Some(a.cmp(b))
            }
            _ => None,
        }
    }
}

impl StatValue {
    /// The JSON rendering; `null` for a value JSON cannot carry (a NaN, an
    /// out-of-range date).
    pub fn to_json(&self) -> serde_json::Value {
        use arrow::temporal_conversions as t;
        use serde_json::Value;
        match self {
            StatValue::Int(value) => match i64::try_from(*value) {
                Ok(value) => Value::from(value),
                Err(_) => match u64::try_from(*value) {
                    Ok(value) => Value::from(value),
                    Err(_) => Value::String(value.to_string()),
                },
            },
            StatValue::Float(value) => serde_json::Number::from_f64(*value)
                .map(Value::Number)
                .unwrap_or(Value::Null),
            StatValue::Bool(value) => Value::Bool(*value),
            StatValue::Text(value) => Value::String(value.clone()),
            StatValue::Decimal { unscaled, scale } => {
                Value::String(decimal_text(*unscaled, *scale))
            }
            StatValue::Date(days) => t::date32_to_datetime(*days)
                .map(|date| Value::String(date.format("%Y-%m-%d").to_string()))
                .unwrap_or(Value::Null),
            StatValue::Timestamp { value, unit, utc } => {
                use TimeUnit::*;
                let (datetime, fraction) = match unit {
                    Second => (t::timestamp_s_to_datetime(*value), "%Y-%m-%dT%H:%M:%S"),
                    Millisecond => (t::timestamp_ms_to_datetime(*value), "%Y-%m-%dT%H:%M:%S%.3f"),
                    Microsecond => (t::timestamp_us_to_datetime(*value), "%Y-%m-%dT%H:%M:%S%.6f"),
                    Nanosecond => (t::timestamp_ns_to_datetime(*value), "%Y-%m-%dT%H:%M:%S%.9f"),
                };
                datetime
                    .map(|datetime| {
                        let mut text = datetime.format(fraction).to_string();
                        if *utc {
                            text.push('Z');
                        }
                        Value::String(text)
                    })
                    .unwrap_or(Value::Null)
            }
            StatValue::Time { value, unit } => {
                use TimeUnit::*;
                let (time, fraction) = match unit {
                    Second => (
                        i32::try_from(*value).ok().and_then(t::time32s_to_time),
                        "%H:%M:%S",
                    ),
                    Millisecond => (
                        i32::try_from(*value).ok().and_then(t::time32ms_to_time),
                        "%H:%M:%S%.3f",
                    ),
                    Microsecond => (t::time64us_to_time(*value), "%H:%M:%S%.6f"),
                    Nanosecond => (t::time64ns_to_time(*value), "%H:%M:%S%.9f"),
                };
                time.map(|time| Value::String(time.format(fraction).to_string()))
                    .unwrap_or(Value::Null)
            }
        }
    }

    /// The value as a pushed-down predicate literal compares it: integers
    /// and dates as `Int64` (dates as day numbers, the way literals against
    /// date columns are coerced), floats, booleans and text as themselves.
    /// `None` for a kind predicates do not compare (decimals, timestamps,
    /// times, integers beyond 64 bits).
    pub fn to_scalar(&self) -> Option<ScalarValue> {
        match self {
            StatValue::Int(value) => i64::try_from(*value).ok().map(ScalarValue::Int64),
            StatValue::Float(value) => Some(ScalarValue::Float64(*value)),
            StatValue::Bool(value) => Some(ScalarValue::Bool(*value)),
            StatValue::Text(value) => Some(ScalarValue::Utf8(value.clone())),
            StatValue::Date(days) => Some(ScalarValue::Int64(i64::from(*days))),
            StatValue::Decimal { .. } | StatValue::Timestamp { .. } | StatValue::Time { .. } => {
                None
            }
        }
    }

    /// The value on the number line, for quantile sketches and range
    /// selectivity: integers, floats, decimals (scaled), dates (days),
    /// timestamps and times (in their unit). Text and booleans have none.
    pub fn to_f64(&self) -> Option<f64> {
        match self {
            StatValue::Int(value) => Some(*value as f64),
            StatValue::Float(value) => Some(*value),
            StatValue::Decimal { unscaled, scale } => {
                Some(*unscaled as f64 / 10f64.powi(i32::from(*scale)))
            }
            StatValue::Date(days) => Some(f64::from(*days)),
            StatValue::Timestamp { value, .. } | StatValue::Time { value, .. } => {
                Some(*value as f64)
            }
            StatValue::Bool(_) | StatValue::Text(_) => None,
        }
    }

    /// The canonical text a distinct-count sketch hashes (see
    /// [`crate::sketch::hash_text`]).
    pub fn to_hash_text(&self) -> String {
        match self {
            StatValue::Int(value) => value.to_string(),
            StatValue::Float(value) => value.to_string(),
            StatValue::Bool(value) => value.to_string(),
            StatValue::Text(value) => value.clone(),
            StatValue::Decimal { unscaled, scale } => decimal_text(*unscaled, *scale),
            other => match other.to_json() {
                serde_json::Value::String(text) => text,
                other => other.to_string(),
            },
        }
    }
}

/// `unscaled` with the decimal point `scale` digits from the right, exact.
pub fn decimal_text(unscaled: i128, scale: i8) -> String {
    let scale = usize::try_from(scale).unwrap_or(0);
    let negative = unscaled < 0;
    let mut digits = unscaled.unsigned_abs().to_string();
    if scale > 0 {
        if digits.len() <= scale {
            digits = format!("{}{digits}", "0".repeat(scale - digits.len() + 1));
        }
        digits.insert(digits.len() - scale, '.');
    }
    if negative {
        digits.insert(0, '-');
    }
    digits
}

/// What a source version is: the immutable identity the statistics were
/// computed from, and what it names.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SourceVersion {
    /// The source identity digest (`analyze_source`): location, and the
    /// Delta version, Iceberg snapshot, object version or listing digest.
    pub identity_sha256: String,
    #[serde(flatten)]
    pub kind: SourceVersionKind,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum SourceVersionKind {
    DeltaVersion {
        version: u64,
    },
    IcebergSnapshot {
        snapshot_id: Option<i64>,
    },
    /// A directory of Parquet files at one listing.
    Listing {
        files: u64,
    },
    /// One Parquet file at one version.
    File,
}

impl SourceVersion {
    /// A short label naming the version: `delta v12`, `iceberg 8842…`,
    /// `listing of 82 files`, `file`, each with the identity's prefix.
    pub fn label(&self) -> String {
        let prefix = &self.identity_sha256[..self.identity_sha256.len().min(12)];
        match &self.kind {
            SourceVersionKind::DeltaVersion { version } => format!("delta v{version} ({prefix})"),
            SourceVersionKind::IcebergSnapshot {
                snapshot_id: Some(id),
            } => format!("iceberg snapshot {id} ({prefix})"),
            SourceVersionKind::IcebergSnapshot { snapshot_id: None } => {
                format!("iceberg ({prefix})")
            }
            SourceVersionKind::Listing { files } => format!("listing of {files} files ({prefix})"),
            SourceVersionKind::File => format!("file ({prefix})"),
        }
    }
}

/// How much was read to compute the statistics.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StatisticsDepth {
    /// From the source's metadata only: footers, the Delta log, manifests.
    Metadata,
    /// The columns were read once to build the sketches.
    Full,
}

/// One column's facts over the whole table.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ColumnStatistics {
    pub name: String,
    pub data_type: DataType,
    /// `None` is "not recorded", never zero.
    pub null_count: Option<u64>,
    pub min: Option<StatValue>,
    pub max: Option<StatValue>,
    /// Whether `min` and `max` are the column's true extremes rather than
    /// bounds a writer may have truncated (text statistics in Parquet
    /// footers, the Delta log and Iceberg manifests are bounds unless the
    /// writer says otherwise; a full read makes them exact).
    pub bounds_exact: bool,
    /// The distinct-count sketch, when the columns were read.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub distinct: Option<HllSketch>,
    /// An exact distinct count from `ANALYZE … WITH (distinct = true)`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub distinct_exact: Option<u64>,
    /// The quantile sketch, when the columns were read and the type is
    /// numeric or temporal.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub quantiles: Option<KllSketch>,
    /// Compressed bytes of the column across the files, when the footers
    /// were read.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bytes: Option<u64>,
}

impl ColumnStatistics {
    /// The best distinct count on record: exact when counted, else the
    /// sketch's estimate.
    pub fn distinct_count(&self) -> Option<u64> {
        self.distinct_exact
            .or_else(|| self.distinct.as_ref().map(HllSketch::estimate))
    }
}

/// One file's facts: rows, bytes and per-column bounds in the order of
/// [`TableStatistics::columns`].
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct FileStatistics {
    /// The file's path relative to the table location (a Delta add path, a
    /// listing path) or, for Iceberg, the manifest's file path.
    pub path: String,
    pub rows: u64,
    pub bytes: u64,
    pub columns: Vec<FileColumnStatistics>,
}

#[derive(Clone, Debug, PartialEq, Default, Serialize, Deserialize)]
pub struct FileColumnStatistics {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub min: Option<StatValue>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max: Option<StatValue>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub null_count: Option<u64>,
}

impl FileStatistics {
    /// Whether the file may hold a row matching `predicate` (already coerced
    /// for the table's schema), judged from its bounds and null counts:
    /// `false` only when the bounds prove no row can match. Columns not in
    /// `names` and bounds not recorded keep the file.
    pub fn may_match(&self, names: &[String], predicate: &StoragePredicate) -> bool {
        let column = |name: &str| {
            names
                .iter()
                .position(|column| column == name)
                .and_then(|index| self.columns.get(index))
        };
        match predicate {
            StoragePredicate::Compare {
                column: name,
                op,
                value,
            } => column(name).is_none_or(|bounds| {
                bounds_may_match(
                    bounds.min.as_ref().and_then(StatValue::to_scalar),
                    bounds.max.as_ref().and_then(StatValue::to_scalar),
                    *op,
                    value,
                )
            }),
            StoragePredicate::IsNull { column: name } => {
                column(name).is_none_or(|bounds| bounds.null_count != Some(0))
            }
            StoragePredicate::IsNotNull { column: name } => {
                column(name).is_none_or(|bounds| bounds.null_count != Some(self.rows))
            }
            StoragePredicate::In {
                column: name,
                values,
            } => column(name).is_none_or(|bounds| {
                values.iter().any(|value| {
                    bounds_may_match(
                        bounds.min.as_ref().and_then(StatValue::to_scalar),
                        bounds.max.as_ref().and_then(StatValue::to_scalar),
                        CompareOp::Eq,
                        value,
                    )
                })
            }),
            StoragePredicate::And(children) => {
                children.iter().all(|child| self.may_match(names, child))
            }
            StoragePredicate::Or(children) => {
                children.iter().any(|child| self.may_match(names, child))
            }
            // A "may match" cannot be inverted; a pattern says nothing about
            // bounds.
            StoragePredicate::Not(_) | StoragePredicate::Like { .. } => true,
        }
    }
}

/// Whether a range `[min, max]` may hold a value satisfying `op value`;
/// `true` when either bound is unknown or the kinds do not compare.
pub fn bounds_may_match(
    min: Option<ScalarValue>,
    max: Option<ScalarValue>,
    op: CompareOp,
    value: &ScalarValue,
) -> bool {
    let (Some(min), Some(max)) = (min, max) else {
        return true;
    };
    let (Some(min_order), Some(max_order)) = (scalar_cmp(&min, value), scalar_cmp(&max, value))
    else {
        return true;
    };
    match op {
        CompareOp::Eq => min_order != Ordering::Greater && max_order != Ordering::Less,
        CompareOp::Ne => !(min_order == Ordering::Equal && max_order == Ordering::Equal),
        CompareOp::Lt => min_order == Ordering::Less,
        CompareOp::Le => min_order != Ordering::Greater,
        CompareOp::Gt => max_order == Ordering::Greater,
        CompareOp::Ge => max_order != Ordering::Less,
    }
}

fn scalar_cmp(left: &ScalarValue, right: &ScalarValue) -> Option<Ordering> {
    match (left, right) {
        (ScalarValue::Bool(left), ScalarValue::Bool(right)) => left.partial_cmp(right),
        (ScalarValue::Int64(left), ScalarValue::Int64(right)) => left.partial_cmp(right),
        (ScalarValue::Float64(left), ScalarValue::Float64(right)) => left.partial_cmp(right),
        (ScalarValue::Utf8(left), ScalarValue::Utf8(right)) => left.partial_cmp(right),
        _ => None,
    }
}

/// A table's statistics at one source version.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct TableStatistics {
    pub version: u32,
    pub table_id: TableId,
    pub source_version: SourceVersion,
    /// Milliseconds since the epoch.
    pub computed_at_ms: u64,
    pub depth: StatisticsDepth,
    /// The source's format and resolved location when the statistics
    /// were computed.
    pub format: DataFormat,
    pub location: String,
    pub rows: u64,
    /// The data files' bytes as stored.
    pub bytes: u64,
    pub files: u64,
    /// Row groups over every file; unknown when no footer was read.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub row_groups: Option<u64>,
    /// The row groups' uncompressed byte total; unknown when no footer was
    /// read.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub uncompressed_bytes: Option<u64>,
    /// The newest data file's modification time, milliseconds since the
    /// epoch, when the store or log records one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_modified_ms: Option<i64>,
    /// The Delta log's partition columns, or the `key=value` keys of a
    /// partitioned Parquet directory; empty otherwise.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub partition_columns: Vec<String>,
    pub columns: Vec<ColumnStatistics>,
    /// Per-file bounds, complete when `per_file_complete`; empty otherwise.
    #[serde(default)]
    pub per_file: Vec<FileStatistics>,
    #[serde(default)]
    pub per_file_complete: bool,
}

impl TableStatistics {
    pub fn column(&self, name: &str) -> Option<&ColumnStatistics> {
        self.columns.iter().find(|column| column.name == name)
    }

    pub fn column_names(&self) -> Vec<String> {
        self.columns
            .iter()
            .map(|column| column.name.clone())
            .collect()
    }

    /// Whether these statistics describe `identity`: the same source
    /// version, so they may answer rather than only cost.
    pub fn is_current_for(&self, identity_sha256: &str) -> bool {
        self.source_version.identity_sha256 == identity_sha256
    }

    /// The average row width in bytes, when known.
    pub fn bytes_per_row(&self) -> Option<f64> {
        (self.rows > 0).then(|| self.bytes as f64 / self.rows as f64)
    }

    /// The files whose bounds admit `predicate`, and the ones proven empty
    /// of matches. Only meaningful when `per_file_complete`.
    pub fn partition_files(&self, predicate: &StoragePredicate) -> (Vec<&FileStatistics>, usize) {
        let names = self.column_names();
        let mut kept = Vec::with_capacity(self.per_file.len());
        let mut skipped = 0;
        for file in &self.per_file {
            if file.may_match(&names, predicate) {
                kept.push(file);
            } else {
                skipped += 1;
            }
        }
        (kept, skipped)
    }

    /// Fold files that were added to the source in: rows, bytes, files and
    /// null counts add, bounds widen, sketches merge when both sides have
    /// them (and are dropped when the added files bring none, since a
    /// sketch that misses rows would under-count). Exact distinct counts
    /// do not survive an addition. The caller sets the new source version.
    pub fn append_files(
        &mut self,
        added: Vec<FileStatistics>,
        added_sketches: Option<Vec<ColumnSketches>>,
    ) -> crate::Result<()> {
        for file in &added {
            if file.columns.len() != self.columns.len() {
                return Err(crate::KaveonError::Execution(format!(
                    "file '{}' carries {} column entries for {} columns",
                    file.path,
                    file.columns.len(),
                    self.columns.len()
                )));
            }
            self.rows = self.rows.saturating_add(file.rows);
            self.bytes = self.bytes.saturating_add(file.bytes);
            self.files = self.files.saturating_add(1);
            for (column, bounds) in self.columns.iter_mut().zip(&file.columns) {
                column.distinct_exact = None;
                column.null_count = match (column.null_count, bounds.null_count) {
                    (Some(a), Some(b)) => Some(a.saturating_add(b)),
                    _ => None,
                };
                column.bytes = None;
                if file.rows == 0 {
                    continue;
                }
                let all_null = bounds.null_count == Some(file.rows);
                if all_null && bounds.min.is_none() && bounds.max.is_none() {
                    continue;
                }
                column.min = widen(column.min.take(), bounds.min.clone(), Ordering::Less);
                column.max = widen(column.max.take(), bounds.max.clone(), Ordering::Greater);
            }
        }
        match added_sketches {
            Some(sketches) => {
                for sketches in sketches {
                    for (column, added) in self.columns.iter_mut().zip(sketches.columns) {
                        match (&mut column.distinct, added.distinct) {
                            (Some(mine), Some(theirs)) => mine.merge(&theirs)?,
                            (mine, _) => *mine = None,
                        }
                        match (&mut column.quantiles, added.quantiles) {
                            (Some(mine), Some(theirs)) => mine.merge(&theirs)?,
                            (mine, _) => *mine = None,
                        }
                    }
                }
            }
            None => {
                for column in &mut self.columns {
                    column.distinct = None;
                    column.quantiles = None;
                }
                if self.depth == StatisticsDepth::Full {
                    self.depth = StatisticsDepth::Metadata;
                }
            }
        }
        if self.per_file_complete {
            if self.per_file.len() + added.len() <= MAX_PER_FILE_STATISTICS {
                self.per_file.extend(added);
            } else {
                self.per_file.clear();
                self.per_file_complete = false;
            }
        }
        Ok(())
    }

    pub fn to_json_bytes(&self) -> crate::Result<Vec<u8>> {
        serde_json::to_vec(self)
            .map_err(|error| crate::KaveonError::Execution(format!("statistics encode: {error}")))
    }

    pub fn from_json_bytes(bytes: &[u8]) -> crate::Result<Self> {
        let value: Self = serde_json::from_slice(bytes).map_err(|error| {
            crate::KaveonError::Execution(format!("statistics decode: {error}"))
        })?;
        if value.version != TABLE_STATISTICS_VERSION {
            return Err(crate::KaveonError::Execution(format!(
                "statistics document version {} is not {TABLE_STATISTICS_VERSION}",
                value.version
            )));
        }
        Ok(value)
    }
}

/// The sketches of one file's columns, in the table's column order.
#[derive(Clone, Debug, Default)]
pub struct ColumnSketches {
    pub columns: Vec<FileColumnSketches>,
}

#[derive(Clone, Debug, Default)]
pub struct FileColumnSketches {
    pub distinct: Option<HllSketch>,
    pub quantiles: Option<KllSketch>,
}

/// A bound widened by another: unknown once either side is unknown or the
/// two do not compare.
pub fn widen(
    current: Option<StatValue>,
    other: Option<StatValue>,
    prefer: Ordering,
) -> Option<StatValue> {
    let (current, other) = (current?, other?);
    match current.partial_cmp(&other) {
        Some(order) if order == prefer => Some(current),
        Some(_) => Some(other),
        None => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn stats() -> TableStatistics {
        TableStatistics {
            version: TABLE_STATISTICS_VERSION,
            table_id: TableId::new("table:t").unwrap(),
            source_version: SourceVersion {
                identity_sha256: "abc".into(),
                kind: SourceVersionKind::Listing { files: 2 },
            },
            computed_at_ms: 1,
            depth: StatisticsDepth::Metadata,
            format: DataFormat::Parquet,
            location: "/lake/t".into(),
            rows: 10,
            bytes: 1000,
            files: 2,
            row_groups: Some(2),
            uncompressed_bytes: Some(1500),
            last_modified_ms: Some(1),
            partition_columns: Vec::new(),
            columns: vec![
                ColumnStatistics {
                    name: "id".into(),
                    data_type: DataType::Int64,
                    null_count: Some(0),
                    min: Some(StatValue::Int(1)),
                    max: Some(StatValue::Int(10)),
                    bounds_exact: true,
                    distinct: None,
                    distinct_exact: None,
                    quantiles: None,
                    bytes: Some(400),
                },
                ColumnStatistics {
                    name: "name".into(),
                    data_type: DataType::Utf8,
                    null_count: Some(2),
                    min: Some(StatValue::Text("apple".into())),
                    max: Some(StatValue::Text("pear".into())),
                    bounds_exact: false,
                    distinct: None,
                    distinct_exact: None,
                    quantiles: None,
                    bytes: Some(600),
                },
            ],
            per_file: vec![
                FileStatistics {
                    path: "a.parquet".into(),
                    rows: 4,
                    bytes: 400,
                    columns: vec![
                        FileColumnStatistics {
                            min: Some(StatValue::Int(1)),
                            max: Some(StatValue::Int(4)),
                            null_count: Some(0),
                        },
                        FileColumnStatistics {
                            min: Some(StatValue::Text("apple".into())),
                            max: Some(StatValue::Text("fig".into())),
                            null_count: Some(0),
                        },
                    ],
                },
                FileStatistics {
                    path: "b.parquet".into(),
                    rows: 6,
                    bytes: 600,
                    columns: vec![
                        FileColumnStatistics {
                            min: Some(StatValue::Int(5)),
                            max: Some(StatValue::Int(10)),
                            null_count: Some(0),
                        },
                        FileColumnStatistics {
                            min: Some(StatValue::Text("kiwi".into())),
                            max: Some(StatValue::Text("pear".into())),
                            null_count: Some(2),
                        },
                    ],
                },
            ],
            per_file_complete: true,
        }
    }

    fn compare(column: &str, op: CompareOp, value: ScalarValue) -> StoragePredicate {
        StoragePredicate::Compare {
            column: column.into(),
            op,
            value,
        }
    }

    #[test]
    fn files_are_skipped_only_when_their_bounds_prove_no_match() {
        let stats = stats();
        let files = |predicate: StoragePredicate| {
            let (kept, skipped) = stats.partition_files(&predicate);
            (
                kept.iter()
                    .map(|file| file.path.as_str())
                    .collect::<Vec<_>>(),
                skipped,
            )
        };
        assert_eq!(
            files(compare("id", CompareOp::Gt, ScalarValue::Int64(4))),
            (vec!["b.parquet"], 1)
        );
        assert_eq!(
            files(compare("id", CompareOp::Ge, ScalarValue::Int64(4))),
            (vec!["a.parquet", "b.parquet"], 0)
        );
        assert_eq!(
            files(compare("id", CompareOp::Eq, ScalarValue::Int64(11))),
            (Vec::<&str>::new(), 2)
        );
        assert_eq!(
            files(compare("id", CompareOp::Ne, ScalarValue::Int64(11))),
            (vec!["a.parquet", "b.parquet"], 0)
        );
        assert_eq!(
            files(compare(
                "name",
                CompareOp::Lt,
                ScalarValue::Utf8("banana".into())
            )),
            (vec!["a.parquet"], 1)
        );
        assert_eq!(
            files(StoragePredicate::IsNull {
                column: "name".into()
            }),
            (vec!["b.parquet"], 1)
        );
        assert_eq!(
            files(StoragePredicate::In {
                column: "id".into(),
                values: vec![ScalarValue::Int64(2), ScalarValue::Int64(3)]
            }),
            (vec!["a.parquet"], 1)
        );
        assert_eq!(
            files(StoragePredicate::Or(vec![
                compare("id", CompareOp::Lt, ScalarValue::Int64(2)),
                compare("id", CompareOp::Gt, ScalarValue::Int64(9)),
            ])),
            (vec!["a.parquet", "b.parquet"], 0)
        );
        assert_eq!(
            files(StoragePredicate::And(vec![
                compare("id", CompareOp::Gt, ScalarValue::Int64(4)),
                compare("name", CompareOp::Lt, ScalarValue::Utf8("banana".into())),
            ])),
            (Vec::<&str>::new(), 2)
        );
        // Unknown columns, mismatched kinds, NOT and LIKE keep every file.
        assert_eq!(
            files(compare("other", CompareOp::Eq, ScalarValue::Int64(1))).1,
            0
        );
        assert_eq!(
            files(compare("id", CompareOp::Eq, ScalarValue::Utf8("x".into()))).1,
            0
        );
        assert_eq!(
            files(StoragePredicate::Not(Box::new(compare(
                "id",
                CompareOp::Gt,
                ScalarValue::Int64(4)
            ))))
            .1,
            0
        );
    }

    #[test]
    fn appending_files_widens_bounds_adds_counts_and_drops_what_it_cannot_keep() {
        let mut stats = stats();
        stats.columns[0].distinct_exact = Some(10);
        let mut sketch = HllSketch::default_precision();
        sketch.insert_text("1");
        stats.columns[0].distinct = Some(sketch.clone());
        stats
            .append_files(
                vec![FileStatistics {
                    path: "c.parquet".into(),
                    rows: 3,
                    bytes: 300,
                    columns: vec![
                        FileColumnStatistics {
                            min: Some(StatValue::Int(-5)),
                            max: Some(StatValue::Int(2)),
                            null_count: Some(1),
                        },
                        FileColumnStatistics {
                            min: None,
                            max: Some(StatValue::Text("zucchini".into())),
                            null_count: None,
                        },
                    ],
                }],
                Some(vec![ColumnSketches {
                    columns: vec![
                        FileColumnSketches {
                            distinct: Some({
                                let mut other = HllSketch::default_precision();
                                other.insert_text("2");
                                other
                            }),
                            quantiles: None,
                        },
                        FileColumnSketches::default(),
                    ],
                }]),
            )
            .unwrap();
        assert_eq!((stats.rows, stats.bytes, stats.files), (13, 1300, 3));
        assert_eq!(stats.columns[0].min, Some(StatValue::Int(-5)));
        assert_eq!(stats.columns[0].max, Some(StatValue::Int(10)));
        assert_eq!(stats.columns[0].null_count, Some(1));
        assert_eq!(stats.columns[0].distinct_exact, None);
        assert_eq!(stats.columns[0].distinct.as_ref().unwrap().estimate(), 2);
        assert_eq!(stats.columns[1].min, None);
        assert_eq!(
            stats.columns[1].max,
            Some(StatValue::Text("zucchini".into()))
        );
        assert_eq!(stats.columns[1].null_count, None);
        assert_eq!(stats.per_file.len(), 3);
        assert!(stats.per_file_complete);

        let bytes = stats.to_json_bytes().unwrap();
        assert_eq!(TableStatistics::from_json_bytes(&bytes).unwrap(), stats);
        assert!(
            stats
                .append_files(
                    vec![FileStatistics {
                        path: "bad".into(),
                        rows: 0,
                        bytes: 0,
                        columns: vec![]
                    }],
                    None
                )
                .is_err()
        );
    }

    #[test]
    fn stat_values_render_convert_and_compare() {
        assert_eq!(decimal_text(-1234, 2), "-12.34");
        assert_eq!(decimal_text(5, 3), "0.005");
        assert_eq!(
            StatValue::Date(0).to_json(),
            serde_json::json!("1970-01-01")
        );
        assert_eq!(
            StatValue::Date(20_654).to_scalar(),
            Some(ScalarValue::Int64(20_654))
        );
        assert_eq!(
            StatValue::Decimal {
                unscaled: 125,
                scale: 2
            }
            .to_f64(),
            Some(1.25)
        );
        assert_eq!(
            StatValue::Timestamp {
                value: 1_000,
                unit: TimeUnit::Millisecond,
                utc: true
            }
            .to_json(),
            serde_json::json!("1970-01-01T00:00:01.000Z")
        );
        assert!(StatValue::Int(1) < StatValue::Int(2));
        assert_eq!(
            StatValue::Int(1).partial_cmp(&StatValue::Text("a".into())),
            None
        );
        let label = SourceVersion {
            identity_sha256: "0123456789abcdef".into(),
            kind: SourceVersionKind::DeltaVersion { version: 3 },
        }
        .label();
        assert_eq!(label, "delta v3 (0123456789ab)");
    }
}
