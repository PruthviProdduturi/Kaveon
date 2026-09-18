//! Exact metadata statistics bound to an immutable source identity.

use crate::{
    DirectoryListing, FooterProfile, IcebergReader, ObjectDeltaReader, ObjectDirectoryReader,
    ObjectParquetReader, ParquetLocation, ParquetReader, StatValue,
    delta_snapshot::{DeltaFileDetail, DeltaSnapshot},
};
use arrow::datatypes::{DataType, SchemaRef};
use kaveon_core::{DataFormat, KaveonError, Result};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::{
    collections::HashMap,
    fs,
    sync::{Arc, Mutex, OnceLock},
    time::UNIX_EPOCH,
};

const MAX_EXACT_STATISTICS_CACHE_ENTRIES: usize = 256;
static EXACT_STATISTICS_CACHE: OnceLock<Mutex<HashMap<String, SourceProfile>>> = OnceLock::new();
static LATEST_DELTA_STATISTICS: OnceLock<Mutex<HashMap<String, (u64, SourceProfile)>>> =
    OnceLock::new();

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SourceStatistics {
    pub identity_sha256: String,
    pub row_count: u64,
    pub columns: Vec<String>,
    /// The source's own schema (Delta log, Iceberg metadata, Parquet
    /// footer), read with the row count and no data pages: `CREATE TABLE`
    /// without a column list stores it.
    pub schema: SchemaRef,
    /// Immutable Delta version used to derive these statistics.
    pub delta_version: Option<u64>,
    /// The listing a directory Parquet table was analyzed at, for the query
    /// that analyzed it to read the same files.
    pub parquet_listing: Option<Arc<DirectoryListing>>,
}

/// One column's facts as the source's metadata records them: Parquet
/// column-chunk statistics merged over row groups and files, or a Delta add
/// action's `stats`. `None` is "not recorded", never zero.
#[derive(Clone, Debug, PartialEq)]
pub struct SourceColumnProfile {
    pub name: String,
    pub data_type: DataType,
    pub nulls: Option<u64>,
    /// JSON of the logical type: numbers as numbers, text as text, dates and
    /// timestamps as ISO 8601 strings, decimals as exact decimal text.
    pub min: Option<Value>,
    pub max: Option<Value>,
    /// Compressed bytes of the column's chunks (Parquet footers only).
    pub compressed_bytes: Option<u64>,
}

/// [`SourceStatistics`] with the table- and column-level facts the source's
/// metadata carries, read under the same immutable identity.
#[derive(Clone, Debug, PartialEq)]
pub struct SourceProfile {
    pub statistics: SourceStatistics,
    pub format: DataFormat,
    pub file_count: u64,
    /// Row groups over every file; unknown when no footer was read.
    pub row_group_count: Option<u64>,
    /// Bytes of the data files as stored.
    pub compressed_bytes: u64,
    /// The row groups' uncompressed byte total; unknown when no footer was
    /// read.
    pub uncompressed_bytes: Option<u64>,
    /// The newest data file's modification time, milliseconds since the
    /// epoch, when the store or log records one.
    pub last_modified_ms: Option<i64>,
    pub partition_columns: Vec<String>,
    /// One entry per column of `statistics.schema`, in schema order.
    pub columns: Vec<SourceColumnProfile>,
    /// Whether the column facts were read. The row-count path skips the
    /// per-file footer reads an Iceberg column profile needs.
    columns_profiled: bool,
}

/// Whether a read needs the column facts or only what the row count takes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Depth {
    Rows,
    Columns,
}

/// Resolves the source and derives exact row count without reading data pages.
/// Calling this again after analysis is the compare step which prevents stale
/// statistics from being published after a concurrent table update.
pub fn analyze_source(location: &str, format: DataFormat) -> Result<SourceStatistics> {
    profile_at(location, format, Depth::Rows).map(|profile| profile.statistics)
}

/// [`analyze_source`] with the table- and column-level facts: the same
/// metadata-only reads, the same identity, cached under it.
pub fn profile_source(location: &str, format: DataFormat) -> Result<SourceProfile> {
    profile_at(location, format, Depth::Columns)
}

