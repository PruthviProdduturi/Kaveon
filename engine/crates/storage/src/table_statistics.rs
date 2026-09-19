//! Building a table's statistics from a source: the metadata path (footers,
//! the Delta log, Iceberg manifests — no data page read) and the full path
//! (every column read once, streamed, in parallel across files, to build
//! the distinct-count and quantile sketches and exact bounds), plus the
//! incremental refresh that folds added files in.

use crate::{
    DirectoryListing, FooterProfile, IcebergReader, ObjectDeltaReader, ObjectLocation,
    ObjectParquetReader, ParquetReader, SourceColumnProfile, SourceProfile,
    delta_snapshot::{DeltaFileDetail, DeltaSnapshot},
    source_statistics::{column_facts_from_delta_stats, columns_from_footers, is_object},
};
use arrow::{
    array::{Array, ArrayRef, AsArray},
    compute,
    datatypes::{DataType, SchemaRef},
};
use kaveon_core::{
    BatchSource, ColumnSketches, ColumnStatistics, DataFormat, FileColumnSketches,
    FileColumnStatistics, FileStatistics, HllSketch, KaveonError, KllSketch, OperatorMemoryAccount,
    Result, SourceVersion, StatValue, StatisticsDepth, TableId, TableStatistics,
    statistics::MAX_PER_FILE_STATISTICS,
};
use object_store::{ObjectStore, path::Path as ObjectPath};
use std::{
    cmp::Ordering,
    collections::BTreeSet,
    path::PathBuf,
    sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering as AtomicOrdering},
    },
    time::{SystemTime, UNIX_EPOCH},
};

/// Footers of a directory table are read this many at a time.
const FOOTER_READ_CONCURRENCY: usize = 8;
/// Rows per batch when reading the columns for sketches.
const SKETCH_BATCH_SIZE: usize = 8_192;
/// The estimated resident size of one column's sketches on one thread.
const SKETCH_MEMORY_PER_COLUMN: u64 = 64 * 1024;

/// How the full path reads the columns.
#[derive(Default)]
pub struct FullScanOptions {
    /// The account the scan reserves its sketches and batches through;
    /// none for an embedded build outside a query.
    pub memory: Option<OperatorMemoryAccount>,
    /// Files read at once; 0 or 1 reads them one after another.
    pub threads: usize,
    /// The columns to sketch; every sketchable column when `None`.
    pub columns: Option<Vec<String>>,
}

/// A data file of a source, wherever it is.
#[derive(Clone)]
pub struct DataFile {
    /// The path relative to the table location (a Delta add path, a listing
    /// path); an Iceberg data file's full path.
    pub label: String,
    location: FileLocation,
    /// Bytes as stored, when the listing or log recorded them.
    bytes: Option<u64>,
}

#[derive(Clone)]
enum FileLocation {
    Local(PathBuf),
    Object {
        store: Arc<dyn ObjectStore>,
        path: ObjectPath,
    },
}

impl DataFile {
    fn footer(&self) -> Result<crate::ParquetFileMetadata> {
        match &self.location {
            FileLocation::Local(path) => ParquetReader::new(path).metadata(),
            FileLocation::Object { store, path } => {
                let store = Arc::clone(store);
                let path = path.clone();
                crate::delta_snapshot::blocking(async move {
                    ObjectParquetReader::new(store, path).metadata().await
                })
            }
        }
    }

    pub(crate) fn open(&self, columns: Option<&[String]>) -> Result<Box<dyn BatchSource>> {
        match &self.location {
            FileLocation::Local(path) => {
                let mut reader = ParquetReader::new(path).with_batch_size(SKETCH_BATCH_SIZE);
                if let Some(columns) = columns {
                    reader = reader.with_columns(columns.to_vec());
                }
                Ok(Box::new(reader.read()?))
            }
            FileLocation::Object { store, path } => {
                let mut reader = ObjectParquetReader::new(Arc::clone(store), path.clone())
                    .with_batch_size(SKETCH_BATCH_SIZE);
                if let Some(columns) = columns {
                    reader = reader.with_columns(columns.to_vec());
                }
                Ok(Box::new(reader.read_blocking()?))
            }
        }
    }
}

/// The source's files and profile at one identity.
pub struct SourceFiles {
    /// The location as resolved by the catalog.
    pub location: String,
    pub profile: SourceProfile,
    pub files: Vec<DataFile>,
    /// Per-file facts the Delta log carried for every file, when it did.
    delta_stats: Option<Vec<(u64, Vec<SourceColumnProfile>)>>,
    /// Whether column facts are matched to files by Parquet field id.
    by_field_id: bool,
}

impl SourceFiles {
    pub fn source_version(&self) -> SourceVersion {
        crate::source_statistics::profile_source_version(&self.profile)
    }
}

