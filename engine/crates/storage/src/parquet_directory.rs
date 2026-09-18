//! Directory Parquet tables.
//!
//! A catalog location that names a directory of Parquet files rather than one
//! object — the Hive/Spark layout, what Trino writes — is a table. The
//! directory is listed once per scan, the listing is sorted so every reader
//! of it sees the same files in the same order, the files are spread over the
//! scan partitions by size, and every file is read through the per-object
//! reader with its own identity-pinned footer cache. The schema is the first
//! file's; every other file is checked against it and a difference is an
//! error naming the file, never a cast.
//!
//! Which objects are data files follows Hive and Spark: an object whose name,
//! or any directory below the root, begins with `_` or `.` is hidden
//! (`_SUCCESS`, `_delta_log/…`, `.part-….crc`); a zero-byte object holds no
//! rows and is skipped; every other object is data when it carries the
//! `.parquet` extension or no extension at all (Trino's layout). An object
//! with any other extension is an error naming it, so a stray file cannot be
//! silently read as data or silently dropped from the table.
//!
//! The listing is not carried to the workers of a distributed query: the
//! executable fragment names the location and every task lists it under the
//! same deterministic rule (the fragment wire format is unchanged). The
//! coordinator pins its own listing per query through the planning source
//! pins, the way it pins Delta versions.

use std::sync::{Arc, mpsc};

use arrow::{datatypes::SchemaRef, record_batch::RecordBatch};
use futures::{StreamExt, TryStreamExt, stream};
use kaveon_core::{BatchSource, Result, StoragePredicate};
use object_store::{ObjectStore, path::Path};

use crate::{
    ParquetFileMetadata, ScanMetrics, ScanPartition,
    adls_reader::{AdlsParquetReader, OpenError, adls_store, scan_parallelism},
    object_reader::{ObjectLocation, error, storage_error},
    parquet_reader::projection_indices,
};

const DEFAULT_BATCH_SIZE: usize = 8_192;
const METADATA_READ_CONCURRENCY: usize = 16;
/// A partition may carry this fraction more than its fair share of bytes in
/// whole files before the largest whole file is split by row group instead:
/// one quarter. Below it, whole files keep their locality (one footer, one
/// full-object cache entry, sequential ranges on one reader).
const IMBALANCE_TOLERANCE_NUMERATOR: u128 = 5;
const IMBALANCE_TOLERANCE_DENOMINATOR: u128 = 4;

/// One data file of a directory table, as listed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DirectoryFile {
    /// Store-relative path of the object.
    pub path: Path,
    pub size: u64,
    pub e_tag: Option<String>,
    pub version: Option<String>,
    /// Last modification, nanoseconds since the epoch, for identity only.
    pub modified_nanos: i64,
}

impl DirectoryFile {
    /// The immutable identity the store gives the object: its ETag or version.
    pub fn identity(&self) -> Option<&str> {
        self.e_tag.as_deref().or(self.version.as_deref())
    }
}

/// The data files under a directory root, sorted by path.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DirectoryListing {
    pub root: Path,
    pub files: Vec<DirectoryFile>,
}

impl DirectoryListing {
    pub fn total_bytes(&self) -> u64 {
        self.files
            .iter()
            .fold(0_u64, |total, file| total.saturating_add(file.size))
    }

    pub fn sizes(&self) -> Vec<u64> {
        self.files.iter().map(|file| file.size).collect()
    }

    /// One line per file — path, size, identity — for digests.
    pub fn identity_lines(&self) -> String {
        let mut lines = String::new();
        for file in &self.files {
            lines.push_str(file.path.as_ref());
            lines.push('\t');
            lines.push_str(&file.size.to_string());
            lines.push('\t');
            lines.push_str(file.identity().unwrap_or_default());
            lines.push('\n');
        }
        lines
    }
}

/// Whether a listed object is data, hidden, or something a table must not
/// silently contain.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum FileVerdict {
    Data,
    Hidden,
    Foreign,
}

/// Classify an object by its path relative to the directory root.
pub(crate) fn file_verdict<'a>(relative: impl Iterator<Item = &'a str>) -> FileVerdict {
    let mut name = "";
    for segment in relative {
        if segment.starts_with('_') || segment.starts_with('.') {
            return FileVerdict::Hidden;
        }
        name = segment;
    }
    let extension = name.rsplit_once('.').map(|(_, extension)| extension);
    match extension {
        None => FileVerdict::Data,
        Some(extension) if extension.eq_ignore_ascii_case("parquet") => FileVerdict::Data,
        Some(_) => FileVerdict::Foreign,
    }
}