fn profile_at(location: &str, format: DataFormat, depth: Depth) -> Result<SourceProfile> {
    match (is_object(location), format) {
        (true, DataFormat::Delta) => {
            let reader = ObjectDeltaReader::from_uri(location)?;
            analyze_object_delta(location, &reader)
        }
        (true, DataFormat::Iceberg) => {
            let reader = IcebergReader::new(location);
            let snapshot = reader.snapshot()?;
            let mut identity = format!(
                "iceberg\n{}\n{:?}\n",
                snapshot.metadata_uri, snapshot.snapshot_id
            );
            for file in &snapshot.files {
                identity.push_str(file);
                identity.push('\n');
            }
            iceberg_profile(&reader, snapshot, digest(identity), depth)
        }
        (true, DataFormat::Parquet) => {
            let reader = ObjectDirectoryReader::from_uri(location)?;
            let probed = crate::delta_snapshot::blocking({
                let reader = reader.clone();
                async move { reader.probe().await }
            })?;
            match probed {
                ParquetLocation::Object(meta) => {
                    let version = meta.e_tag.or(meta.version).ok_or_else(|| {
                        KaveonError::Storage(
                            "object store did not provide an ETag or version for stable ANALYZE"
                                .into(),
                        )
                    })?;
                    let identity_sha256 =
                        digest(format!("parquet\n{location}\n{version}\n{}", meta.size));
                    if let Some(cached) = cached_statistics(&identity_sha256, depth) {
                        return Ok(cached);
                    }
                    let parquet = crate::delta_snapshot::blocking(async move {
                        ObjectParquetReader::new(reader.store(), reader.root().clone())
                            .metadata()
                            .await
                    })?;
                    Ok(cache_statistics(parquet_profile(
                        identity_sha256,
                        &parquet.schema,
                        parquet.profile,
                        None,
                    )))
                }
                ParquetLocation::Directory(listing) => {
                    if listing.files.iter().any(|file| file.identity().is_none()) {
                        return Err(KaveonError::Storage(
                            "object store did not provide an ETag or version for every file of \
                             the directory for stable ANALYZE"
                                .into(),
                        ));
                    }
                    let listing = Arc::new(listing);
                    let identity_sha256 = digest(format!(
                        "parquet-directory\n{location}\n{}",
                        listing.identity_lines()
                    ));
                    if let Some(cached) = cached_statistics(&identity_sha256, depth) {
                        return Ok(cached);
                    }
                    let reader = reader.with_listing(Arc::clone(&listing));
                    let parquet =
                        crate::delta_snapshot::blocking(async move { reader.metadata().await })?;
                    Ok(cache_statistics(parquet_profile(
                        identity_sha256,
                        &parquet.schema,
                        parquet.profile,
                        Some(listing),
                    )))
                }
            }
        }
        (false, DataFormat::Parquet) if fs::metadata(location)?.is_dir() => {
            let listing = Arc::new(crate::parquet_reader::local_directory_listing(
                std::path::Path::new(location),
            )?);
            let mut identity = format!("parquet-local-directory\n{location}\n");
            for file in &listing.files {
                identity.push_str(&format!(
                    "{}\t{}\t{}\n",
                    file.path, file.size, file.modified_nanos
                ));
            }
            let identity_sha256 = digest(identity);
            if let Some(cached) = cached_statistics(&identity_sha256, depth) {
                return Ok(cached);
            }
            let parquet = ParquetReader::new(location)
                .with_listing(Arc::clone(&listing))
                .metadata()?;
            Ok(cache_statistics(parquet_profile(
                identity_sha256,
                &parquet.schema,
                parquet.profile,
                Some(listing),
            )))
        }
        (false, DataFormat::Parquet) => {
            let file = fs::metadata(location)?;
            let modified = file
                .modified()?
                .duration_since(UNIX_EPOCH)
                .map_err(|_| KaveonError::Storage("file modification time precedes epoch".into()))?
                .as_nanos();
            let identity_sha256 = digest(format!(
                "parquet-local\n{location}\n{}\n{modified}",
                file.len()
            ));
            if let Some(cached) = cached_statistics(&identity_sha256, depth) {
                return Ok(cached);
            }
            let parquet = ParquetReader::new(location).metadata()?;
            Ok(cache_statistics(parquet_profile(
                identity_sha256,
                &parquet.schema,
                parquet.profile,
                None,
            )))
        }
        (false, DataFormat::Delta) => {
            let reader = crate::DeltaTableReader::new(location);
            let snapshot = reader.snapshot()?;
            let version = snapshot.version;
            let identity_sha256 = digest(format!("delta-local\n{location}\n{version}"));
            if let Some(cached) = cached_statistics(&identity_sha256, depth) {
                return Ok(cached);
            }
            let reader = reader.with_version(version);
            let profile = delta_profile(identity_sha256, snapshot, |snapshot| {
                reader.metadata_for_snapshot(snapshot)
            })?;
            Ok(cache_statistics(profile))
        }
        (false, DataFormat::Iceberg) => {
            let reader = IcebergReader::new(location);
            let snapshot = reader.snapshot()?;
            let identity_sha256 = digest(format!(
                "iceberg-local\n{}\n{:?}\n{:?}",
                snapshot.metadata_uri, snapshot.snapshot_id, snapshot.files
            ));
            iceberg_profile(&reader, snapshot, identity_sha256, depth)
        }
    }
}