/// The source's files under its current identity.
pub fn enumerate_source(location: &str, format: DataFormat) -> Result<SourceFiles> {
    let profile = crate::profile_source(location, format)?;
    let object = is_object(location);
    let (files, delta_stats, by_field_id) = match (format, object) {
        (DataFormat::Parquet, false) => {
            let root = PathBuf::from(location);
            match &profile.statistics.parquet_listing {
                Some(listing) => (
                    listing
                        .files
                        .iter()
                        .map(|file| DataFile {
                            label: file.path.to_string(),
                            location: FileLocation::Local(root.join(file.path.as_ref())),
                            bytes: Some(file.size),
                        })
                        .collect(),
                    None,
                    false,
                ),
                None => (
                    vec![DataFile {
                        label: root
                            .file_name()
                            .map(|name| name.to_string_lossy().into_owned())
                            .unwrap_or_else(|| location.to_owned()),
                        location: FileLocation::Local(root),
                        bytes: Some(profile.compressed_bytes),
                    }],
                    None,
                    false,
                ),
            }
        }
        (DataFormat::Parquet, true) => {
            let object = ObjectLocation::from_uri(location.trim_end_matches('/'))?;
            match &profile.statistics.parquet_listing {
                Some(listing) => (
                    listing
                        .files
                        .iter()
                        .map(|file| DataFile {
                            label: relative_label(&listing.root, &file.path),
                            location: FileLocation::Object {
                                store: Arc::clone(&object.store),
                                path: file.path.clone(),
                            },
                            bytes: Some(file.size),
                        })
                        .collect(),
                    None,
                    false,
                ),
                None => (
                    vec![DataFile {
                        label: object
                            .path
                            .filename()
                            .map(str::to_owned)
                            .unwrap_or_else(|| location.to_owned()),
                        location: FileLocation::Object {
                            store: Arc::clone(&object.store),
                            path: object.path.clone(),
                        },
                        bytes: Some(profile.compressed_bytes),
                    }],
                    None,
                    false,
                ),
            }
        }
        (DataFormat::Delta, false) => {
            let reader = crate::DeltaTableReader::new(location);
            let snapshot = reader.snapshot()?;
            let snapshot = pinned_snapshot(snapshot, &profile, |version| {
                reader.with_version(version).snapshot()
            })?;
            let root = PathBuf::from(location);
            let files = snapshot
                .files
                .iter()
                .zip(&snapshot.details)
                .map(|(path, detail)| DataFile {
                    label: path.to_string(),
                    location: FileLocation::Local(root.join(path.as_ref())),
                    bytes: detail.size,
                })
                .collect();
            (files, delta_stats(&profile, &snapshot), false)
        }
        (DataFormat::Delta, true) => {
            let object = ObjectLocation::from_uri(location.trim_end_matches('/'))?;
            let reader = ObjectDeltaReader::new(Arc::clone(&object.store), object.path.clone());
            let snapshot = reader.snapshot()?;
            let snapshot = pinned_snapshot(snapshot, &profile, |version| {
                ObjectDeltaReader::new(Arc::clone(&object.store), object.path.clone())
                    .with_version(version)
                    .snapshot()
            })?;
            let files = snapshot
                .files
                .iter()
                .zip(&snapshot.details)
                .map(|(path, detail)| DataFile {
                    label: relative_label(&object.path, path),
                    location: FileLocation::Object {
                        store: Arc::clone(&object.store),
                        path: path.clone(),
                    },
                    bytes: detail.size,
                })
                .collect();
            (files, delta_stats(&profile, &snapshot), false)
        }
        (DataFormat::Iceberg, _) => {
            let reader = IcebergReader::new(location);
            let snapshot = reader.snapshot()?;
            if snapshot.snapshot_id != profile.iceberg_snapshot_id {
                return Err(KaveonError::Storage(
                    "Iceberg snapshot changed while enumerating files".into(),
                ));
            }
            let files = snapshot
                .files
                .iter()
                .map(|file| {
                    let location = if is_object(file) {
                        let object = ObjectLocation::from_uri(file)?;
                        FileLocation::Object {
                            store: object.store,
                            path: object.path,
                        }
                    } else {
                        FileLocation::Local(crate::iceberg_reader::local_path(file)?)
                    };
                    Ok(DataFile {
                        label: file.clone(),
                        location,
                        bytes: None,
                    })
                })
                .collect::<Result<Vec<_>>>()?;
            (files, None, true)
        }
    };
    Ok(SourceFiles {
        location: location.to_owned(),
        profile,
        files,
        delta_stats,
        by_field_id,
    })
}

/// The snapshot at the profile's version, re-resolved when a commit landed
/// between the profile and the listing.
fn pinned_snapshot(
    snapshot: DeltaSnapshot,
    profile: &SourceProfile,
    at_version: impl FnOnce(u64) -> Result<DeltaSnapshot>,
) -> Result<DeltaSnapshot> {
    match profile.statistics.delta_version {
        Some(version) if version != snapshot.version => at_version(version),
        _ => Ok(snapshot),
    }
}

fn delta_stats(
    profile: &SourceProfile,
    snapshot: &DeltaSnapshot,
) -> Option<Vec<(u64, Vec<SourceColumnProfile>)>> {
    let schema = &profile.statistics.schema;
    snapshot
        .details
        .iter()
        .map(|detail| single_file_delta_stats(schema, detail))
        .collect()
}

fn single_file_delta_stats(
    schema: &SchemaRef,
    detail: &DeltaFileDetail,
) -> Option<(u64, Vec<SourceColumnProfile>)> {
    column_facts_from_delta_stats(schema, std::slice::from_ref(detail))
}

fn relative_label(root: &ObjectPath, path: &ObjectPath) -> String {
    let root = root.as_ref();
    let path = path.as_ref();
    if root.is_empty() {
        return path.to_owned();
    }
    path.strip_prefix(root)
        .map(|rest| rest.trim_start_matches('/'))
        .filter(|rest| !rest.is_empty())
        .unwrap_or(path)
        .to_owned()
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|since| since.as_millis() as u64)
        .unwrap_or(0)
}

/// The metadata path: table facts and per-column bounds from the source's
/// metadata, per-file bounds from the Delta log or every file's footer. No
/// data page is read.
pub fn metadata_statistics(
    location: &str,
    format: DataFormat,
    table_id: TableId,
) -> Result<TableStatistics> {
    let source = enumerate_source(location, format)?;
    statistics_from_source(&source, table_id)
}

/// The partition columns a profiled source carries: the Delta log's, or the
/// `key=value` keys of a partitioned Parquet directory.
pub fn partition_column_names(profile: &SourceProfile) -> Vec<String> {
    if !profile.partition_columns.is_empty() {
        return profile.partition_columns.clone();
    }
    profile
        .statistics
        .parquet_listing
        .as_ref()
        .map(|listing| {
            listing
                .partitions
                .iter()
                .map(|column| column.name().to_owned())
                .collect()
        })
        .unwrap_or_default()
}