/// List the data files under `root`. The store's listing is recursive, so a
/// partitioned layout (`year=2024/part-0.parquet`) is included; the values in
/// those directory names are not surfaced as columns.
pub async fn list_parquet_directory(
    store: &dyn ObjectStore,
    root: &Path,
) -> Result<DirectoryListing> {
    let prefix = (!root.as_ref().is_empty()).then_some(root);
    let mut objects = store.list(prefix);
    let mut files = Vec::new();
    while let Some(object) = objects.try_next().await.map_err(storage_error)? {
        let Some(relative) = object.location.prefix_match(root) else {
            continue;
        };
        let relative = relative
            .map(|segment| segment.as_ref().to_owned())
            .collect::<Vec<_>>();
        match file_verdict(relative.iter().map(String::as_str)) {
            FileVerdict::Hidden => continue,
            FileVerdict::Foreign => {
                return Err(error(format!(
                    "Parquet directory '{root}' contains '{}', which is neither a .parquet file nor \
                     an extension-less data file; remove it or hide it with a leading '_' or '.'",
                    object.location
                )));
            }
            FileVerdict::Data => {}
        }
        if object.size == 0 {
            continue;
        }
        files.push(DirectoryFile {
            path: object.location,
            size: u64::try_from(object.size).map_err(storage_error)?,
            e_tag: object.e_tag,
            version: object.version,
            modified_nanos: object
                .last_modified
                .timestamp_nanos_opt()
                .unwrap_or_default(),
        });
    }
    files.sort_by(|left, right| left.path.as_ref().cmp(right.path.as_ref()));
    Ok(DirectoryListing {
        root: root.clone(),
        files,
    })
}

/// Which files of a listing one scan partition reads, and how.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct FileAssignment {
    /// Files this partition reads whole, as indices into the listing.
    pub whole: Vec<usize>,
    /// Files every partition shares by row group: the partition applies
    /// inside each of them, exactly as it does to a single-file table.
    pub split: Vec<usize>,
}

impl FileAssignment {
    /// The files this partition opens, in listing order, with whether the
    /// partition applies inside the file.
    pub fn files(&self) -> Vec<(usize, bool)> {
        let mut files = self
            .whole
            .iter()
            .map(|index| (*index, false))
            .chain(self.split.iter().map(|index| (*index, true)))
            .collect::<Vec<_>>();
        files.sort_unstable();
        files
    }