fn analyze_object_delta(location: &str, reader: &ObjectDeltaReader) -> Result<SourceProfile> {
    if let Some((version, statistics)) = cached_delta_statistics(location)
        && reader.is_latest_version(version)?
    {
        return Ok(statistics);
    }
    let snapshot = reader.snapshot()?;
    let version = snapshot.version;
    let mut identity = format!("delta\n{}\n{}\n", location, snapshot.version);
    for file in &snapshot.files {
        identity.push_str(file.as_ref());
        identity.push('\n');
    }
    let identity_sha256 = digest(identity);
    if let Some(cached) = cached_statistics(&identity_sha256, Depth::Columns) {
        cache_delta_statistics(location, version, cached.clone());
        return Ok(cached);
    }
    let profile = delta_profile(identity_sha256, snapshot, |snapshot| {
        reader.metadata_for_snapshot(snapshot)
    })?;
    let profile = cache_statistics(profile);
    cache_delta_statistics(location, version, profile.clone());
    Ok(profile)
}

/// A Parquet table's profile from its merged footers.
fn parquet_profile(
    identity_sha256: String,
    schema: &SchemaRef,
    footers: FooterProfile,
    listing: Option<Arc<DirectoryListing>>,
) -> SourceProfile {
    let columns = columns_from_footers(schema, &footers, false);
    SourceProfile {
        statistics: SourceStatistics {
            identity_sha256,
            row_count: footers.row_count,
            columns: schema.fields().iter().map(|f| f.name().clone()).collect(),
            schema: Arc::clone(schema),
            delta_version: None,
            parquet_listing: listing,
        },
        format: DataFormat::Parquet,
        file_count: footers.file_count,
        row_group_count: Some(footers.row_group_count),
        compressed_bytes: footers.file_bytes,
        uncompressed_bytes: Some(footers.uncompressed_bytes),
        last_modified_ms: footers.last_modified_ms,
        partition_columns: Vec::new(),
        columns,
        columns_profiled: true,
    }
}

/// A Delta snapshot's profile: from the add actions' `stats` when every
/// active file carries them and the log carries the schema — no footer is
/// opened — else from the footers through `footers`.
fn delta_profile(
    identity_sha256: String,
    snapshot: DeltaSnapshot,
    footers: impl FnOnce(DeltaSnapshot) -> Result<crate::ParquetFileMetadata>,
) -> Result<SourceProfile> {
    let version = snapshot.version;
    let file_count = snapshot.files.len() as u64;
    let file_bytes = snapshot.details.iter().fold(0u64, |total, detail| {
        total.saturating_add(detail.size.unwrap_or(0))
    });
    let last_modified_ms = snapshot
        .details
        .iter()
        .filter_map(|detail| detail.modification_time_ms)
        .max();
    if let Some(schema) = snapshot.schema.clone()
        && let Some((row_count, columns)) = columns_from_delta_stats(&schema, &snapshot.details)
    {
        return Ok(SourceProfile {
            statistics: SourceStatistics {
                identity_sha256,
                row_count,
                columns: schema.fields().iter().map(|f| f.name().clone()).collect(),
                schema,
                delta_version: Some(version),
                parquet_listing: None,
            },
            format: DataFormat::Delta,
            file_count,
            row_group_count: None,
            compressed_bytes: file_bytes,
            uncompressed_bytes: None,
            last_modified_ms,
            partition_columns: Vec::new(),
            columns,
            columns_profiled: true,
        });
    }
    let metadata = footers(snapshot)?;
    let columns = columns_from_footers(&metadata.schema, &metadata.profile, false);
    Ok(SourceProfile {
        statistics: SourceStatistics {
            identity_sha256,
            row_count: metadata.row_count,
            columns: metadata
                .schema
                .fields()
                .iter()
                .map(|field| field.name().clone())
                .collect(),
            schema: Arc::clone(&metadata.schema),
            delta_version: Some(version),
            parquet_listing: None,
        },
        format: DataFormat::Delta,
        file_count,
        row_group_count: Some(metadata.profile.row_group_count),
        compressed_bytes: if file_bytes > 0 {
            file_bytes
        } else {
            metadata.profile.file_bytes
        },
        uncompressed_bytes: Some(metadata.profile.uncompressed_bytes),
        last_modified_ms: last_modified_ms.or(metadata.profile.last_modified_ms),
        partition_columns: Vec::new(),
        columns,
        columns_profiled: true,
    })
}