fn statistics_from_source(source: &SourceFiles, table_id: TableId) -> Result<TableStatistics> {
    let profile = &source.profile;
    let columns = profile
        .columns
        .iter()
        .map(|column| ColumnStatistics {
            name: column.name.clone(),
            data_type: column.data_type.clone(),
            null_count: column.nulls,
            min: column.min.clone(),
            max: column.max.clone(),
            bounds_exact: column.bounds_exact
                && (column.min.is_some() || column.max.is_some() || column.nulls.is_some()),
            distinct: None,
            distinct_exact: None,
            quantiles: None,
            bytes: column.compressed_bytes,
        })
        .collect::<Vec<_>>();
    let (per_file, per_file_complete) = if source.files.len() <= MAX_PER_FILE_STATISTICS {
        (per_file_statistics(source)?, true)
    } else {
        (Vec::new(), false)
    };
    Ok(TableStatistics {
        version: kaveon_core::statistics::TABLE_STATISTICS_VERSION,
        table_id,
        source_version: source.source_version(),
        computed_at_ms: now_ms(),
        depth: StatisticsDepth::Metadata,
        format: profile.format,
        location: source.location.clone(),
        rows: profile.statistics.row_count,
        bytes: profile.compressed_bytes,
        files: source.files.len() as u64,
        row_groups: profile.row_group_count,
        uncompressed_bytes: profile.uncompressed_bytes,
        last_modified_ms: profile.last_modified_ms,
        partition_columns: partition_column_names(profile),
        columns,
        per_file,
        per_file_complete,
    })
}

/// Every file's bounds: from the Delta log when it carries them for every
/// file, else from the footers, read a few at a time.
fn per_file_statistics(source: &SourceFiles) -> Result<Vec<FileStatistics>> {
    let schema = &source.profile.statistics.schema;
    if let Some(stats) = &source.delta_stats {
        return Ok(source
            .files
            .iter()
            .zip(stats)
            .map(|(file, (rows, columns))| FileStatistics {
                path: file.label.clone(),
                rows: *rows,
                bytes: file.bytes.unwrap_or(0),
                columns: columns
                    .iter()
                    .map(|column| FileColumnStatistics {
                        min: column.min.clone(),
                        max: column.max.clone(),
                        null_count: column.nulls,
                    })
                    .collect(),
            })
            .collect());
    }
    let footers = read_footers(&source.files)?;
    Ok(source
        .files
        .iter()
        .zip(footers)
        .map(|(file, metadata)| {
            file_statistics_from_footer(file, schema, &metadata.profile, source.by_field_id)
        })
        .collect())
}

fn file_statistics_from_footer(
    file: &DataFile,
    schema: &SchemaRef,
    footer: &FooterProfile,
    by_field_id: bool,
) -> FileStatistics {
    let columns = columns_from_footers(schema, footer, by_field_id);
    FileStatistics {
        path: file.label.clone(),
        rows: footer.row_count,
        bytes: file.bytes.unwrap_or(footer.file_bytes),
        columns: columns
            .into_iter()
            .map(|column| FileColumnStatistics {
                min: column.min,
                max: column.max,
                null_count: column.nulls,
            })
            .collect(),
    }
}

/// The footers of `files`, in order, read `FOOTER_READ_CONCURRENCY` at a
/// time on threads of their own.
fn read_footers(files: &[DataFile]) -> Result<Vec<crate::ParquetFileMetadata>> {
    let results: Vec<Mutex<Option<Result<crate::ParquetFileMetadata>>>> =
        files.iter().map(|_| Mutex::new(None)).collect();
    let next = AtomicUsize::new(0);
    let workers = FOOTER_READ_CONCURRENCY.min(files.len().max(1));
    std::thread::scope(|scope| {
        for _ in 0..workers {
            scope.spawn(|| {
                loop {
                    let index = next.fetch_add(1, AtomicOrdering::Relaxed);
                    let Some(file) = files.get(index) else {
                        return;
                    };
                    let result = file.footer();
                    *results[index].lock().unwrap_or_else(|e| e.into_inner()) = Some(result);
                }
            });
        }
    });
    results
        .into_iter()
        .map(|slot| {
            slot.into_inner()
                .unwrap_or_else(|e| e.into_inner())
                .unwrap_or_else(|| Err(KaveonError::Storage("footer read did not run".into())))
        })
        .collect()
}

/// The full path: the metadata statistics, then every sketchable column of
/// every file read once to build the distinct-count and quantile sketches,
/// exact bounds and null counts. The source identity is read again at the
/// end; a source that changed underneath fails rather than mixing
/// versions.
pub fn full_statistics(
    location: &str,
    format: DataFormat,
    table_id: TableId,
    options: &FullScanOptions,
) -> Result<TableStatistics> {
    let source = enumerate_source(location, format)?;
    let mut statistics = statistics_from_source(&source, table_id)?;
    let selected = sketch_columns(&statistics, options.columns.as_deref())?;
    let scanned = scan_files(&source.files, &statistics, &selected, options)?;
    apply_scan(&mut statistics, &selected, scanned);
    let after = crate::analyze_source(location, format)?;
    if after.identity_sha256 != statistics.source_version.identity_sha256 {
        return Err(KaveonError::Storage(
            "table source changed while its statistics were being computed".into(),
        ));
    }
    statistics.depth = StatisticsDepth::Full;
    statistics.computed_at_ms = now_ms();
    Ok(statistics)
}

