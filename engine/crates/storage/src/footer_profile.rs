//! Column facts a Parquet footer carries — column-chunk statistics and
//! sizes — merged over row groups and, by the readers that combine files,
//! over the files of a table. No data page is read.

use parquet::{
    basic::{ConvertedType, LogicalType, TimeUnit, Type as PhysicalType},
    file::{metadata::ParquetMetaData, statistics::Statistics},
};
use std::{cmp::Ordering, collections::BTreeMap};

/// One column's merged facts. `min`/`max`/`nulls` are `None` whenever any
/// row group or file lacks them — a bound that is not known for every
/// chunk is not a bound.
#[derive(Clone, Debug, PartialEq)]
pub struct ColumnProfile {
    /// The root column's name in the file schema.
    pub name: String,
    /// The Parquet field id of the root column, when the file carries one
    /// (Iceberg maps columns by id, not name).
    pub field_id: Option<i32>,
    pub nulls: Option<u64>,
    pub min: Option<StatValue>,
    pub max: Option<StatValue>,
    /// Compressed bytes of every column chunk under the root column.
    pub compressed_bytes: Option<u64>,
}

/// The facts of all root columns plus the file-level totals, merged in the
/// same file order the readers use. The footer supplies the rows, row
/// groups, uncompressed bytes and column facts; the file's listing or
/// `HEAD` supplies its size and modification time.
#[derive(Clone, Debug, PartialEq, Default)]
pub struct FooterProfile {
    pub file_count: u64,
    pub row_count: u64,
    pub row_group_count: u64,
    /// The files' sizes as stored.
    pub file_bytes: u64,
    pub uncompressed_bytes: u64,
    /// The newest file's modification time, milliseconds since the epoch.
    pub last_modified_ms: Option<i64>,
    pub columns: Vec<ColumnProfile>,
}