/// An Iceberg snapshot's profile: rows and bytes from the manifests; column
/// facts from the live files' footers, matched by field id, when asked for.
fn iceberg_profile(
    reader: &IcebergReader,
    snapshot: crate::IcebergSnapshot,
    identity_sha256: String,
    depth: Depth,
) -> Result<SourceProfile> {
    if let Some(cached) = cached_statistics(&identity_sha256, depth) {
        return Ok(cached);
    }
    let footers = match depth {
        Depth::Columns => Some(reader.footer_profile(&snapshot)?),
        Depth::Rows => None,
    };
    let columns = match &footers {
        Some(footers) => columns_from_footers(&snapshot.schema, footers, true),
        None => unprofiled_columns(&snapshot.schema),
    };
    Ok(cache_statistics(SourceProfile {
        statistics: SourceStatistics {
            identity_sha256,
            row_count: snapshot.row_count,
            columns: snapshot
                .schema
                .fields()
                .iter()
                .map(|f| f.name().clone())
                .collect(),
            schema: Arc::clone(&snapshot.schema),
            delta_version: None,
            parquet_listing: None,
        },
        format: DataFormat::Iceberg,
        file_count: snapshot.files.len() as u64,
        row_group_count: footers.as_ref().map(|footers| footers.row_group_count),
        compressed_bytes: snapshot.total_bytes,
        uncompressed_bytes: footers.as_ref().map(|footers| footers.uncompressed_bytes),
        last_modified_ms: footers
            .as_ref()
            .and_then(|footers| footers.last_modified_ms),
        partition_columns: snapshot.partition_columns,
        columns,
        columns_profiled: footers.is_some(),
    }))
}

/// One entry per schema field from the merged footers, matched by field id
/// when `by_field_id` (Iceberg) and by name otherwise.
fn columns_from_footers(
    schema: &SchemaRef,
    footers: &FooterProfile,
    by_field_id: bool,
) -> Vec<SourceColumnProfile> {
    schema
        .fields()
        .iter()
        .map(|field| {
            let profile = if by_field_id {
                field
                    .metadata()
                    .get("PARQUET:field_id")
                    .and_then(|id| id.parse::<i32>().ok())
                    .and_then(|id| {
                        footers
                            .columns
                            .iter()
                            .find(|column| column.field_id == Some(id))
                    })
            } else {
                footers
                    .columns
                    .iter()
                    .find(|column| column.name == *field.name())
            };
            SourceColumnProfile {
                name: field.name().clone(),
                data_type: field.data_type().clone(),
                nulls: profile.and_then(|column| column.nulls),
                min: profile.and_then(|column| column.min.as_ref().map(StatValue::to_json)),
                max: profile.and_then(|column| column.max.as_ref().map(StatValue::to_json)),
                compressed_bytes: profile.and_then(|column| column.compressed_bytes),
            }
        })
        .collect()
}

fn unprofiled_columns(schema: &SchemaRef) -> Vec<SourceColumnProfile> {
    schema
        .fields()
        .iter()
        .map(|field| SourceColumnProfile {
            name: field.name().clone(),
            data_type: field.data_type().clone(),
            nulls: None,
            min: None,
            max: None,
            compressed_bytes: None,
        })
        .collect()
}

/// The row count and column facts from every active file's `stats`, or
/// `None` when any file lacks `stats` or `numRecords`. A bound one file does
/// not record is unknown for the table.
fn columns_from_delta_stats(
    schema: &SchemaRef,
    details: &[DeltaFileDetail],
) -> Option<(u64, Vec<SourceColumnProfile>)> {
    let parsed = details
        .iter()
        .map(|detail| {
            let stats: Value = serde_json::from_str(detail.stats.as_deref()?).ok()?;
            let rows = stats.get("numRecords").and_then(Value::as_u64)?;
            Some((rows, stats))
        })
        .collect::<Option<Vec<_>>>()?;
    let row_count = parsed
        .iter()
        .try_fold(0u64, |total, (rows, _)| total.checked_add(*rows))?;
    let columns = schema
        .fields()
        .iter()
        .map(|field| {
            let mut nulls = Some(0u64);
            let mut min: Option<StatValue> = None;
            let mut max: Option<StatValue> = None;
            let mut min_known = true;
            let mut max_known = true;
            for (rows, stats) in &parsed {
                match stats
                    .pointer(&format!("/nullCount/{}", escape_pointer(field.name())))
                    .and_then(Value::as_u64)
                {
                    Some(count) => nulls = nulls.map(|total| total.saturating_add(count)),
                    None => nulls = None,
                }
                if *rows == 0 {
                    continue;
                }
                let all_null = stats
                    .pointer(&format!("/nullCount/{}", escape_pointer(field.name())))
                    .and_then(Value::as_u64)
                    .is_some_and(|count| count == *rows);
                for (known, bound, key, prefer) in [
                    (
                        &mut min_known,
                        &mut min,
                        "minValues",
                        std::cmp::Ordering::Less,
                    ),
                    (
                        &mut max_known,
                        &mut max,
                        "maxValues",
                        std::cmp::Ordering::Greater,
                    ),
                ] {
                    if !*known {
                        continue;
                    }
                    let value = stats
                        .pointer(&format!("/{key}/{}", escape_pointer(field.name())))
                        .filter(|value| !value.is_null())
                        .and_then(|value| delta_stat_value(value, field.data_type()));
                    match value {
                        Some(value) => match bound.take() {
                            None => *bound = Some(value),
                            Some(current) => match current.partial_cmp(&value) {
                                Some(order) if order == prefer => *bound = Some(current),
                                Some(_) => *bound = Some(value),
                                None => *known = false,
                            },
                        },
                        None if all_null => {}
                        None => *known = false,
                    }
                }
            }
            SourceColumnProfile {
                name: field.name().clone(),
                data_type: field.data_type().clone(),
                nulls,
                min: min.filter(|_| min_known).map(|value| value.to_json()),
                max: max.filter(|_| max_known).map(|value| value.to_json()),
                compressed_bytes: None,
            }
        })
        .collect();
    Some((row_count, columns))
}