/// The statistics for the source's current version, from `previous`: the
/// same version comes back unchanged; files added to a full document are
/// read and folded in; a removal or a metadata-only document recomputes at
/// the document's depth.
pub fn refresh_statistics(
    previous: &TableStatistics,
    location: &str,
    format: DataFormat,
    options: &FullScanOptions,
) -> Result<TableStatistics> {
    let source = enumerate_source(location, format)?;
    if source.profile.statistics.identity_sha256 == previous.source_version.identity_sha256 {
        return Ok(previous.clone());
    }
    let table_id = previous.table_id.clone();
    if previous.depth != StatisticsDepth::Full || !previous.per_file_complete {
        return match previous.depth {
            StatisticsDepth::Metadata => statistics_from_source(&source, table_id),
            StatisticsDepth::Full => full_statistics(location, format, table_id, options),
        };
    }
    let known: BTreeSet<&str> = previous
        .per_file
        .iter()
        .map(|file| file.path.as_str())
        .collect();
    let current: BTreeSet<&str> = source
        .files
        .iter()
        .map(|file| file.label.as_str())
        .collect();
    let removed = known.iter().any(|path| !current.contains(path));
    let schema_changed = source.profile.columns.len() != previous.columns.len()
        || source
            .profile
            .columns
            .iter()
            .zip(&previous.columns)
            .any(|(now, then)| now.name != then.name || now.data_type != then.data_type);
    if removed || schema_changed {
        return full_statistics(location, format, table_id, options);
    }
    let added: Vec<DataFile> = source
        .files
        .iter()
        .filter(|file| !known.contains(file.label.as_str()))
        .cloned()
        .collect();
    let added_source = SourceFiles {
        location: source.location.clone(),
        profile: source.profile.clone(),
        delta_stats: source.delta_stats.as_ref().map(|stats| {
            source
                .files
                .iter()
                .zip(stats)
                .filter(|(file, _)| !known.contains(file.label.as_str()))
                .map(|(_, stats)| stats.clone())
                .collect()
        }),
        files: added,
        by_field_id: source.by_field_id,
    };
    let mut added_files = per_file_statistics(&added_source)?;
    let selected = sketch_columns(previous, None)?;
    let scanned = scan_files(&added_source.files, previous, &selected, options)?;
    // The scan's exact bounds and counts replace the metadata bounds of the
    // added files, and its sketches fold into the table's.
    let mut sketches = Vec::with_capacity(scanned.len());
    for (file, scan) in added_files.iter_mut().zip(scanned) {
        file.rows = scan.rows;
        for (slot, column) in selected.iter().zip(&scan.columns) {
            file.columns[*slot] = FileColumnStatistics {
                min: column.min.clone(),
                max: column.max.clone(),
                null_count: Some(column.nulls),
            };
        }
        let mut file_sketches = vec![FileColumnSketches::default(); previous.columns.len()];
        for (slot, column) in selected.iter().zip(scan.columns) {
            file_sketches[*slot] = FileColumnSketches {
                distinct: column.distinct,
                quantiles: column.quantiles,
            };
        }
        sketches.push(ColumnSketches {
            columns: file_sketches,
        });
    }
    let mut next = previous.clone();
    next.append_files(added_files, Some(sketches))?;
    let after = crate::analyze_source(location, format)?;
    if after.identity_sha256 != source.profile.statistics.identity_sha256 {
        return Err(KaveonError::Storage(
            "table source changed while its statistics were being refreshed".into(),
        ));
    }
    next.source_version = source.source_version();
    next.computed_at_ms = now_ms();
    Ok(next)
}

/// The Arrow schema a statistics document describes: its columns in
/// order, every one nullable.
pub fn statistics_schema(statistics: &TableStatistics) -> SchemaRef {
    Arc::new(arrow::datatypes::Schema::new(
        statistics
            .columns
            .iter()
            .map(|column| {
                arrow::datatypes::Field::new(&column.name, column.data_type.clone(), true)
            })
            .collect::<Vec<_>>(),
    ))
}

/// `listing` without the files the statistics prove empty of rows matching
/// `predicate`: the kept listing and how many files were skipped. `None`
/// when the statistics carry no complete per-file bounds, or describe a
/// listing other than this one (a file the statistics do not know keeps
/// the whole listing: the bounds are for another version). The caller
/// establishes that the statistics are current for the listing's version.
pub fn skip_listing_files(
    listing: &DirectoryListing,
    statistics: &TableStatistics,
    predicate: &kaveon_core::StoragePredicate,
) -> Option<(DirectoryListing, u64)> {
    if !statistics.per_file_complete {
        return None;
    }
    let predicate = predicate.coerced_for(&statistics_schema(statistics));
    let names = statistics.column_names();
    let by_label: std::collections::HashMap<&str, &FileStatistics> = statistics
        .per_file
        .iter()
        .map(|file| (file.path.as_str(), file))
        .collect();
    let mut kept = Vec::with_capacity(listing.files.len());
    let mut skipped = 0u64;
    for file in &listing.files {
        let label = relative_label(&listing.root, &file.path);
        let facts = by_label.get(label.as_str())?;
        if facts.may_match(&names, &predicate) {
            kept.push(file.clone());
        } else {
            skipped += 1;
        }
    }
    if skipped == 0 {
        return None;
    }
    Some((
        DirectoryListing {
            root: listing.root.clone(),
            files: kept,
            partitions: listing.partitions.clone(),
        },
        skipped,
    ))
}

/// Whether a column's values can be folded into the sketches.
pub use kaveon_core::sketch::sketchable;

/// Whether a column's values sit on a number line for a quantile sketch.
fn numeric(data_type: &DataType) -> bool {
    kaveon_core::sketch::quantile_sketchable(data_type)
}

/// The column indexes to sketch.
fn sketch_columns(
    statistics: &TableStatistics,
    requested: Option<&[String]>,
) -> Result<Vec<usize>> {
    match requested {
        Some(names) => names
            .iter()
            .map(|name| {
                let index = statistics
                    .columns
                    .iter()
                    .position(|column| column.name == *name)
                    .ok_or_else(|| {
                        KaveonError::Execution(format!("column '{name}' does not exist"))
                    })?;
                if !sketchable(&statistics.columns[index].data_type) {
                    return Err(KaveonError::Execution(format!(
                        "column '{name}' ({}) cannot be sketched",
                        statistics.columns[index].data_type
                    )));
                }
                Ok(index)
            })
            .collect(),
        None => Ok(statistics
            .columns
            .iter()
            .enumerate()
            .filter(|(_, column)| sketchable(&column.data_type))
            .map(|(index, _)| index)
            .collect()),
    }
}

/// What one file's scan found for the selected columns.
struct FileScan {
    rows: u64,
    columns: Vec<ColumnScan>,
}

struct ColumnScan {
    nulls: u64,
    min: Option<StatValue>,
    max: Option<StatValue>,
    distinct: Option<HllSketch>,
    quantiles: Option<KllSketch>,
}