/// A statistic value in its logical type, comparable within its variant and
/// renderable as JSON: numbers as numbers, text as text, dates and
/// timestamps as ISO 8601 strings, decimals as their exact decimal text.
#[derive(Clone, Debug, PartialEq)]
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
        unit: arrow::datatypes::TimeUnit,
        utc: bool,
    },
    /// Since midnight in `unit`.
    Time {
        value: i64,
        unit: arrow::datatypes::TimeUnit,
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
                use arrow::datatypes::TimeUnit::*;
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
                use arrow::datatypes::TimeUnit::*;
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

/// A bound merged over chunks: unknown once any chunk cannot supply it.
#[derive(Clone, Debug, Default)]
struct Bound {
    value: Option<StatValue>,
    unknown: bool,
}

impl Bound {
    fn fold(&mut self, value: Option<StatValue>, prefer: Ordering) {
        if self.unknown {
            return;
        }
        let Some(value) = value else {
            self.unknown = true;
            self.value = None;
            return;
        };
        match self.value.take() {
            None => self.value = Some(value),
            Some(current) => match current.partial_cmp(&value) {
                Some(order) if order == prefer => self.value = Some(current),
                Some(_) => self.value = Some(value),
                None => self.unknown = true,
            },
        }
    }
}

#[derive(Clone, Debug, Default)]
struct Accumulator {
    field_id: Option<i32>,
    nulls: Option<u64>,
    nulls_unknown: bool,
    min: Bound,
    max: Bound,
    compressed_bytes: u64,
    /// A root column with more than one leaf (a struct, a list) has sizes
    /// but no scalar bounds.
    nested: bool,
}

impl Accumulator {
    fn finish(self, name: String) -> ColumnProfile {
        ColumnProfile {
            name,
            field_id: self.field_id,
            nulls: if self.nested || self.nulls_unknown {
                None
            } else {
                self.nulls
            },
            min: if self.nested { None } else { self.min.value },
            max: if self.nested { None } else { self.max.value },
            compressed_bytes: Some(self.compressed_bytes),
        }
    }
}

impl FooterProfile {
    /// The profile of one file from its footer, root columns in schema
    /// order; the file's size and modification time come from the caller.
    pub fn from_parquet(
        metadata: &ParquetMetaData,
        file_bytes: u64,
        last_modified_ms: Option<i64>,
    ) -> FooterProfile {
        let descriptor = metadata.file_metadata().schema_descr();
        let roots = descriptor.root_schema().get_fields();
        let mut order = Vec::with_capacity(roots.len());
        let mut columns: BTreeMap<String, Accumulator> = BTreeMap::new();
        for root in roots {
            let info = root.get_basic_info();
            let name = info.name().to_owned();
            let leaves = if root.is_primitive() { 1 } else { usize::MAX };
            columns.insert(
                name.clone(),
                Accumulator {
                    field_id: info.has_id().then(|| info.id()),
                    nested: leaves != 1,
                    ..Default::default()
                },
            );
            order.push(name);
        }
        let mut uncompressed_bytes = 0u64;
        for group in metadata.row_groups() {
            uncompressed_bytes = uncompressed_bytes
                .saturating_add(u64::try_from(group.total_byte_size()).unwrap_or(0));
            for (leaf, chunk) in descriptor.columns().iter().zip(group.columns()) {
                let Some(root) = leaf.path().parts().first() else {
                    continue;
                };
                let Some(column) = columns.get_mut(root) else {
                    continue;
                };
                column.compressed_bytes = column
                    .compressed_bytes
                    .saturating_add(u64::try_from(chunk.compressed_size()).unwrap_or(0));
                if column.nested {
                    continue;
                }
                let Some(statistics) = chunk.statistics() else {
                    column.nulls_unknown = true;
                    column.nulls = None;
                    column.min.fold(None, Ordering::Less);
                    column.max.fold(None, Ordering::Greater);
                    continue;
                };
                match statistics.null_count_opt() {
                    Some(nulls) if !column.nulls_unknown => {
                        column.nulls = Some(column.nulls.unwrap_or(0).saturating_add(nulls));
                    }
                    _ => {
                        column.nulls_unknown = true;
                        column.nulls = None;
                    }
                }
                // A chunk whose values are all null has no bounds to
                // contribute and does not make the column's unknown.
                let all_null = statistics.null_count_opt().is_some_and(|nulls| {
                    chunk.num_values() >= 0 && nulls == chunk.num_values() as u64
                });
                if all_null
                    && statistics.min_bytes_opt().is_none()
                    && statistics.max_bytes_opt().is_none()
                {
                    continue;
                }
                let (min, max) = bounds(statistics, leaf.as_ref());
                column.min.fold(min, Ordering::Less);
                column.max.fold(max, Ordering::Greater);
            }
        }
        FooterProfile {
            file_count: 1,
            row_count: u64::try_from(metadata.file_metadata().num_rows()).unwrap_or(0),
            row_group_count: metadata.num_row_groups() as u64,
            file_bytes,
            uncompressed_bytes,
            last_modified_ms,
            columns: order
                .into_iter()
                .filter_map(|name| columns.remove(&name).map(|column| column.finish(name)))
                .collect(),
        }
    }

    /// Fold another file's profile in: bytes and null counts add, bounds
    /// widen, and a fact one side does not know becomes unknown. A file
    /// without rows contributes its bytes only. Columns are matched by name.
    pub fn merge(&mut self, other: FooterProfile) {
        self.file_count = self.file_count.saturating_add(other.file_count);
        self.row_group_count = self.row_group_count.saturating_add(other.row_group_count);
        self.file_bytes = self.file_bytes.saturating_add(other.file_bytes);
        self.uncompressed_bytes = self
            .uncompressed_bytes
            .saturating_add(other.uncompressed_bytes);
        self.last_modified_ms = self.last_modified_ms.max(other.last_modified_ms);
        let other_rows = other.row_count;
        let mut others: BTreeMap<String, ColumnProfile> = other
            .columns
            .into_iter()
            .map(|column| (column.name.clone(), column))
            .collect();
        for column in &mut self.columns {
            let Some(other) = others.remove(&column.name) else {
                column.nulls = None;
                column.min = None;
                column.max = None;
                column.compressed_bytes = None;
                continue;
            };
            column.compressed_bytes = column
                .compressed_bytes
                .zip(other.compressed_bytes)
                .map(|(a, b)| a.saturating_add(b));
            if other_rows == 0 {
                continue;
            }
            if self.row_count == 0 {
                column.nulls = other.nulls;
                column.min = other.min;
                column.max = other.max;
                continue;
            }
            column.nulls = column
                .nulls
                .zip(other.nulls)
                .map(|(a, b)| a.saturating_add(b));
            let mut min = Bound {
                value: column.min.take(),
                unknown: false,
            };
            min.fold(other.min, Ordering::Less);
            column.min = min.value;
            let mut max = Bound {
                value: column.max.take(),
                unknown: false,
            };
            max.fold(other.max, Ordering::Greater);
            column.max = max.value;
        }
        self.row_count = self.row_count.saturating_add(other_rows);
    }
}

/// The chunk's min and max in the column's logical type; `None` for a bound
/// the chunk does not carry, carries in a deprecated byte-ordered form, or
/// carries in a type this profile does not render (INT96, raw binary).
fn bounds(
    statistics: &Statistics,
    leaf: &parquet::schema::types::ColumnDescriptor,
) -> (Option<StatValue>, Option<StatValue>) {
    let logical = leaf.logical_type();
    let converted = leaf.converted_type();
    let scale = i8::try_from(leaf.type_scale()).ok();
    let decimal = |unscaled: i128| scale.map(|scale| StatValue::Decimal { unscaled, scale });
    let integer = |raw: i128, bits: u32| -> StatValue {
        let unsigned = match &logical {
            Some(LogicalType::Integer { is_signed, .. }) => !is_signed,
            _ => matches!(
                converted,
                ConvertedType::UINT_8
                    | ConvertedType::UINT_16
                    | ConvertedType::UINT_32
                    | ConvertedType::UINT_64
            ),
        };
        if unsigned {
            let mask = if bits >= 128 {
                i128::MAX
            } else {
                (1i128 << bits) - 1
            };
            StatValue::Int(raw & mask)
        } else {
            StatValue::Int(raw)
        }
    };
    let timestamp = |value: i64| -> Option<StatValue> {
        let (unit, utc) = match (&logical, converted) {
            (
                Some(LogicalType::Timestamp {
                    unit,
                    is_adjusted_to_u_t_c,
                }),
                _,
            ) => (arrow_unit(unit), *is_adjusted_to_u_t_c),
            (_, ConvertedType::TIMESTAMP_MILLIS) => (arrow::datatypes::TimeUnit::Millisecond, true),
            (_, ConvertedType::TIMESTAMP_MICROS) => (arrow::datatypes::TimeUnit::Microsecond, true),
            _ => return None,
        };
        Some(StatValue::Timestamp { value, unit, utc })
    };
    let time = |value: i64| -> Option<StatValue> {
        let unit = match (&logical, converted) {
            (Some(LogicalType::Time { unit, .. }), _) => arrow_unit(unit),
            (_, ConvertedType::TIME_MILLIS) => arrow::datatypes::TimeUnit::Millisecond,
            (_, ConvertedType::TIME_MICROS) => arrow::datatypes::TimeUnit::Microsecond,
            _ => return None,
        };
        Some(StatValue::Time { value, unit })
    };
    let is_decimal =
        matches!(logical, Some(LogicalType::Decimal { .. })) || converted == ConvertedType::DECIMAL;
    let is_date = matches!(logical, Some(LogicalType::Date)) || converted == ConvertedType::DATE;
    let is_timestamp = matches!(logical, Some(LogicalType::Timestamp { .. }))
        || matches!(
            converted,
            ConvertedType::TIMESTAMP_MILLIS | ConvertedType::TIMESTAMP_MICROS
        );
    let is_time = matches!(logical, Some(LogicalType::Time { .. }))
        || matches!(
            converted,
            ConvertedType::TIME_MILLIS | ConvertedType::TIME_MICROS
        );
    let is_text = matches!(
        logical,
        Some(LogicalType::String | LogicalType::Enum | LogicalType::Json)
    ) || matches!(
        converted,
        ConvertedType::UTF8 | ConvertedType::ENUM | ConvertedType::JSON
    );
    let convert = |raw: Raw| -> Option<StatValue> {
        match raw {
            Raw::Bool(value) => Some(StatValue::Bool(value)),
            Raw::I32(value) if is_decimal => decimal(i128::from(value)),
            Raw::I32(value) if is_date => Some(StatValue::Date(value)),
            Raw::I32(value) if is_time => time(i64::from(value)),
            Raw::I32(value) => Some(integer(i128::from(value), 32)),
            Raw::I64(value) if is_decimal => decimal(i128::from(value)),
            Raw::I64(value) if is_timestamp => timestamp(value),
            Raw::I64(value) if is_time => time(value),
            Raw::I64(value) => Some(integer(i128::from(value), 64)),
            Raw::F64(value) => Some(StatValue::Float(value)),
            Raw::Bytes(bytes) if is_decimal => decimal(big_endian_i128(bytes)?),
            Raw::Bytes(bytes) if is_text => std::str::from_utf8(bytes)
                .ok()
                .map(|text| StatValue::Text(text.to_owned())),
            Raw::Bytes(_) => None,
        }
    };
    if leaf.physical_type() == PhysicalType::INT96
        || (matches!(
            leaf.physical_type(),
            PhysicalType::BYTE_ARRAY | PhysicalType::FIXED_LEN_BYTE_ARRAY
        ) && statistics.is_min_max_deprecated())
    {
        return (None, None);
    }
    let (min, max) = raw_bounds(statistics);
    (min.and_then(convert), max.and_then(convert))
}

enum Raw<'a> {
    Bool(bool),
    I32(i32),
    I64(i64),
    F64(f64),
    Bytes(&'a [u8]),
}

fn raw_bounds(statistics: &Statistics) -> (Option<Raw<'_>>, Option<Raw<'_>>) {
    match statistics {
        Statistics::Boolean(s) => (
            s.min_opt().map(|v| Raw::Bool(*v)),
            s.max_opt().map(|v| Raw::Bool(*v)),
        ),
        Statistics::Int32(s) => (
            s.min_opt().map(|v| Raw::I32(*v)),
            s.max_opt().map(|v| Raw::I32(*v)),
        ),
        Statistics::Int64(s) => (
            s.min_opt().map(|v| Raw::I64(*v)),
            s.max_opt().map(|v| Raw::I64(*v)),
        ),
        Statistics::Int96(_) => (None, None),
        Statistics::Float(s) => (
            s.min_opt().map(|v| Raw::F64(f64::from(*v))),
            s.max_opt().map(|v| Raw::F64(f64::from(*v))),
        ),
        Statistics::Double(s) => (
            s.min_opt().map(|v| Raw::F64(*v)),
            s.max_opt().map(|v| Raw::F64(*v)),
        ),
        Statistics::ByteArray(s) => (
            s.min_opt().map(|v| Raw::Bytes(v.data())),
            s.max_opt().map(|v| Raw::Bytes(v.data())),
        ),
        Statistics::FixedLenByteArray(s) => (
            s.min_opt().map(|v| Raw::Bytes(v.data())),
            s.max_opt().map(|v| Raw::Bytes(v.data())),
        ),
    }
}

fn arrow_unit(unit: &TimeUnit) -> arrow::datatypes::TimeUnit {
    match unit {
        TimeUnit::MILLIS(_) => arrow::datatypes::TimeUnit::Millisecond,
        TimeUnit::MICROS(_) => arrow::datatypes::TimeUnit::Microsecond,
        TimeUnit::NANOS(_) => arrow::datatypes::TimeUnit::Nanosecond,
    }
}

/// A two's-complement big-endian integer of up to 16 bytes.
fn big_endian_i128(bytes: &[u8]) -> Option<i128> {
    if bytes.is_empty() || bytes.len() > 16 {
        return None;
    }
    let fill = if bytes[0] & 0x80 != 0 { 0xff } else { 0x00 };
    let mut buffer = [fill; 16];
    buffer[16 - bytes.len()..].copy_from_slice(bytes);
    Some(i128::from_be_bytes(buffer))
}

#[cfg(test)]
mod tests {
    use super::*;
    use arrow::{
        array::{
            ArrayRef, Date32Array, Decimal128Array, Int64Array, StringArray,
            TimestampMicrosecondArray,
        },
        datatypes::{DataType, Field, Schema},
        record_batch::RecordBatch,
    };
    use parquet::{arrow::ArrowWriter, file::properties::WriterProperties};
    use std::sync::Arc;

    fn profile_of(batch: RecordBatch, row_group_size: usize) -> FooterProfile {
        let properties = WriterProperties::builder()
            .set_max_row_group_size(row_group_size)
            .build();
        let mut writer =
            ArrowWriter::try_new(Vec::new(), batch.schema(), Some(properties)).unwrap();
        writer.write(&batch).unwrap();
        let bytes = writer.into_inner().unwrap();
        let reader =
            parquet::file::reader::SerializedFileReader::new(bytes::Bytes::from(bytes)).unwrap();
        use parquet::file::reader::FileReader;
        FooterProfile::from_parquet(reader.metadata(), 0, None)
    }

    #[test]
    fn bounds_and_null_counts_merge_across_row_groups() {
        let schema = Arc::new(Schema::new(vec![
            Field::new("id", DataType::Int64, false),
            Field::new("name", DataType::Utf8, true),
            Field::new("day", DataType::Date32, true),
            Field::new(
                "at",
                DataType::Timestamp(arrow::datatypes::TimeUnit::Microsecond, Some("UTC".into())),
                true,
            ),
            Field::new("amount", DataType::Decimal128(10, 2), true),
        ]));
        let batch = RecordBatch::try_new(
            schema,
            vec![
                Arc::new(Int64Array::from(vec![5, 1, 9, 3])) as ArrayRef,
                Arc::new(StringArray::from(vec![
                    Some("pear"),
                    None,
                    Some("apple"),
                    None,
                ])) as ArrayRef,
                Arc::new(Date32Array::from(vec![
                    Some(19_000),
                    None,
                    Some(18_000),
                    Some(19_500),
                ])) as ArrayRef,
                Arc::new(
                    TimestampMicrosecondArray::from(vec![
                        Some(1_600_000_000_000_000),
                        Some(1_700_000_000_000_000),
                        None,
                        Some(1_650_000_000_000_000),
                    ])
                    .with_timezone("UTC"),
                ) as ArrayRef,
                Arc::new(
                    Decimal128Array::from(vec![Some(1_050), Some(-25), None, Some(99_999)])
                        .with_precision_and_scale(10, 2)
                        .unwrap(),
                ) as ArrayRef,
            ],
        )
        .unwrap();
        let profile = profile_of(batch, 2);
        assert!(profile.uncompressed_bytes > 0);
        let by_name = |name: &str| {
            profile
                .columns
                .iter()
                .find(|column| column.name == name)
                .unwrap()
                .clone()
        };
        let id = by_name("id");
        assert_eq!(id.nulls, Some(0));
        assert_eq!(id.min, Some(StatValue::Int(1)));
        assert_eq!(id.max, Some(StatValue::Int(9)));
        assert!(id.compressed_bytes.unwrap() > 0);
        let name = by_name("name");
        assert_eq!(name.nulls, Some(2));
        assert_eq!(name.min.unwrap().to_json(), serde_json::json!("apple"));
        assert_eq!(name.max.unwrap().to_json(), serde_json::json!("pear"));
        let day = by_name("day");
        assert_eq!(day.nulls, Some(1));
        assert_eq!(day.min.unwrap().to_json(), serde_json::json!("2019-04-14"));
        assert_eq!(day.max.unwrap().to_json(), serde_json::json!("2023-05-23"));
        let at = by_name("at");
        assert_eq!(
            at.min.unwrap().to_json(),
            serde_json::json!("2020-09-13T12:26:40.000000Z")
        );
        assert_eq!(
            at.max.unwrap().to_json(),
            serde_json::json!("2023-11-14T22:13:20.000000Z")
        );
        let amount = by_name("amount");
        assert_eq!(amount.min.unwrap().to_json(), serde_json::json!("-0.25"));
        assert_eq!(amount.max.unwrap().to_json(), serde_json::json!("999.99"));
    }

    #[test]
    fn merging_files_widens_bounds_and_adds_counts() {
        let schema = Arc::new(Schema::new(vec![Field::new("id", DataType::Int64, true)]));
        let first = profile_of(
            RecordBatch::try_new(
                schema.clone(),
                vec![Arc::new(Int64Array::from(vec![Some(4), None])) as ArrayRef],
            )
            .unwrap(),
            10,
        );
        let second = profile_of(
            RecordBatch::try_new(
                schema,
                vec![Arc::new(Int64Array::from(vec![Some(-3), Some(12)])) as ArrayRef],
            )
            .unwrap(),
            10,
        );
        let mut merged = first.clone();
        merged.merge(second.clone());
        assert_eq!(
            merged.uncompressed_bytes,
            first.uncompressed_bytes + second.uncompressed_bytes
        );
        let id = &merged.columns[0];
        assert_eq!(id.nulls, Some(1));
        assert_eq!(id.min, Some(StatValue::Int(-3)));
        assert_eq!(id.max, Some(StatValue::Int(12)));
        assert_eq!(
            id.compressed_bytes,
            Some(
                first.columns[0].compressed_bytes.unwrap()
                    + second.columns[0].compressed_bytes.unwrap()
            )
        );
        let mut unknown = merged.clone();
        unknown.merge(FooterProfile {
            file_count: 1,
            row_count: 1,
            row_group_count: 1,
            file_bytes: 0,
            uncompressed_bytes: 0,
            last_modified_ms: None,
            columns: vec![ColumnProfile {
                name: "id".into(),
                field_id: None,
                nulls: None,
                min: None,
                max: None,
                compressed_bytes: None,
            }],
        });
        assert_eq!(unknown.columns[0].nulls, None);
        assert_eq!(unknown.columns[0].min, None);
    }

    #[test]
    fn decimal_text_places_the_point_exactly() {
        assert_eq!(decimal_text(105, 2), "1.05");
        assert_eq!(decimal_text(-5, 3), "-0.005");
        assert_eq!(decimal_text(12, 0), "12");
        assert_eq!(decimal_text(0, 2), "0.00");
    }
}