/// A Delta `stats` bound in the column's logical type. Delta writes numbers
/// as JSON numbers, and strings, dates and timestamps as text.
fn delta_stat_value(value: &Value, data_type: &DataType) -> Option<StatValue> {
    match (value, data_type) {
        (Value::Bool(value), _) => Some(StatValue::Bool(*value)),
        (Value::Number(number), DataType::Decimal128(_, scale)) => {
            let text = number.to_string();
            let (integral, fraction) = text.split_once('.').unwrap_or((text.as_str(), ""));
            let scale_digits = usize::try_from(*scale).ok()?;
            if fraction.len() > scale_digits || text.contains(['e', 'E']) {
                return None;
            }
            let mut digits = integral.to_owned();
            digits.push_str(fraction);
            digits.push_str(&"0".repeat(scale_digits - fraction.len()));
            digits
                .parse::<i128>()
                .ok()
                .map(|unscaled| StatValue::Decimal {
                    unscaled,
                    scale: *scale,
                })
        }
        (Value::Number(number), DataType::Float32 | DataType::Float64) => {
            number.as_f64().map(StatValue::Float)
        }
        (Value::Number(number), _) => number
            .as_i64()
            .map(|value| StatValue::Int(i128::from(value)))
            .or_else(|| {
                number
                    .as_u64()
                    .map(|value| StatValue::Int(i128::from(value)))
            })
            .or_else(|| number.as_f64().map(StatValue::Float)),
        (Value::String(text), _) => Some(StatValue::Text(text.clone())),
        _ => None,
    }
}

fn escape_pointer(name: &str) -> String {
    name.replace('~', "~0").replace('/', "~1")
}

fn cached_statistics(identity_sha256: &str, depth: Depth) -> Option<SourceProfile> {
    EXACT_STATISTICS_CACHE
        .get_or_init(Default::default)
        .lock()
        .ok()?
        .get(identity_sha256)
        .filter(|profile| depth == Depth::Rows || profile.columns_profiled)
        .cloned()
}

fn cache_statistics(profile: SourceProfile) -> SourceProfile {
    let Ok(mut cache) = EXACT_STATISTICS_CACHE.get_or_init(Default::default).lock() else {
        return profile;
    };
    let key = profile.statistics.identity_sha256.clone();
    if cache.len() >= MAX_EXACT_STATISTICS_CACHE_ENTRIES && !cache.contains_key(&key) {
        // Statistics are an optimization only. Clearing at the fixed bound is
        // deterministic, keeps memory bounded, and cannot affect correctness.
        cache.clear();
    }
    cache.insert(key, profile.clone());
    profile
}

fn cached_delta_statistics(location: &str) -> Option<(u64, SourceProfile)> {
    LATEST_DELTA_STATISTICS
        .get_or_init(Default::default)
        .lock()
        .ok()?
        .get(location)
        .cloned()
}