/// Read the selected columns of every file, `options.threads` files at a
/// time, each batch reserved through the memory account while it is folded.
fn scan_files(
    files: &[DataFile],
    statistics: &TableStatistics,
    selected: &[usize],
    options: &FullScanOptions,
) -> Result<Vec<FileScan>> {
    if selected.is_empty() {
        return Ok(files
            .iter()
            .map(|_| FileScan {
                rows: 0,
                columns: Vec::new(),
            })
            .collect());
    }
    let names: Vec<String> = selected
        .iter()
        .map(|index| statistics.columns[*index].name.clone())
        .collect();
    let types: Vec<DataType> = selected
        .iter()
        .map(|index| statistics.columns[*index].data_type.clone())
        .collect();
    let results: Vec<Mutex<Option<Result<FileScan>>>> =
        files.iter().map(|_| Mutex::new(None)).collect();
    let next = AtomicUsize::new(0);
    let workers = options.threads.clamp(1, files.len().max(1));
    let failed = std::sync::atomic::AtomicBool::new(false);
    std::thread::scope(|scope| {
        for _ in 0..workers {
            scope.spawn(|| {
                let sketch_reservation = options
                    .memory
                    .as_ref()
                    .map(|memory| memory.reserve(SKETCH_MEMORY_PER_COLUMN * names.len() as u64));
                if let Some(Err(error)) = sketch_reservation {
                    let index = next.fetch_add(1, AtomicOrdering::Relaxed);
                    if let Some(slot) = results.get(index) {
                        *slot.lock().unwrap_or_else(|e| e.into_inner()) = Some(Err(error));
                    }
                    failed.store(true, AtomicOrdering::Relaxed);
                    return;
                }
                loop {
                    if failed.load(AtomicOrdering::Relaxed) {
                        return;
                    }
                    let index = next.fetch_add(1, AtomicOrdering::Relaxed);
                    let Some(file) = files.get(index) else {
                        return;
                    };
                    let result = scan_file(file, &names, &types, options.memory.as_ref());
                    if result.is_err() {
                        failed.store(true, AtomicOrdering::Relaxed);
                    }
                    *results[index].lock().unwrap_or_else(|e| e.into_inner()) = Some(result);
                }
            });
        }
    });
    results
        .into_iter()
        .map(|slot| {
            slot.into_inner()
                .unwrap_or_else(|e| e.into_inner())
                .unwrap_or_else(|| Err(KaveonError::Storage("statistics scan did not run".into())))
        })
        .collect()
}

fn scan_file(
    file: &DataFile,
    names: &[String],
    types: &[DataType],
    memory: Option<&OperatorMemoryAccount>,
) -> Result<FileScan> {
    let mut source = file.open(Some(names))?;
    let mut rows = 0u64;
    let mut columns: Vec<ColumnScan> = types
        .iter()
        .map(|data_type| ColumnScan {
            nulls: 0,
            min: None,
            max: None,
            distinct: Some(HllSketch::default_precision()),
            quantiles: numeric(data_type).then(KllSketch::default_k),
        })
        .collect();
    while let Some(batch) = source.next_batch()? {
        let reservation = memory
            .map(|memory| {
                memory.check_cancelled()?;
                memory.reserve(batch.get_array_memory_size() as u64)
            })
            .transpose()?;
        rows += batch.num_rows() as u64;
        for (slot, name) in names.iter().enumerate() {
            let array = batch.column_by_name(name).ok_or_else(|| {
                KaveonError::Storage(format!("column '{name}' missing from '{}'", file.label))
            })?;
            fold_array(array, &mut columns[slot])?;
        }
        drop(reservation);
    }
    Ok(FileScan { rows, columns })
}

/// Fold one array into the column's scan: null count, exact bounds through
/// the executor's kernels, every non-null value into the sketches (the
/// core's folds, shared with the executor's approximate aggregates).
fn fold_array(array: &ArrayRef, scan: &mut ColumnScan) -> Result<()> {
    let array: ArrayRef = match array.data_type() {
        DataType::Dictionary(_, values) => compute::cast(array, values)?,
        _ => Arc::clone(array),
    };
    scan.nulls += array.null_count() as u64;
    if array.null_count() == array.len() {
        return Ok(());
    }
    macro_rules! primitive_bounds {
        ($array:expr, $to_stat:expr) => {{
            let values = $array;
            widen_bounds(
                scan,
                compute::min(values).map($to_stat),
                compute::max(values).map($to_stat),
            );
        }};
    }
    match array.data_type() {
        DataType::Boolean => {
            let values = array.as_boolean();
            widen_bounds(
                scan,
                compute::min_boolean(values).map(StatValue::Bool),
                compute::max_boolean(values).map(StatValue::Bool),
            );
        }
        DataType::Int8 => primitive_bounds!(
            array.as_primitive::<arrow::datatypes::Int8Type>(),
            |v: i8| StatValue::Int(i128::from(v))
        ),
        DataType::Int16 => primitive_bounds!(
            array.as_primitive::<arrow::datatypes::Int16Type>(),
            |v: i16| StatValue::Int(i128::from(v))
        ),
        DataType::Int32 => primitive_bounds!(
            array.as_primitive::<arrow::datatypes::Int32Type>(),
            |v: i32| StatValue::Int(i128::from(v))
        ),
        DataType::Int64 => primitive_bounds!(
            array.as_primitive::<arrow::datatypes::Int64Type>(),
            |v: i64| StatValue::Int(i128::from(v))
        ),
        DataType::UInt8 => primitive_bounds!(
            array.as_primitive::<arrow::datatypes::UInt8Type>(),
            |v: u8| StatValue::Int(i128::from(v))
        ),
        DataType::UInt16 => primitive_bounds!(
            array.as_primitive::<arrow::datatypes::UInt16Type>(),
            |v: u16| StatValue::Int(i128::from(v))
        ),
        DataType::UInt32 => primitive_bounds!(
            array.as_primitive::<arrow::datatypes::UInt32Type>(),
            |v: u32| StatValue::Int(i128::from(v))
        ),
        DataType::UInt64 => primitive_bounds!(
            array.as_primitive::<arrow::datatypes::UInt64Type>(),
            |v: u64| StatValue::Int(i128::from(v))
        ),
        DataType::Float32 => primitive_bounds!(
            array.as_primitive::<arrow::datatypes::Float32Type>(),
            |v: f32| StatValue::Float(f64::from(v))
        ),
        DataType::Float64 => primitive_bounds!(
            array.as_primitive::<arrow::datatypes::Float64Type>(),
            StatValue::Float
        ),
        DataType::Date32 => primitive_bounds!(
            array.as_primitive::<arrow::datatypes::Date32Type>(),
            StatValue::Date
        ),
        DataType::Timestamp(unit, zone) => {
            let (unit, utc) = (*unit, zone.is_some());
            let values = kaveon_core::sketch::timestamp_values(&array, unit);
            let to_stat = move |value: i64| StatValue::Timestamp { value, unit, utc };
            widen_bounds(
                scan,
                compute::min(&values).map(to_stat),
                compute::max(&values).map(to_stat),
            );
        }
        DataType::Decimal128(_, scale) => {
            let scale = *scale;
            primitive_bounds!(
                array.as_primitive::<arrow::datatypes::Decimal128Type>(),
                move |unscaled: i128| StatValue::Decimal { unscaled, scale }
            )
        }
        DataType::Utf8 => {
            let values = array.as_string::<i32>();
            widen_bounds(
                scan,
                compute::min_string(values).map(|v| StatValue::Text(v.to_owned())),
                compute::max_string(values).map(|v| StatValue::Text(v.to_owned())),
            );
        }
        DataType::LargeUtf8 => {
            let values = array.as_string::<i64>();
            widen_bounds(
                scan,
                compute::min_string(values).map(|v| StatValue::Text(v.to_owned())),
                compute::max_string(values).map(|v| StatValue::Text(v.to_owned())),
            );
        }
        other => {
            return Err(KaveonError::Storage(format!(
                "column type {other} cannot be sketched"
            )));
        }
    }
    if let Some(distinct) = scan.distinct.as_mut() {
        kaveon_core::sketch::fold_distinct(&array, distinct)?;
    }
    if let Some(quantiles) = scan.quantiles.as_mut() {
        kaveon_core::sketch::fold_quantiles(&array, quantiles)?;
    }
    Ok(())
}