    pub fn len(&self) -> usize {
        self.whole.len() + self.split.len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

/// Spread files over the scan partitions by size. Whole files go to the
/// partition with the fewest bytes so far, largest first (ties by listing
/// order). If the heaviest partition would then carry more than a quarter
/// over its fair share, the largest whole file is split by row group across
/// every partition instead, and the rest are placed again; a single-file
/// table therefore splits by row group as it always has. The result is a
/// pure function of the sizes and the partition count, so every task of a
/// query derives the same assignment from the same listing.
pub fn assign_files(sizes: &[u64], partition: ScanPartition) -> FileAssignment {
    if partition.count == 1 {
        return FileAssignment {
            whole: (0..sizes.len()).collect(),
            split: Vec::new(),
        };
    }
    let count = partition.count;
    let total = sizes.iter().map(|size| u128::from(*size)).sum::<u128>();
    let fair_share = total.div_ceil(count as u128);
    let mut whole_order = (0..sizes.len()).collect::<Vec<_>>();
    whole_order.sort_by(|left, right| sizes[*right].cmp(&sizes[*left]).then(left.cmp(right)));
    let mut split = Vec::new();
    let mut split_bytes = 0_u128;
    let mut owner = vec![0_usize; sizes.len()];
    loop {
        let mut loads = vec![0_u128; count];
        for index in &whole_order {
            let lightest = (0..count)
                .min_by_key(|candidate| (loads[*candidate], *candidate))
                .expect("partition count is positive");
            loads[lightest] += u128::from(sizes[*index]);
            owner[*index] = lightest;
        }
        let heaviest =
            loads.iter().copied().max().unwrap_or_default() + split_bytes / count as u128;
        let balanced = heaviest * IMBALANCE_TOLERANCE_DENOMINATOR
            <= fair_share * IMBALANCE_TOLERANCE_NUMERATOR;
        if balanced || whole_order.is_empty() {
            break;
        }
        let largest = whole_order.remove(0);
        split_bytes += u128::from(sizes[largest]);
        split.push(largest);
    }
    let mut whole = whole_order
        .into_iter()
        .filter(|index| owner[*index] == partition.index)
        .collect::<Vec<_>>();
    whole.sort_unstable();
    split.sort_unstable();
    FileAssignment { whole, split }
}

/// The schema every file of a directory table must present: the first file's
/// names, order and types. A file may declare a column non-nullable where the
/// first declares it nullable (its arrays fit), never the reverse.
pub(crate) fn check_file_schema(
    root: &str,
    first: &str,
    expected: &SchemaRef,
    file: &str,
    actual: &SchemaRef,
) -> Result<()> {
    if expected.fields().len() != actual.fields().len() {
        return Err(error(format!(
            "Parquet directory '{root}': '{file}' has {} columns where '{first}' has {}; a \
             directory table has one schema",
            actual.fields().len(),
            expected.fields().len()
        )));
    }
    for (position, (expected, actual)) in expected.fields().iter().zip(actual.fields()).enumerate()
    {
        if expected.name() != actual.name() {
            return Err(error(format!(
                "Parquet directory '{root}': '{file}' has column '{}' at position {position} \
                 where '{first}' has '{}'; a directory table has one schema",
                actual.name(),
                expected.name()
            )));
        }
        if expected.data_type() != actual.data_type() {
            return Err(error(format!(
                "Parquet directory '{root}': column '{}' is {} in '{file}' but {} in '{first}'; a \
                 directory table has one schema and no file is cast to another's",
                expected.name(),
                actual.data_type(),
                expected.data_type()
            )));
        }
        if actual.is_nullable() && !expected.is_nullable() {
            return Err(error(format!(
                "Parquet directory '{root}': column '{}' is nullable in '{file}' but not in \
                 '{first}'; a directory table has one schema",
                expected.name()
            )));
        }
    }
    Ok(())
}

/// The schema a scan advertises: the file schema, projected in the caller's
/// column order.
pub(crate) fn advertised_schema(
    schema: &SchemaRef,
    columns: Option<&[String]>,
) -> Result<SchemaRef> {
    let Some(columns) = columns else {
        return Ok(Arc::clone(schema));
    };
    let indices = projection_indices(schema, columns)?;
    Ok(Arc::new(schema.project(&indices).map_err(storage_error)?))
}

/// What a Parquet location holds.
#[derive(Debug)]
pub enum ParquetLocation {
    Object(object_store::ObjectMeta),
    Directory(DirectoryListing),
}

/// A directory table on an object store.
#[derive(Clone)]
pub struct ObjectDirectoryReader {
    store: Arc<dyn ObjectStore>,
    account: String,
    container: String,
    root: Path,
    listing: Option<Arc<DirectoryListing>>,
    batch_size: usize,
    columns: Option<Vec<String>>,
    predicate: Option<StoragePredicate>,
    partition: Option<ScanPartition>,
    metrics: ScanMetrics,
}

impl ObjectDirectoryReader {
    /// `account` and `container` name the per-file cache namespace; for ADLS
    /// they are the real ones so the caches are shared with single-object
    /// reads of the same files.
    pub fn new(
        store: Arc<dyn ObjectStore>,
        account: impl Into<String>,
        container: impl Into<String>,
        root: Path,
    ) -> Self {
        Self {
            store,
            account: account.into(),
            container: container.into(),
            root,
            listing: None,
            batch_size: DEFAULT_BATCH_SIZE,
            columns: None,
            predicate: None,
            partition: None,
            metrics: ScanMetrics::default(),
        }
    }

    /// An `abfss://` location through the cached Azure client, an `s3://`
    /// location through the S3 client; credentials come from the environment.
    pub fn from_uri(uri: &str) -> Result<Self> {
        let uri = uri.trim_end_matches('/');
        if uri.starts_with("abfss://") {
            let reader = AdlsParquetReader::from_abfss_uri(uri)?;
            let (account, container, root) = reader.namespace_and_path();
            let store = adls_store(
                &account,
                &container,
                Default::default(),
                &ScanMetrics::default(),
            )?;
            return Ok(Self::new(
                store,
                account,
                container,
                Path::parse(root).map_err(storage_error)?,
            ));
        }
        let bucket = uri
            .strip_prefix("s3://")
            .and_then(|rest| rest.split_once('/'))
            .map(|(bucket, _)| bucket.to_owned())
            .ok_or_else(|| error("expected s3:// or abfss:// object URI"))?;
        let location = ObjectLocation::from_uri(uri)?;
        Ok(Self::new(location.store, "s3", bucket, location.path))
    }

    /// Read a listing already taken for this query instead of listing again.
    pub fn with_listing(mut self, listing: Arc<DirectoryListing>) -> Self {
        self.listing = Some(listing);
        self
    }
    pub fn with_batch_size(mut self, value: usize) -> Self {
        self.batch_size = value;
        self
    }
    pub fn with_columns(mut self, value: Vec<String>) -> Self {
        self.columns = Some(value);
        self
    }
    pub fn with_predicate(mut self, value: StoragePredicate) -> Self {
        self.predicate = Some(match self.predicate.take() {
            Some(previous) => StoragePredicate::And(vec![previous, value]),
            None => value,
        });
        self
    }
    pub fn with_partition(mut self, value: ScanPartition) -> Self {
        self.partition = Some(value);
        self
    }
    pub fn with_metrics(mut self, value: ScanMetrics) -> Self {
        self.metrics = value;
        self
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn store(&self) -> Arc<dyn ObjectStore> {
        Arc::clone(&self.store)
    }

    /// The pinned listing, or the directory listed now.
    pub async fn listing(&self) -> Result<Arc<DirectoryListing>> {
        if let Some(listing) = &self.listing {
            return Ok(Arc::clone(listing));
        }
        let started = std::time::Instant::now();
        let listing = list_parquet_directory(self.store.as_ref(), &self.root).await?;
        self.metrics.snapshot_time(started.elapsed());
        Ok(Arc::new(listing))
    }

    /// Whether the location is one object or a directory of data files. A
    /// `HEAD` that finds no object — ADLS answers that for a directory, S3
    /// for any prefix — is followed by one listing.
    pub async fn probe(&self) -> Result<ParquetLocation> {
        match self.store.head(&self.root).await {
            Ok(object) => Ok(ParquetLocation::Object(object)),
            Err(object_store::Error::NotFound { .. }) => {
                let listing = self.listing().await?;
                if listing.files.is_empty() {
                    return Err(error(format!(
                        "location '{}' is neither an object nor a directory of Parquet files",
                        self.root
                    )));
                }
                Ok(ParquetLocation::Directory(Arc::unwrap_or_clone(listing)))
            }
            Err(failure) => Err(storage_error(failure)),
        }
    }

    fn file_reader(&self, file: &DirectoryFile) -> AdlsParquetReader {
        let mut reader = AdlsParquetReader::over_store(
            Arc::clone(&self.store),
            &self.account,
            &self.container,
            file.path.as_ref(),
        )
        .with_batch_size(self.batch_size);
        if let Some(columns) = &self.columns {
            reader = reader.with_columns(columns.clone());
        }
        if let Some(predicate) = &self.predicate {
            reader = reader.with_predicate(predicate.clone());
        }
        reader
    }

    /// Exact metadata over every file: the first file's schema, the summed
    /// row and row-group counts, every file checked against the schema.
    pub async fn metadata(&self) -> Result<ParquetFileMetadata> {
        let listing = self.listing().await?;
        let Some(first) = listing.files.first() else {
            return Err(error(format!(
                "location '{}' holds no Parquet data files",
                self.root
            )));
        };
        let metrics = ScanMetrics::default();
        let readers = listing
            .files
            .iter()
            .map(|file| (file.path.clone(), self.file_reader(file)))
            .collect::<Vec<_>>();
        let mut opened = stream::iter(readers.into_iter().map(|(path, reader)| {
            let store = Arc::clone(&self.store);
            let metrics = metrics.clone();
            async move {
                reader
                    .open(store, &metrics)
                    .await
                    .map(|opened| (path, opened))
                    .map_err(kaveon_core::KaveonError::from)
            }
        }))
        .buffered(METADATA_READ_CONCURRENCY);
        let (_, head) = opened
            .next()
            .await
            .ok_or_else(|| error("directory listing is empty"))??;
        let schema = Arc::clone(head.schema());
        let mut row_count = head.row_count()?;
        let mut row_group_count = head.row_group_count();
        let mut profile = head.profile();
        while let Some(next) = opened.next().await {
            let (path, next) = next?;
            check_file_schema(
                self.root.as_ref(),
                first.path.as_ref(),
                &schema,
                path.as_ref(),
                next.schema(),
            )?;
            row_count = row_count
                .checked_add(next.row_count()?)
                .ok_or_else(|| error("Parquet directory row count overflow"))?;
            row_group_count = row_group_count
                .checked_add(next.row_group_count())
                .ok_or_else(|| error("Parquet directory row-group count overflow"))?;
            profile.merge(next.profile());
        }
        Ok(ParquetFileMetadata {
            schema,
            row_count,
            row_group_count,
            profile,
        })
    }

    /// Stream the partition's files on a reader thread with its own runtime.
    pub fn read_blocking(self) -> Result<ObjectDirectorySource> {
        if self.batch_size == 0 {
            return Err(error("batch size must be greater than zero"));
        }
        let (initial_tx, initial_rx) = mpsc::sync_channel(1);
        let (tx, rx) = mpsc::sync_channel(2);
        std::thread::Builder::new()
            .name("kaveon-directory-reader".into())
            .spawn(move || {
                let runtime = match tokio::runtime::Builder::new_multi_thread()
                    .worker_threads(scan_parallelism())
                    .enable_all()
                    .build()
                {
                    Ok(runtime) => runtime,
                    Err(failure) => {
                        let _ = initial_tx.send(Err(storage_error(failure)));
                        return;
                    }
                };
                runtime.block_on(self.run(initial_tx, tx));
            })
            .map_err(storage_error)?;
        let (schema, metrics) = initial_rx
            .recv()
            .map_err(|_| error("directory reader terminated before initialization"))??;
        Ok(ObjectDirectorySource {
            schema,
            receiver: rx,
            metrics,
            exhausted: false,
        })
    }

    /// The reader loop: list, assign, then one file after another through the
    /// per-object reader. The advertised schema and the metrics handle go out
    /// first; batches follow, re-wrapped to the advertised schema; `None`
    /// ends the stream and an error ends it early.
    pub(crate) async fn run(
        self,
        initial_tx: mpsc::SyncSender<Result<(SchemaRef, ScanMetrics)>>,
        tx: mpsc::SyncSender<Result<Option<RecordBatch>>>,
    ) {
        let (schema, files) = match self.prepare().await {
            Ok(prepared) => prepared,
            Err(failure) => {
                let _ = initial_tx.send(Err(failure));
                return;
            }
        };
        if initial_tx
            .send(Ok((Arc::clone(&schema), self.metrics.clone())))
            .is_err()
        {
            return;
        }
        for (file, split) in files {
            let mut reader = self.file_reader(&file);
            if split && let Some(partition) = self.partition {
                reader = reader.with_partition(partition);
            }
            let opened = match reader.open(Arc::clone(&self.store), &self.metrics).await {
                Ok(opened) => opened,
                Err(OpenError::NotFound(_)) => {
                    let _ = tx.send(Err(error(format!(
                        "Parquet directory '{}': '{}' disappeared after the directory was listed",
                        self.root, file.path
                    ))));
                    return;
                }
                Err(OpenError::Failed(failure)) => {
                    let _ = tx.send(Err(failure));
                    return;
                }
            };
            self.metrics.file_opened();
            let mut stream = match reader.stream(opened, self.metrics.clone()).await {
                Ok(stream) => stream,
                Err(failure) => {
                    let _ = tx.send(Err(failure));
                    return;
                }
            };
            loop {
                match stream.next_batch().await {
                    Ok(Some(batch)) => {
                        let batch =
                            RecordBatch::try_new(Arc::clone(&schema), batch.columns().to_vec())
                                .map_err(storage_error);
                        let failed = batch.is_err();
                        if tx.send(batch.map(Some)).is_err() || failed {
                            return;
                        }
                    }
                    Ok(None) => break,
                    Err(failure) => {
                        let _ = tx.send(Err(failure));
                        return;
                    }
                }
            }
        }
        let _ = tx.send(Ok(None));
    }

    /// The advertised schema and this partition's files, every file's schema
    /// checked against the first listed file's as it is opened.
    async fn prepare(&self) -> Result<(SchemaRef, Vec<(DirectoryFile, bool)>)> {
        let listing = self.listing().await?;
        let Some(first) = listing.files.first() else {
            return Err(error(format!(
                "location '{}' is neither an object nor a directory of Parquet files",
                self.root
            )));
        };
        let assignment = match self.partition {
            Some(partition) => assign_files(&listing.sizes(), partition),
            None => FileAssignment {
                whole: (0..listing.files.len()).collect(),
                split: Vec::new(),
            },
        };
        self.metrics.files_considered(assignment.len() as u64);
        let head = self
            .file_reader(first)
            .open(Arc::clone(&self.store), &self.metrics)
            .await
            .map_err(kaveon_core::KaveonError::from)?;
        let file_schema = Arc::clone(head.schema());
        let schema = advertised_schema(&file_schema, self.columns.as_deref())?;
        let root = self.root.clone();
        let first_path = first.path.clone();
        let store = Arc::clone(&self.store);
        let metrics = self.metrics.clone();
        let files = stream::iter(assignment.files().into_iter().map(|(index, split)| {
            let file = listing.files[index].clone();
            let reader = self.file_reader(&file);
            let store = Arc::clone(&store);
            let metrics = metrics.clone();
            let (root, first_path, file_schema) =
                (root.clone(), first_path.clone(), Arc::clone(&file_schema));
            async move {
                if file.path != first_path {
                    let opened = reader
                        .open(store, &metrics)
                        .await
                        .map_err(kaveon_core::KaveonError::from)?;
                    check_file_schema(
                        root.as_ref(),
                        first_path.as_ref(),
                        &file_schema,
                        file.path.as_ref(),
                        opened.schema(),
                    )?;
                }
                Ok::<_, kaveon_core::KaveonError>((file, split))
            }
        }))
        .buffered(METADATA_READ_CONCURRENCY)
        .try_collect::<Vec<_>>()
        .await?;
        Ok((schema, files))
    }
}

pub struct ObjectDirectorySource {
    schema: SchemaRef,
    receiver: mpsc::Receiver<Result<Option<RecordBatch>>>,
    metrics: ScanMetrics,
    exhausted: bool,
}

impl ObjectDirectorySource {
    pub fn metrics(&self) -> ScanMetrics {
        self.metrics.clone()
    }
}

impl BatchSource for ObjectDirectorySource {
    fn schema(&self) -> &SchemaRef {
        &self.schema
    }
    fn next_batch(&mut self) -> Result<Option<RecordBatch>> {
        if self.exhausted {
            return Ok(None);
        }
        let batch = self
            .receiver
            .recv()
            .map_err(|_| error("directory reader terminated without an end-of-stream marker"))??;
        self.exhausted = batch.is_none();
        Ok(batch)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use arrow::{
        array::{ArrayRef, Int64Array, StringArray},
        datatypes::{DataType, Field, Schema},
    };
    use object_store::{PutPayload, memory::InMemory};
    use parquet::{arrow::ArrowWriter, file::properties::WriterProperties};

    fn parquet_bytes(schema: &SchemaRef, values: Vec<i64>, row_group_size: usize) -> Vec<u8> {
        let labels = values
            .iter()
            .map(|value| format!("v{value}"))
            .collect::<Vec<_>>();
        let mut columns: Vec<ArrayRef> = vec![Arc::new(Int64Array::from(values))];
        if schema.fields().len() > 1 {
            columns.push(Arc::new(StringArray::from(labels)));
        }
        let batch = RecordBatch::try_new(Arc::clone(schema), columns).unwrap();
        let properties = WriterProperties::builder()
            .set_max_row_group_size(row_group_size)
            .build();
        let mut writer =
            ArrowWriter::try_new(Vec::new(), Arc::clone(schema), Some(properties)).unwrap();
        writer.write(&batch).unwrap();
        writer.into_inner().unwrap()
    }

    fn two_columns() -> SchemaRef {
        Arc::new(Schema::new(vec![
            Field::new("x", DataType::Int64, false),
            Field::new("label", DataType::Utf8, false),
        ]))
    }

    async fn put(store: &dyn ObjectStore, path: &str, bytes: Vec<u8>) {
        store
            .put(&Path::from(path), PutPayload::from(bytes))
            .await
            .unwrap();
    }

    /// A directory of three data files with hidden and marker objects around
    /// them: `b` is deliberately written first so the listing order is the
    /// path order, not the write order.
    async fn fixture(store: &dyn ObjectStore) {
        let schema = two_columns();
        put(
            store,
            "table/b.parquet",
            parquet_bytes(&schema, vec![3, 4], 1),
        )
        .await;
        put(
            store,
            "table/a.parquet",
            parquet_bytes(&schema, vec![1, 2], 1),
        )
        .await;
        // The partitioned file is far larger than the other two, so it is
        // the one a two-way partition splits by row group.
        put(
            store,
            "table/year=2026/c.PARQUET",
            parquet_bytes(&schema, (5..=104).collect(), 10),
        )
        .await;
        put(store, "table/_SUCCESS", Vec::new()).await;
        put(
            store,
            "table/_delta_log/00000000000000000000.json",
            b"{}".to_vec(),
        )
        .await;
        put(
            store,
            "table/.hidden.parquet",
            parquet_bytes(&schema, vec![99], 1),
        )
        .await;
        put(
            store,
            "table/_temporary/x.parquet",
            parquet_bytes(&schema, vec![98], 1),
        )
        .await;
        put(store, "table/empty-marker", Vec::new()).await;
        put(
            store,
            "table-2/other.parquet",
            parquet_bytes(&schema, vec![97], 1),
        )
        .await;
    }

    /// Every `x` of the source, sorted: the decoder lanes interleave row
    /// groups, so arrival order is not file order.
    fn values(source: &mut dyn BatchSource) -> Vec<i64> {
        let mut values = Vec::new();
        while let Some(batch) = source.next_batch().unwrap() {
            let column = batch
                .column(0)
                .as_any()
                .downcast_ref::<Int64Array>()
                .unwrap();
            values.extend(column.values().iter().copied());
        }
        assert!(source.next_batch().unwrap().is_none());
        values.sort_unstable();
        values
    }

    #[test]
    fn verdicts_follow_hive_visibility_and_the_extension_rule() {
        let verdict = |path: &str| file_verdict(path.split('/'));
        assert_eq!(verdict("part-0.parquet"), FileVerdict::Data);
        assert_eq!(verdict("part-0.snappy.PARQUET"), FileVerdict::Data);
        assert_eq!(verdict("20260917_000000_00000_abcde"), FileVerdict::Data);
        assert_eq!(verdict("year=2026/part-0.parquet"), FileVerdict::Data);
        assert_eq!(verdict("_SUCCESS"), FileVerdict::Hidden);
        assert_eq!(verdict("_delta_log/0.json"), FileVerdict::Hidden);
        assert_eq!(verdict(".part-0.parquet.crc"), FileVerdict::Hidden);
        assert_eq!(verdict("_temporary/0/part-0.parquet"), FileVerdict::Hidden);
        assert_eq!(verdict("README.md"), FileVerdict::Foreign);
        assert_eq!(verdict("data/part-0.orc"), FileVerdict::Foreign);
    }

    #[tokio::test]
    async fn listing_is_sorted_skips_hidden_and_stays_under_the_root() {
        let store = InMemory::new();
        fixture(&store).await;
        let listing = list_parquet_directory(&store, &Path::from("table"))
            .await
            .unwrap();
        assert_eq!(
            listing
                .files
                .iter()
                .map(|file| file.path.to_string())
                .collect::<Vec<_>>(),
            [
                "table/a.parquet",
                "table/b.parquet",
                "table/year=2026/c.PARQUET"
            ]
        );
        assert!(listing.files.iter().all(|file| file.size > 0));
        assert!(listing.files.iter().all(|file| file.identity().is_some()));
        assert_eq!(listing.identity_lines().lines().count(), 3);

        put(&store, "table/notes.txt", b"x".to_vec()).await;
        let failure = list_parquet_directory(&store, &Path::from("table"))
            .await
            .unwrap_err()
            .to_string();
        assert!(failure.contains("table/notes.txt"), "{failure}");
    }

    #[test]
    fn assignment_spreads_whole_files_and_splits_the_large_ones() {
        let partitions = |count: usize| {
            (0..count)
                .map(|index| ScanPartition::new(index, count).unwrap())
                .collect::<Vec<_>>()
        };
        // One partition reads everything whole.
        assert_eq!(
            assign_files(&[5, 5], ScanPartition::new(0, 1).unwrap()),
            FileAssignment {
                whole: vec![0, 1],
                split: vec![]
            }
        );
        // A single file splits by row group on every partition, as before.
        for partition in partitions(3) {
            assert_eq!(
                assign_files(&[100], partition),
                FileAssignment {
                    whole: vec![],
                    split: vec![0]
                }
            );
        }
        // Equal files, one per partition: whole, balanced, no split.
        let equal = partitions(3)
            .into_iter()
            .map(|partition| assign_files(&[100, 100, 100], partition))
            .collect::<Vec<_>>();
        assert!(equal.iter().all(|assignment| assignment.split.is_empty()));
        let mut owned = equal
            .iter()
            .flat_map(|assignment| assignment.whole.iter().copied())
            .collect::<Vec<_>>();
        owned.sort_unstable();
        assert_eq!(owned, [0, 1, 2]);
        // Four equal files on three partitions: one would carry twice the
        // fair share, so the largest is split and the rest stay whole.
        let four = partitions(3)
            .into_iter()
            .map(|partition| assign_files(&[100, 100, 100, 100], partition))
            .collect::<Vec<_>>();
        assert!(four.iter().all(|assignment| assignment.split == [0]));
        assert!(four.iter().all(|assignment| assignment.whole.len() == 1));
        // One large file with small companions: the large file splits, the
        // small ones spread whole.
        let skewed = partitions(3)
            .into_iter()
            .map(|partition| assign_files(&[10, 300, 10], partition))
            .collect::<Vec<_>>();
        assert!(skewed.iter().all(|assignment| assignment.split == [1]));
        let mut small = skewed
            .iter()
            .flat_map(|assignment| assignment.whole.iter().copied())
            .collect::<Vec<_>>();
        small.sort_unstable();
        assert_eq!(small, [0, 2]);
        // Every file is owned by exactly one partition or split by all.
        for sizes in [
            vec![7, 3, 9, 1, 4, 4, 12, 2],
            vec![1; 17],
            vec![1000, 1, 1, 1],
        ] {
            for count in 1..=5 {
                let assignments = partitions(count)
                    .into_iter()
                    .map(|partition| assign_files(&sizes, partition))
                    .collect::<Vec<_>>();
                for index in 0..sizes.len() {
                    let whole = assignments
                        .iter()
                        .filter(|assignment| assignment.whole.contains(&index))
                        .count();
                    let split = assignments
                        .iter()
                        .filter(|assignment| assignment.split.contains(&index))
                        .count();
                    assert!(
                        (whole == 1 && split == 0) || (whole == 0 && split == count),
                        "{sizes:?} over {count}: file {index} whole {whole} split {split}"
                    );
                }
            }
        }
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn directory_metadata_sums_files_and_checks_every_schema() {
        let store: Arc<dyn ObjectStore> = Arc::new(InMemory::new());
        fixture(store.as_ref()).await;
        let reader = ObjectDirectoryReader::new(
            Arc::clone(&store),
            "memory",
            format!("metadata-{}", uuid_like()),
            Path::from("table"),
        );
        let ParquetLocation::Directory(listing) = reader.probe().await.unwrap() else {
            panic!("a directory of Parquet files is a directory");
        };
        assert_eq!(listing.files.len(), 3);
        let metadata = reader.metadata().await.unwrap();
        assert_eq!(metadata.row_count, 104);
        assert_eq!(metadata.row_group_count, 14);
        assert_eq!(metadata.schema, two_columns());

        // A file with another schema is named, not cast.
        let narrow = Arc::new(Schema::new(vec![Field::new("x", DataType::Int64, false)]));
        put(
            store.as_ref(),
            "table/d.parquet",
            parquet_bytes(&narrow, vec![8], 1),
        )
        .await;
        let failure = reader.metadata().await.unwrap_err().to_string();
        assert!(failure.contains("table/d.parquet"), "{failure}");
        assert!(failure.contains("table/a.parquet"), "{failure}");

        // A single object probes as one; nothing at all is an error naming
        // the location.
        let ParquetLocation::Object(object) = ObjectDirectoryReader::new(
            Arc::clone(&store),
            "memory",
            "probe",
            Path::from("table/a.parquet"),
        )
        .probe()
        .await
        .unwrap() else {
            panic!("an object is an object");
        };
        assert_eq!(object.location, Path::from("table/a.parquet"));
        let missing = ObjectDirectoryReader::new(store, "memory", "probe", Path::from("absent"))
            .probe()
            .await
            .expect_err("nothing at the location is an error")
            .to_string();
        assert!(missing.contains("absent"), "{missing}");
    }

    fn uuid_like() -> String {
        format!(
            "{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        )
    }

    #[test]
    fn partitions_cover_the_directory_once_with_projection_and_pruning() {
        let store: Arc<dyn ObjectStore> = Arc::new(InMemory::new());
        let runtime = tokio::runtime::Runtime::new().unwrap();
        runtime.block_on(fixture(store.as_ref()));
        let container = format!("partitions-{}", uuid_like());
        let mut seen = Vec::new();
        let mut considered = 0;
        for index in 0..2 {
            let metrics = ScanMetrics::default();
            let mut source = ObjectDirectoryReader::new(
                Arc::clone(&store),
                "memory",
                &container,
                Path::from("table"),
            )
            .with_columns(vec!["label".into(), "x".into()])
            .with_partition(ScanPartition::new(index, 2).unwrap())
            .with_metrics(metrics.clone())
            .read_blocking()
            .unwrap();
            assert_eq!(source.schema().field(0).name(), "label");
            assert_eq!(source.schema().field(1).name(), "x");
            while let Some(batch) = source.next_batch().unwrap() {
                assert_eq!(batch.schema(), *source.schema());
                let column = batch
                    .column(1)
                    .as_any()
                    .downcast_ref::<Int64Array>()
                    .unwrap();
                seen.extend(column.values().iter().copied());
            }
            let snapshot = metrics.snapshot();
            assert_eq!(snapshot.files_opened, snapshot.files_considered);
            considered += snapshot.files_considered;
        }
        seen.sort_unstable();
        assert_eq!(seen, (1..=104).collect::<Vec<_>>());
        // The large file is split between the two partitions and the two
        // small ones spread whole: four file opens in total.
        assert_eq!(considered, 4);

        // Row-group pruning applies inside every file.
        let metrics = ScanMetrics::default();
        let mut pruned = ObjectDirectoryReader::new(
            Arc::clone(&store),
            "memory",
            &container,
            Path::from("table"),
        )
        .with_predicate(StoragePredicate::Compare {
            column: "x".into(),
            op: kaveon_core::CompareOp::Ge,
            value: kaveon_core::ScalarValue::Int64(100),
        })
        .with_metrics(metrics.clone())
        .read_blocking()
        .unwrap();
        assert_eq!(values(&mut pruned), [100, 101, 102, 103, 104]);
        let snapshot = metrics.snapshot();
        assert_eq!(snapshot.files_considered, 3);
        assert_eq!(snapshot.row_groups_considered, 14);
        assert_eq!(snapshot.row_groups_selected, 1);

        // A pinned listing is read as pinned: a file added afterwards is not
        // seen until the next query lists again.
        let pinned = runtime
            .block_on(list_parquet_directory(store.as_ref(), &Path::from("table")))
            .unwrap();
        runtime.block_on(put(
            store.as_ref(),
            "table/z.parquet",
            parquet_bytes(&two_columns(), vec![105], 1),
        ));
        let mut from_pin = ObjectDirectoryReader::new(
            Arc::clone(&store),
            "memory",
            &container,
            Path::from("table"),
        )
        .with_listing(Arc::new(pinned))
        .read_blocking()
        .unwrap();
        assert_eq!(values(&mut from_pin), (1..=104).collect::<Vec<_>>());
        let mut fresh =
            ObjectDirectoryReader::new(store, "memory", &container, Path::from("table"))
                .read_blocking()
                .unwrap();
        assert_eq!(values(&mut fresh), (1..=105).collect::<Vec<_>>());
    }

    #[test]
    fn a_schema_mismatch_names_the_file_when_read() {
        let store: Arc<dyn ObjectStore> = Arc::new(InMemory::new());
        let runtime = tokio::runtime::Runtime::new().unwrap();
        let schema = two_columns();
        runtime.block_on(put(
            store.as_ref(),
            "table/a.parquet",
            parquet_bytes(&schema, vec![1], 1),
        ));
        let retyped = Arc::new(Schema::new(vec![
            Field::new("x", DataType::Utf8, false),
            Field::new("label", DataType::Utf8, false),
        ]));
        let batch = RecordBatch::try_new(
            Arc::clone(&retyped),
            vec![
                Arc::new(StringArray::from(vec!["1"])) as ArrayRef,
                Arc::new(StringArray::from(vec!["v1"])) as ArrayRef,
            ],
        )
        .unwrap();
        let mut writer = ArrowWriter::try_new(Vec::new(), retyped, None).unwrap();
        writer.write(&batch).unwrap();
        runtime.block_on(put(
            store.as_ref(),
            "table/b.parquet",
            writer.into_inner().unwrap(),
        ));
        let failure = ObjectDirectoryReader::new(
            store,
            "memory",
            format!("mismatch-{}", uuid_like()),
            Path::from("table"),
        )
        .read_blocking()
        .err()
        .expect("a schema mismatch is an error")
        .to_string();
        assert!(failure.contains("table/b.parquet"), "{failure}");
        assert!(
            failure.contains("Utf8") && failure.contains("Int64"),
            "{failure}"
        );
    }

    #[test]
    fn the_single_object_reader_reads_a_directory_at_its_location() {
        let store: Arc<dyn ObjectStore> = Arc::new(InMemory::new());
        tokio::runtime::Runtime::new()
            .unwrap()
            .block_on(fixture(store.as_ref()));
        let container = format!("adls-directory-{}", uuid_like());
        let metrics = ScanMetrics::default();
        let mut source =
            AdlsParquetReader::over_store(Arc::clone(&store), "memory", &container, "table")
                .with_columns(vec!["x".into()])
                .with_metrics(metrics.clone())
                .read_blocking()
                .unwrap();
        assert_eq!(values(&mut source), (1..=104).collect::<Vec<_>>());
        assert_eq!(metrics.snapshot().files_opened, 3);
        let mut object =
            AdlsParquetReader::over_store(store, "memory", &container, "table/b.parquet")
                .read_blocking()
                .unwrap();
        assert_eq!(values(&mut object), [3, 4]);
    }
}