fn cache_delta_statistics(location: &str, version: u64, statistics: SourceProfile) {
    let Ok(mut cache) = LATEST_DELTA_STATISTICS.get_or_init(Default::default).lock() else {
        return;
    };
    if cache.len() >= MAX_EXACT_STATISTICS_CACHE_ENTRIES && !cache.contains_key(location) {
        cache.clear();
    }
    // Concurrent analyses may finish out of order. Never let an older snapshot
    // replace a newer cache entry.
    if cache
        .get(location)
        .is_none_or(|(cached, _)| version >= *cached)
    {
        cache.insert(location.to_owned(), (version, statistics));
    }
}
fn digest(value: String) -> String {
    format!("{:x}", Sha256::digest(value.as_bytes()))
}
fn is_object(value: &str) -> bool {
    value.starts_with("abfss://") || value.starts_with("s3://")
}

#[cfg(test)]
mod tests {
    use super::*;
    use arrow::{
        array::{ArrayRef, Int64Array, StringArray},
        datatypes::{DataType, Field, Schema},
        record_batch::RecordBatch,
    };
    use object_store::{ObjectStore, memory::InMemory, path::Path};
    use parquet::{arrow::ArrowWriter, file::properties::WriterProperties};
    use std::{fs::File, sync::Arc};

    fn write(path: &std::path::Path, rows: usize) {
        let schema = Arc::new(Schema::new(vec![
            Field::new("id", DataType::Int64, false),
            Field::new("name", DataType::Utf8, false),
        ]));
        let batch = RecordBatch::try_new(
            schema.clone(),
            vec![
                Arc::new(Int64Array::from_iter_values((0..rows).map(|v| v as i64))) as ArrayRef,
                Arc::new(StringArray::from_iter_values(
                    (0..rows).map(|v| format!("r{v}")),
                )) as ArrayRef,
            ],
        )
        .unwrap();
        let mut writer = ArrowWriter::try_new(File::create(path).unwrap(), schema, None).unwrap();
        writer.write(&batch).unwrap();
        writer.close().unwrap();
    }

    fn parquet_bytes(rows: usize) -> Vec<u8> {
        let schema = Arc::new(Schema::new(vec![
            Field::new("id", DataType::Int64, false),
            Field::new("name", DataType::Utf8, false),
        ]));
        let batch = RecordBatch::try_new(
            schema.clone(),
            vec![
                Arc::new(Int64Array::from_iter_values((0..rows).map(|v| v as i64))) as ArrayRef,
                Arc::new(StringArray::from_iter_values(
                    (0..rows).map(|v| format!("r{v}")),
                )) as ArrayRef,
            ],
        )
        .unwrap();
        let mut writer = ArrowWriter::try_new(Vec::new(), schema, None).unwrap();
        writer.write(&batch).unwrap();
        writer.into_inner().unwrap()
    }