fn widen_bounds(scan: &mut ColumnScan, min: Option<StatValue>, max: Option<StatValue>) {
    if let Some(min) = min {
        scan.min = match scan.min.take() {
            None => Some(min),
            Some(current) => match current.partial_cmp(&min) {
                Some(Ordering::Greater) => Some(min),
                _ => Some(current),
            },
        };
    }
    if let Some(max) = max {
        scan.max = match scan.max.take() {
            None => Some(max),
            Some(current) => match current.partial_cmp(&max) {
                Some(Ordering::Less) => Some(max),
                _ => Some(current),
            },
        };
    }
}

/// Fold the files' scans into the table: exact bounds and null counts for
/// the selected columns, merged sketches, and the row count the scan saw.
fn apply_scan(statistics: &mut TableStatistics, selected: &[usize], scanned: Vec<FileScan>) {
    if selected.is_empty() {
        return;
    }
    let rows: u64 = scanned.iter().map(|scan| scan.rows).sum();
    statistics.rows = rows;
    for (position, slot) in selected.iter().enumerate() {
        let column = &mut statistics.columns[*slot];
        let mut nulls = 0u64;
        let mut min: Option<StatValue> = None;
        let mut max: Option<StatValue> = None;
        let mut distinct: Option<HllSketch> = Some(HllSketch::default_precision());
        let mut quantiles: Option<KllSketch> =
            numeric(&column.data_type).then(KllSketch::default_k);
        for scan in &scanned {
            let Some(found) = scan.columns.get(position) else {
                continue;
            };
            nulls += found.nulls;
            let mut bounds = ColumnScan {
                nulls: 0,
                min: min.take(),
                max: max.take(),
                distinct: None,
                quantiles: None,
            };
            widen_bounds(&mut bounds, found.min.clone(), found.max.clone());
            min = bounds.min;
            max = bounds.max;
            match (&mut distinct, &found.distinct) {
                (Some(mine), Some(theirs)) => {
                    if mine.merge(theirs).is_err() {
                        distinct = None;
                    }
                }
                _ => distinct = None,
            }
            match (&mut quantiles, &found.quantiles) {
                (Some(mine), Some(theirs)) => {
                    if mine.merge(theirs).is_err() {
                        quantiles = None;
                    }
                }
                (Some(_), None) => quantiles = None,
                (None, _) => {}
            }
        }
        column.null_count = Some(nulls);
        column.min = min;
        column.max = max;
        column.bounds_exact = true;
        column.distinct = distinct;
        column.distinct_exact = None;
        column.quantiles = quantiles;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use arrow::{
        array::{ArrayRef, Date32Array, Float64Array, Int64Array, StringArray},
        datatypes::{Field, Schema},
        record_batch::RecordBatch,
    };
    use kaveon_core::SourceVersionKind;
    use kaveon_core::{CompareOp, QueryMemoryPool, ScalarValue, StoragePredicate};
    use parquet::{arrow::ArrowWriter, file::properties::WriterProperties};
    use std::fs::File;

    fn schema() -> SchemaRef {
        Arc::new(Schema::new(vec![
            Field::new("id", DataType::Int64, false),
            Field::new("name", DataType::Utf8, true),
            Field::new("score", DataType::Float64, true),
            Field::new("day", DataType::Date32, true),
        ]))
    }

    fn batch(ids: std::ops::Range<i64>) -> RecordBatch {
        RecordBatch::try_new(
            schema(),
            vec![
                Arc::new(Int64Array::from_iter_values(ids.clone())) as ArrayRef,
                Arc::new(StringArray::from_iter(
                    ids.clone()
                        .map(|id| (id % 7 != 0).then(|| format!("name-{}", id % 50))),
                )) as ArrayRef,
                Arc::new(Float64Array::from_iter(
                    ids.clone().map(|id| Some(id as f64 / 10.0)),
                )) as ArrayRef,
                Arc::new(Date32Array::from_iter(
                    ids.clone().map(|id| Some(20_000 + (id % 365) as i32)),
                )) as ArrayRef,
            ],
        )
        .unwrap()
    }

    fn write(path: &std::path::Path, ids: std::ops::Range<i64>) {
        let properties = WriterProperties::builder()
            .set_max_row_group_size(1000)
            .build();
        let mut writer =
            ArrowWriter::try_new(File::create(path).unwrap(), schema(), Some(properties)).unwrap();
        writer.write(&batch(ids)).unwrap();
        writer.close().unwrap();
    }

    fn temp(name: &str) -> PathBuf {
        let directory = std::env::temp_dir().join(format!(
            "kaveon-table-statistics-{name}-{}-{}",
            std::process::id(),
            uuid_like()
        ));
        std::fs::create_dir_all(&directory).unwrap();
        directory
    }

    fn uuid_like() -> u128 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    }

    fn table_id() -> TableId {
        TableId::new("table:test").unwrap()
    }

    fn assert_close(actual: Option<u64>, expected: u64) {
        let actual = actual.expect("a distinct count") as f64;
        let error = (actual - expected as f64).abs() / expected as f64;
        assert!(error < 0.03, "{actual} is not within 3 % of {expected}");
    }

    #[test]
    fn metadata_and_full_statistics_over_a_parquet_directory() {
        let directory = temp("directory");
        write(&directory.join("a.parquet"), 0..3_000);
        write(&directory.join("b.parquet"), 3_000..5_000);
        write(&directory.join("c.parquet"), 5_000..12_000);
        let location = directory.to_str().unwrap();

        let metadata = metadata_statistics(location, DataFormat::Parquet, table_id()).unwrap();
        assert_eq!(metadata.depth, StatisticsDepth::Metadata);
        assert_eq!(metadata.rows, 12_000);
        assert_eq!(metadata.files, 3);
        assert!(metadata.per_file_complete);
        assert_eq!(metadata.per_file.len(), 3);
        assert_eq!(
            metadata
                .per_file
                .iter()
                .map(|f| f.path.as_str())
                .collect::<Vec<_>>(),
            ["a.parquet", "b.parquet", "c.parquet"]
        );
        assert_eq!(metadata.per_file[1].rows, 2_000);
        assert_eq!(
            metadata.per_file[1].columns[0].min,
            Some(StatValue::Int(3_000))
        );
        assert_eq!(
            metadata.per_file[1].columns[0].max,
            Some(StatValue::Int(4_999))
        );
        assert!(matches!(
            metadata.source_version.kind,
            SourceVersionKind::Listing { files: 3 }
        ));
        let id = metadata.column("id").unwrap();
        assert_eq!(id.min, Some(StatValue::Int(0)));
        assert_eq!(id.max, Some(StatValue::Int(11_999)));
        assert_eq!(id.null_count, Some(0));
        assert!(id.bounds_exact);
        assert!(id.distinct.is_none());
        let name = metadata.column("name").unwrap();
        assert_eq!(name.null_count, Some(12_000 / 7 + 1));
        // The Rust Parquet writer records the exactness flags.
        assert!(name.bounds_exact);
        // File skipping from the per-file bounds.
        let (kept, skipped) = metadata.partition_files(&StoragePredicate::Compare {
            column: "id".into(),
            op: CompareOp::Ge,
            value: ScalarValue::Int64(5_000),
        });
        assert_eq!(kept.len(), 1);
        assert_eq!(skipped, 2);

        let pool = QueryMemoryPool::new("analyze", 64 * 1024 * 1024).unwrap();
        let options = FullScanOptions {
            memory: Some(pool.operator("analyze").unwrap()),
            threads: 3,
            columns: None,
        };
        let full = full_statistics(location, DataFormat::Parquet, table_id(), &options).unwrap();
        assert_eq!(full.depth, StatisticsDepth::Full);
        assert_eq!(full.rows, 12_000);
        assert_eq!(full.source_version, metadata.source_version);
        let id = full.column("id").unwrap();
        let distinct = id.distinct_count().unwrap() as f64;
        assert!((distinct - 12_000.0).abs() / 12_000.0 < 0.03, "{distinct}");
        let median = id.quantiles.as_ref().unwrap().quantile(0.5).unwrap();
        assert!((median - 6_000.0).abs() < 300.0, "{median}");
        let name = full.column("name").unwrap();
        assert_close(name.distinct_count(), 50);
        assert_eq!(name.min, Some(StatValue::Text("name-0".into())));
        assert_eq!(name.max, Some(StatValue::Text("name-9".into())));
        assert!(name.bounds_exact);
        assert_eq!(name.null_count, Some(12_000 / 7 + 1));
        assert!(name.quantiles.is_none());
        let day = full.column("day").unwrap();
        assert_close(day.distinct_count(), 365);
        assert_eq!(day.min, Some(StatValue::Date(20_000)));
        let score = full.column("score").unwrap();
        assert_eq!(score.max, Some(StatValue::Float(1_199.9)));
        assert_eq!(pool.snapshot().current_bytes, 0);

        // The document round-trips.
        let bytes = full.to_json_bytes().unwrap();
        assert_eq!(TableStatistics::from_json_bytes(&bytes).unwrap(), full);

        // A new file folds in without re-reading the old ones: the same
        // sketch as a full build over all four.
        write(&directory.join("d.parquet"), 12_000..13_000);
        let refreshed = refresh_statistics(&full, location, DataFormat::Parquet, &options).unwrap();
        assert_eq!(refreshed.depth, StatisticsDepth::Full);
        assert_eq!(refreshed.rows, 13_000);
        assert_eq!(refreshed.files, 4);
        assert_eq!(refreshed.per_file.len(), 4);
        assert_ne!(refreshed.source_version, full.source_version);
        let rebuilt = full_statistics(location, DataFormat::Parquet, table_id(), &options).unwrap();
        assert_eq!(refreshed.source_version, rebuilt.source_version);
        assert_eq!(
            refreshed.column("id").unwrap().distinct,
            rebuilt.column("id").unwrap().distinct
        );
        assert_eq!(
            refreshed.column("id").unwrap().max,
            Some(StatValue::Int(12_999))
        );
        assert_eq!(
            refreshed.column("name").unwrap().null_count,
            rebuilt.column("name").unwrap().null_count
        );
        // The same version again is the same document.
        let same = refresh_statistics(&refreshed, location, DataFormat::Parquet, &options).unwrap();
        assert_eq!(same, refreshed);
        // A removed file recomputes.
        std::fs::remove_file(directory.join("a.parquet")).unwrap();
        let removed =
            refresh_statistics(&refreshed, location, DataFormat::Parquet, &options).unwrap();
        assert_eq!(removed.rows, 10_000);
        assert_eq!(removed.files, 3);
        assert_eq!(
            removed.column("id").unwrap().min,
            Some(StatValue::Int(3_000))
        );
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn delta_statistics_come_from_the_log_and_refresh_by_version() {
        let directory = temp("delta");
        let log = directory.join("_delta_log");
        std::fs::create_dir_all(&log).unwrap();
        write(&directory.join("a.parquet"), 0..100);
        write(&directory.join("b.parquet"), 100..300);
        let stats = |lo: i64, hi: i64, rows: u64| {
            serde_json::json!({
                "numRecords": rows,
                "minValues": {"id": lo, "name": "name-0", "score": lo as f64 / 10.0},
                "maxValues": {"id": hi, "name": "name-9", "score": hi as f64 / 10.0},
                "nullCount": {"id": 0, "name": 3, "score": 0}
            })
            .to_string()
        };
        let schema = serde_json::json!({"type":"struct","fields":[
            {"name":"id","type":"long","nullable":false,"metadata":{}},
            {"name":"name","type":"string","nullable":true,"metadata":{}},
            {"name":"score","type":"double","nullable":true,"metadata":{}},
            {"name":"day","type":"date","nullable":true,"metadata":{}}
        ]});
        let lines = [
            serde_json::json!({"protocol":{"minReaderVersion":1,"minWriterVersion":2}}).to_string(),
            serde_json::json!({"metaData":{"id":"t","format":{"provider":"parquet"},"schemaString":schema.to_string(),"partitionColumns":[]}}).to_string(),
            serde_json::json!({"add":{"path":"a.parquet","size":10,"dataChange":true,"stats":stats(0, 99, 100)}}).to_string(),
            serde_json::json!({"add":{"path":"b.parquet","size":20,"dataChange":true,"stats":stats(100, 299, 200)}}).to_string(),
        ];
        std::fs::write(log.join("00000000000000000000.json"), lines.join("\n")).unwrap();
        let location = directory.to_str().unwrap();
        let metadata = metadata_statistics(location, DataFormat::Delta, table_id()).unwrap();
        assert_eq!(metadata.rows, 300);
        assert_eq!(metadata.files, 2);
        assert_eq!(metadata.bytes, 30);
        assert!(matches!(
            metadata.source_version.kind,
            SourceVersionKind::DeltaVersion { version: 0 }
        ));
        assert_eq!(metadata.per_file.len(), 2);
        assert_eq!(metadata.per_file[0].path, "a.parquet");
        assert_eq!(metadata.per_file[0].rows, 100);
        assert_eq!(
            metadata.per_file[0].columns[0].max,
            Some(StatValue::Int(99))
        );
        assert_eq!(metadata.per_file[1].columns[1].null_count, Some(3));
        // Day has no stats in the log: unknown, never zero.
        assert_eq!(metadata.per_file[1].columns[3].min, None);
        let name = metadata.column("name").unwrap();
        assert_eq!(name.min, Some(StatValue::Text("name-0".into())));
        // Delta text bounds are not flagged exact.
        assert!(!name.bounds_exact);
        assert!(metadata.column("id").unwrap().bounds_exact);

        let options = FullScanOptions {
            memory: None,
            threads: 2,
            columns: Some(vec!["id".into(), "name".into()]),
        };
        let full = full_statistics(location, DataFormat::Delta, table_id(), &options).unwrap();
        assert_close(full.column("id").unwrap().distinct_count(), 300);
        assert!(full.column("name").unwrap().bounds_exact);
        assert!(full.column("score").unwrap().distinct.is_none());
        assert!(full.column("day").unwrap().distinct.is_none());

        // A commit that adds a file folds in at the new version.
        write(&directory.join("c.parquet"), 300..350);
        std::fs::write(
            log.join("00000000000000000001.json"),
            serde_json::json!({"add":{"path":"c.parquet","size":5,"dataChange":true,"stats":stats(300, 349, 50)}}).to_string(),
        )
        .unwrap();
        let refreshed = refresh_statistics(&full, location, DataFormat::Delta, &options).unwrap();
        assert!(matches!(
            refreshed.source_version.kind,
            SourceVersionKind::DeltaVersion { version: 1 }
        ));
        assert_eq!(refreshed.rows, 350);
        assert_eq!(refreshed.files, 3);
        assert_close(refreshed.column("id").unwrap().distinct_count(), 350);
        assert_eq!(
            refreshed.column("id").unwrap().max,
            Some(StatValue::Int(349))
        );
        // A commit that removes a file recomputes.
        std::fs::write(
            log.join("00000000000000000002.json"),
            serde_json::json!({"remove":{"path":"a.parquet","dataChange":true}}).to_string(),
        )
        .unwrap();
        let after_remove =
            refresh_statistics(&refreshed, location, DataFormat::Delta, &options).unwrap();
        assert_eq!(after_remove.rows, 250);
        assert_eq!(
            after_remove.column("id").unwrap().min,
            Some(StatValue::Int(100))
        );
        assert_close(after_remove.column("id").unwrap().distinct_count(), 250);
        // A metadata-only document refreshes on the metadata path.
        let cheap = refresh_statistics(
            &metadata,
            location,
            DataFormat::Delta,
            &FullScanOptions::default(),
        )
        .unwrap();
        assert_eq!(cheap.depth, StatisticsDepth::Metadata);
        assert_eq!(cheap.rows, 250);
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn a_source_that_changes_during_the_scan_is_refused_and_memory_is_bounded() {
        let directory = temp("memory");
        write(&directory.join("a.parquet"), 0..20_000);
        let location = directory.to_str().unwrap();
        let pool = QueryMemoryPool::new("analyze", 16 * 1024).unwrap();
        let options = FullScanOptions {
            memory: Some(pool.operator("analyze").unwrap()),
            threads: 1,
            columns: None,
        };
        let error = full_statistics(location, DataFormat::Parquet, table_id(), &options)
            .unwrap_err()
            .to_string();
        assert!(error.contains("reserve"), "{error}");
        assert_eq!(pool.snapshot().current_bytes, 0);
        std::fs::remove_dir_all(directory).unwrap();
    }
}