    #[test]
    fn local_parquet_statistics_are_exact_stable_and_change_on_replacement() {
        let path =
            std::env::temp_dir().join(format!("kaveon-analyze-{}.parquet", std::process::id()));
        write(&path, 3);
        let first = analyze_source(path.to_str().unwrap(), DataFormat::Parquet).unwrap();
        let repeated = analyze_source(path.to_str().unwrap(), DataFormat::Parquet).unwrap();
        assert_eq!(first.row_count, 3);
        assert_eq!(first.columns, ["id", "name"]);
        assert_eq!(
            first
                .schema
                .fields()
                .iter()
                .map(|field| (
                    field.name().as_str(),
                    field.data_type().clone(),
                    field.is_nullable()
                ))
                .collect::<Vec<_>>(),
            [
                ("id", DataType::Int64, false),
                ("name", DataType::Utf8, false)
            ]
        );
        assert_eq!(first.identity_sha256, repeated.identity_sha256);
        write(&path, 17);
        let replaced = analyze_source(path.to_str().unwrap(), DataFormat::Parquet).unwrap();
        assert_eq!(replaced.row_count, 17);
        assert_ne!(first.identity_sha256, replaced.identity_sha256);
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn local_parquet_profile_merges_column_facts_over_row_groups() {
        let path =
            std::env::temp_dir().join(format!("kaveon-profile-{}.parquet", std::process::id()));
        let schema = Arc::new(Schema::new(vec![
            Field::new("id", DataType::Int64, false),
            Field::new("name", DataType::Utf8, true),
        ]));
        let batch = RecordBatch::try_new(
            schema.clone(),
            vec![
                Arc::new(Int64Array::from(vec![7, 2, 11, 4])) as ArrayRef,
                Arc::new(StringArray::from(vec![
                    Some("kiwi"),
                    None,
                    Some("apple"),
                    Some("zucchini"),
                ])) as ArrayRef,
            ],
        )
        .unwrap();
        let properties = WriterProperties::builder()
            .set_max_row_group_size(2)
            .build();
        let mut writer =
            ArrowWriter::try_new(File::create(&path).unwrap(), schema, Some(properties)).unwrap();
        writer.write(&batch).unwrap();
        writer.close().unwrap();

        let profile = profile_source(path.to_str().unwrap(), DataFormat::Parquet).unwrap();
        assert_eq!(profile.format, DataFormat::Parquet);
        assert_eq!(profile.statistics.row_count, 4);
        assert_eq!(profile.file_count, 1);
        assert_eq!(profile.row_group_count, Some(2));
        assert_eq!(
            profile.compressed_bytes,
            std::fs::metadata(&path).unwrap().len()
        );
        assert!(profile.uncompressed_bytes.unwrap() > 0);
        assert!(profile.last_modified_ms.unwrap() > 0);
        assert!(profile.partition_columns.is_empty());
        assert_eq!(profile.columns.len(), 2);
        let id = &profile.columns[0];
        assert_eq!(id.name, "id");
        assert_eq!(id.data_type, DataType::Int64);
        assert_eq!(id.nulls, Some(0));
        assert_eq!(id.min, Some(serde_json::json!(2)));
        assert_eq!(id.max, Some(serde_json::json!(11)));
        assert!(id.compressed_bytes.unwrap() > 0);
        let name = &profile.columns[1];
        assert_eq!(name.nulls, Some(1));
        assert_eq!(name.min, Some(serde_json::json!("apple")));
        assert_eq!(name.max, Some(serde_json::json!("zucchini")));

        // The row-count path shares the cached profile under one identity.
        let statistics = analyze_source(path.to_str().unwrap(), DataFormat::Parquet).unwrap();
        assert_eq!(statistics, profile.statistics);
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn local_delta_statistics_follow_the_exact_active_snapshot() {
        let directory =
            std::env::temp_dir().join(format!("kaveon-analyze-delta-{}", std::process::id()));
        let log = directory.join("_delta_log");
        std::fs::create_dir_all(&log).unwrap();
        write(&directory.join("first.parquet"), 3);
        write(&directory.join("second.parquet"), 5);
        write(&directory.join("third.parquet"), 7);
        std::fs::write(
            log.join("00000000000000000000.json"),
            "{\"add\":{\"path\":\"first.parquet\"}}\n{\"add\":{\"path\":\"second.parquet\"}}",
        )
        .unwrap();

        let first = analyze_source(directory.to_str().unwrap(), DataFormat::Delta).unwrap();
        assert_eq!(first.row_count, 8);
        assert_eq!(first.columns, ["id", "name"]);
        assert_eq!(first.schema.fields().len(), 2);
        assert_eq!(first.schema.field(0).data_type(), &DataType::Int64);
        assert_eq!(first.delta_version, Some(0));
        let profile = profile_source(directory.to_str().unwrap(), DataFormat::Delta).unwrap();
        assert_eq!(profile.statistics, first);
        assert_eq!(profile.file_count, 2);
        assert_eq!(profile.row_group_count, Some(2));
        assert_eq!(
            profile.compressed_bytes,
            std::fs::metadata(directory.join("first.parquet"))
                .unwrap()
                .len()
                + std::fs::metadata(directory.join("second.parquet"))
                    .unwrap()
                    .len()
        );
        assert_eq!(profile.columns[0].min, Some(serde_json::json!(0)));
        assert_eq!(profile.columns[0].max, Some(serde_json::json!(4)));
        assert_eq!(profile.columns[1].nulls, Some(0));

        std::fs::write(
            log.join("00000000000000000001.json"),
            "{\"remove\":{\"path\":\"first.parquet\"}}\n{\"add\":{\"path\":\"third.parquet\"}}",
        )
        .unwrap();
        let second = analyze_source(directory.to_str().unwrap(), DataFormat::Delta).unwrap();
        assert_eq!(second.row_count, 12);
        assert_ne!(first.identity_sha256, second.identity_sha256);
        assert_eq!(second.delta_version, Some(1));

        std::fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn local_delta_add_stats_profile_the_table_without_footer_reads() {
        let directory =
            std::env::temp_dir().join(format!("kaveon-analyze-delta-stats-{}", std::process::id()));
        let log = directory.join("_delta_log");
        std::fs::create_dir_all(&log).unwrap();
        // No data file exists: the log alone must answer.
        let schema = serde_json::json!({"type":"struct","fields":[
            {"name":"id","type":"long","nullable":false,"metadata":{}},
            {"name":"name","type":"string","nullable":true,"metadata":{}},
            {"name":"day","type":"date","nullable":true,"metadata":{}}
        ]});
        let add = |path: &str, size: u64, modified: i64, stats: serde_json::Value| {
            serde_json::json!({"add":{"path":path,"size":size,"modificationTime":modified,"dataChange":true,"stats":stats.to_string()}}).to_string()
        };
        let log_lines = [
            serde_json::json!({"protocol":{"minReaderVersion":1,"minWriterVersion":2}}).to_string(),
            serde_json::json!({"metaData":{"id":"t","format":{"provider":"parquet"},"schemaString":schema.to_string(),"partitionColumns":[]}}).to_string(),
            add(
                "a.parquet",
                1000,
                1_700_000_000_000,
                serde_json::json!({"numRecords":3,"minValues":{"id":5,"name":"kiwi","day":"2024-01-03"},"maxValues":{"id":9,"name":"pear","day":"2024-02-01"},"nullCount":{"id":0,"name":1,"day":0}}),
            ),
            add(
                "b.parquet",
                2500,
                1_700_000_500_000,
                serde_json::json!({"numRecords":4,"minValues":{"id":1,"name":"apple","day":"2023-12-31"},"maxValues":{"id":7,"name":"fig","day":"2024-01-20"},"nullCount":{"id":0,"name":2,"day":1}}),
            ),
        ];
        std::fs::write(
            log.join("00000000000000000000.json"),
            log_lines.join(
                "
",
            ),
        )
        .unwrap();
        let profile = profile_source(directory.to_str().unwrap(), DataFormat::Delta).unwrap();
        assert_eq!(profile.format, DataFormat::Delta);
        assert_eq!(profile.statistics.row_count, 7);
        assert_eq!(profile.statistics.delta_version, Some(0));
        assert_eq!(profile.statistics.columns, ["id", "name", "day"]);
        assert_eq!(profile.file_count, 2);
        assert_eq!(profile.row_group_count, None);
        assert_eq!(profile.compressed_bytes, 3500);
        assert_eq!(profile.uncompressed_bytes, None);
        assert_eq!(profile.last_modified_ms, Some(1_700_000_500_000));
        let id = &profile.columns[0];
        assert_eq!(id.data_type, DataType::Int64);
        assert_eq!(id.nulls, Some(0));
        assert_eq!(id.min, Some(serde_json::json!(1)));
        assert_eq!(id.max, Some(serde_json::json!(9)));
        assert_eq!(id.compressed_bytes, None);
        let name = &profile.columns[1];
        assert_eq!(name.nulls, Some(3));
        assert_eq!(name.min, Some(serde_json::json!("apple")));
        assert_eq!(name.max, Some(serde_json::json!("pear")));
        let day = &profile.columns[2];
        assert_eq!(day.nulls, Some(1));
        assert_eq!(day.min, Some(serde_json::json!("2023-12-31")));
        assert_eq!(day.max, Some(serde_json::json!("2024-02-01")));
        assert_eq!(
            analyze_source(directory.to_str().unwrap(), DataFormat::Delta)
                .unwrap()
                .row_count,
            7
        );
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn object_delta_cached_statistics_invalidate_on_next_commit() {
        let store: Arc<dyn ObjectStore> = Arc::new(InMemory::new());
        let runtime = tokio::runtime::Runtime::new().unwrap();
        for (name, rows) in [("first.parquet", 3), ("second.parquet", 7)] {
            runtime
                .block_on(store.put(
                    &Path::from(format!("table/{name}")),
                    parquet_bytes(rows).into(),
                ))
                .unwrap();
        }
        runtime
            .block_on(store.put(
                &Path::from("table/_delta_log/00000000000000000000.json"),
                br#"{"add":{"path":"first.parquet"}}"#.to_vec().into(),
            ))
            .unwrap();
        let reader = ObjectDeltaReader::new(store.clone(), Path::from("table"));
        let location = format!("memory://delta-cache-{}", std::process::id());
        let first = analyze_object_delta(&location, &reader).unwrap();
        assert_eq!(first.statistics.row_count, 3);
        assert_eq!(first.statistics.delta_version, Some(0));
        assert_eq!(first.file_count, 1);
        assert_eq!(first.columns[0].min, Some(serde_json::json!(0)));
        assert_eq!(first.columns[0].max, Some(serde_json::json!(2)));
        assert_eq!(analyze_object_delta(&location, &reader).unwrap(), first);

        runtime
            .block_on(
                store.put(
                    &Path::from("table/_delta_log/00000000000000000001.json"),
                    br#"{"remove":{"path":"first.parquet"}}
{"add":{"path":"second.parquet"}}"#
                        .to_vec()
                        .into(),
                ),
            )
            .unwrap();
        let second = analyze_object_delta(&location, &reader).unwrap();
        assert_eq!(second.statistics.row_count, 7);
        assert_ne!(
            second.statistics.identity_sha256,
            first.statistics.identity_sha256
        );
        assert_eq!(second.statistics.delta_version, Some(1));
    }
}
