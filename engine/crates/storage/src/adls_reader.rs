use std::{
    collections::HashMap,
    pin::Pin,
    sync::{
        Arc, Mutex, OnceLock,
        atomic::{AtomicUsize, Ordering},
        mpsc,
    },
    time::Instant,
};

use arrow::{datatypes::SchemaRef, record_batch::RecordBatch};
use bytes::Bytes;
use futures::future::{BoxFuture, FutureExt};
use futures::{Stream, StreamExt};
use kaveon_core::{BatchSource, KaveonError, Result, StoragePredicate};
use object_store::{GetOptions, ObjectStore, azure::MicrosoftAzureBuilder, path::Path};
use parquet::arrow::{
    ParquetRecordBatchStreamBuilder, ProjectionMask,
    arrow_reader::ArrowReaderMetadata,
    async_reader::{AsyncFileReader, ParquetObjectReader, ParquetRecordBatchStream},
};
use parquet::{errors::ParquetError, file::metadata::ParquetMetaData};

use crate::{
    ScanMetrics, ScanPartition,
    parquet_reader::{
        matching_row_groups, parquet_row_filter, projection_indices, record_selection_metrics,
        validate_predicate,
    },
};

const DEFAULT_BATCH_SIZE: usize = 8_192;
// Once a bounded immutable object is resident, small decoder batches add
// channel, filter, and aggregate dispatch overhead without providing I/O
// backpressure. Keep caller-selected sizes exact, but coalesce the default
// for this in-memory path.
const PRELOADED_BATCH_SIZE: usize = 65_536;
const MAX_METADATA_CACHE_ENTRIES: usize = 256;
const MAX_OBJECT_STORE_CACHE_ENTRIES: usize = 32;
const FULL_OBJECT_CACHE_LIMIT: usize = 64 * 1024 * 1024;
const FULL_OBJECT_CACHE_MIN_ROW_GROUPS: usize = 32;
const PROCESS_FULL_OBJECT_CACHE_LIMIT: usize = 256 * 1024 * 1024;
const PROCESS_DECODED_BATCH_CACHE_LIMIT: usize = 256 * 1024 * 1024;
static FULL_OBJECT_CACHE_BYTES: AtomicUsize = AtomicUsize::new(0);
static FULL_OBJECT_CACHE: OnceLock<Mutex<HashMap<String, Arc<FullObjectEntry>>>> = OnceLock::new();
static DECODED_BATCH_CACHE_BYTES: AtomicUsize = AtomicUsize::new(0);
static DECODED_BATCH_CACHE: OnceLock<Mutex<HashMap<String, Arc<DecodedBatchEntry>>>> =
    OnceLock::new();
static OBJECT_STORE_CACHE: OnceLock<Mutex<HashMap<String, Arc<dyn ObjectStore>>>> = OnceLock::new();
static METADATA_LOAD_LOCKS: OnceLock<Mutex<HashMap<String, Arc<tokio::sync::Mutex<()>>>>> =
    OnceLock::new();
static METADATA_OVERFLOW_LOCK: OnceLock<Arc<tokio::sync::Mutex<()>>> = OnceLock::new();

fn metadata_load_lock(key: &str) -> Arc<tokio::sync::Mutex<()>> {
    let mut locks = METADATA_LOAD_LOCKS
        .get_or_init(Default::default)
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    if locks.len() >= MAX_METADATA_CACHE_ENTRIES && !locks.contains_key(key) {
        locks.retain(|_, lock| Arc::strong_count(lock) > 1);
        if locks.len() >= MAX_METADATA_CACHE_ENTRIES {
            return METADATA_OVERFLOW_LOCK
                .get_or_init(|| Arc::new(tokio::sync::Mutex::new(())))
                .clone();
        }
    }
    locks
        .entry(key.to_owned())
        .or_insert_with(|| Arc::new(tokio::sync::Mutex::new(())))
        .clone()
}

fn cached_object_store(key: &str) -> Option<Arc<dyn ObjectStore>> {
    OBJECT_STORE_CACHE
        .get_or_init(Default::default)
        .lock()
        .ok()?
        .get(key)
        .cloned()
}

fn cache_object_store(key: String, store: Arc<dyn ObjectStore>) {
    let Ok(mut cache) = OBJECT_STORE_CACHE.get_or_init(Default::default).lock() else {
        return;
    };
    if cache.len() >= MAX_OBJECT_STORE_CACHE_ENTRIES && !cache.contains_key(&key) {
        cache.clear();
    }
    cache.insert(key, store);
}

struct DecodedBatchEntry {
    batches: Arc<Vec<RecordBatch>>,
    _reservation: DecodedCacheReservation,
}

struct DecodedCacheReservation(usize);

impl Drop for DecodedCacheReservation {
    fn drop(&mut self) {
        DECODED_BATCH_CACHE_BYTES.fetch_sub(self.0, Ordering::AcqRel);
    }
}

fn cached_decoded_batches(key: &str) -> Option<Arc<Vec<RecordBatch>>> {
    DECODED_BATCH_CACHE
        .get_or_init(Default::default)
        .lock()
        .ok()?
        .get(key)
        .map(|entry| Arc::clone(&entry.batches))
}

fn cache_decoded_batches(key: String, batches: Vec<RecordBatch>) {
    if batches.is_empty() {
        return;
    }
    let bytes = batches
        .iter()
        .map(RecordBatch::get_array_memory_size)
        .sum::<usize>();
    if bytes == 0 || bytes > PROCESS_DECODED_BATCH_CACHE_LIMIT {
        return;
    }
    let Ok(mut cache) = DECODED_BATCH_CACHE.get_or_init(Default::default).lock() else {
        return;
    };
    if cache.contains_key(&key) {
        return;
    }
    let reserved =
        DECODED_BATCH_CACHE_BYTES.fetch_update(Ordering::AcqRel, Ordering::Acquire, |current| {
            current
                .checked_add(bytes)
                .filter(|&next| next <= PROCESS_DECODED_BATCH_CACHE_LIMIT)
        });
    if reserved.is_err() {
        cache.clear();
        if DECODED_BATCH_CACHE_BYTES
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |current| {
                current
                    .checked_add(bytes)
                    .filter(|&next| next <= PROCESS_DECODED_BATCH_CACHE_LIMIT)
            })
            .is_err()
        {
            return;
        }
    }
    cache.insert(
        key,
        Arc::new(DecodedBatchEntry {
            batches: Arc::new(batches),
            _reservation: DecodedCacheReservation(bytes),
        }),
    );
}

struct CacheReservation(usize);

impl CacheReservation {
    fn try_new(bytes: usize) -> Option<Self> {
        FULL_OBJECT_CACHE_BYTES
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |current| {
                current
                    .checked_add(bytes)
                    .filter(|&next| next <= PROCESS_FULL_OBJECT_CACHE_LIMIT)
            })
            .ok()
            .map(|_| Self(bytes))
    }
}

struct FullObjectEntry {
    bytes: tokio::sync::OnceCell<Bytes>,
    fetches: AtomicUsize,
    _reservation: CacheReservation,
}

fn full_object_entry(key: String, size: usize) -> Option<Arc<FullObjectEntry>> {
    let mut cache = FULL_OBJECT_CACHE
        .get_or_init(Default::default)
        .lock()
        .ok()?;
    if let Some(entry) = cache.get(&key) {
        return Some(entry.clone());
    }
    let reservation = match CacheReservation::try_new(size) {
        Some(reservation) => reservation,
        None => {
            cache.clear();
            CacheReservation::try_new(size)?
        }
    };
    let entry = Arc::new(FullObjectEntry {
        bytes: tokio::sync::OnceCell::new(),
        fetches: AtomicUsize::new(0),
        _reservation: reservation,
    });
    cache.insert(key, entry.clone());
    Some(entry)
}

impl Drop for CacheReservation {
    fn drop(&mut self) {
        FULL_OBJECT_CACHE_BYTES.fetch_sub(self.0, Ordering::AcqRel);
    }
}

/// Parquet's async decoder fetches projected column ranges one row group at a
/// time. For small files with many row groups this turns a single scan into
/// hundreds of sequential cloud requests. Preloading the immutable object once
/// bounds that amplification while retaining range reads for larger files.
struct AdlsObjectReader {
    inner: ParquetObjectReader,
    store: Arc<dyn ObjectStore>,
    path: Path,
    size: usize,
    e_tag: Option<String>,
    version: Option<String>,
    shared: Option<Arc<FullObjectEntry>>,
}

impl AdlsObjectReader {
    fn new(
        store: Arc<dyn ObjectStore>,
        metadata: object_store::ObjectMeta,
        cache_key: Option<String>,
    ) -> Self {
        let shared = cache_key.and_then(|key| full_object_entry(key, metadata.size));
        Self {
            inner: ParquetObjectReader::new(store.clone(), metadata.clone()),
            store,
            path: metadata.location,
            size: metadata.size,
            e_tag: metadata.e_tag,
            version: metadata.version,
            shared,
        }
    }

    async fn cached_bytes(&self) -> parquet::errors::Result<Bytes> {
        let entry = self.shared.as_ref().expect("cache entry is present");
        let store = self.store.clone();
        let path = self.path.clone();
        let size = self.size;
        let e_tag = self.e_tag.clone();
        let version = self.version.clone();
        entry
            .bytes
            .get_or_try_init(|| async move {
                entry.fetches.fetch_add(1, Ordering::AcqRel);
                store
                    .get_opts(
                        &path,
                        GetOptions {
                            if_match: e_tag,
                            version,
                            range: Some((0..size).into()),
                            ..Default::default()
                        },
                    )
                    .await
                    .map_err(|error| ParquetError::External(Box::new(error)))?
                    .bytes()
                    .await
                    .map_err(|error| ParquetError::External(Box::new(error)))
            })
            .await
            .cloned()
    }
}

impl AsyncFileReader for AdlsObjectReader {
    fn get_bytes(
        &mut self,
        range: std::ops::Range<usize>,
    ) -> BoxFuture<'_, parquet::errors::Result<Bytes>> {
        if self.shared.is_none() {
            let store = self.store.clone();
            let path = self.path.clone();
            let e_tag = self.e_tag.clone();
            let version = self.version.clone();
            return async move {
                store
                    .get_opts(
                        &path,
                        GetOptions {
                            if_match: e_tag,
                            version,
                            range: Some(range.into()),
                            ..Default::default()
                        },
                    )
                    .await
                    .map_err(|error| ParquetError::External(Box::new(error)))?
                    .bytes()
                    .await
                    .map_err(|error| ParquetError::External(Box::new(error)))
            }
            .boxed();
        }
        async move {
            let cached = self.cached_bytes().await?;
            if cached.get(range.clone()).is_none() {
                return Err(ParquetError::General(
                    "Parquet range exceeds object size".into(),
                ));
            }
            Ok(cached.slice(range))
        }
        .boxed()
    }

    fn get_byte_ranges(
        &mut self,
        ranges: Vec<std::ops::Range<usize>>,
    ) -> BoxFuture<'_, parquet::errors::Result<Vec<Bytes>>> {
        if self.shared.is_none() {
            let store = self.store.clone();
            let path = self.path.clone();
            let e_tag = self.e_tag.clone();
            let version = self.version.clone();
            return async move {
                futures::future::try_join_all(ranges.into_iter().map(|range| {
                    let store = store.clone();
                    let path = path.clone();
                    let e_tag = e_tag.clone();
                    let version = version.clone();
                    async move {
                        store
                            .get_opts(
                                &path,
                                GetOptions {
                                    if_match: e_tag,
                                    version,
                                    range: Some(range.into()),
                                    ..Default::default()
                                },
                            )
                            .await
                            .map_err(|error| ParquetError::External(Box::new(error)))?
                            .bytes()
                            .await
                            .map_err(|error| ParquetError::External(Box::new(error)))
                    }
                }))
                .await
            }
            .boxed();
        }
        async move {
            let cached = self.cached_bytes().await?;
            ranges
                .into_iter()
                .map(|range| {
                    if cached.get(range.clone()).is_none() {
                        return Err(ParquetError::General(
                            "Parquet range exceeds object size".into(),
                        ));
                    }
                    Ok(cached.slice(range))
                })
                .collect()
        }
        .boxed()
    }

    fn get_metadata(&mut self) -> BoxFuture<'_, parquet::errors::Result<Arc<ParquetMetaData>>> {
        self.inner.get_metadata()
    }
}

static METADATA_CACHE: OnceLock<Mutex<HashMap<String, ArrowReaderMetadata>>> = OnceLock::new();

fn object_identity(metadata: &object_store::ObjectMeta) -> Result<String> {
    if metadata.e_tag.is_none() && metadata.version.is_none() {
        return Err(storage_error(
            "ADLS object has no ETag or version for an immutable Parquet read",
        ));
    }
    Ok(format!(
        "{}:{}:{}:{}",
        metadata.size,
        metadata
            .last_modified
            .timestamp_nanos_opt()
            .unwrap_or_default(),
        metadata.e_tag.as_deref().unwrap_or_default(),
        metadata.version.as_deref().unwrap_or_default()
    ))
}

fn cached_metadata(key: &str, identity: &str) -> Option<ArrowReaderMetadata> {
    METADATA_CACHE
        .get_or_init(Default::default)
        .lock()
        .ok()?
        .get(&format!("{key}:{identity}"))
        .cloned()
}

fn cache_metadata(key: String, identity: String, metadata: ArrowReaderMetadata) {
    let Ok(mut cache) = METADATA_CACHE.get_or_init(Default::default).lock() else {
        return;
    };
    let identity_key = format!("{key}:{identity}");
    if cache.len() >= MAX_METADATA_CACHE_ENTRIES && !cache.contains_key(&identity_key) {
        cache.clear();
    }
    cache.insert(identity_key, metadata);
}

async fn load_parquet_metadata(
    store: Arc<dyn ObjectStore>,
    path: &Path,
    cache_key: &str,
    metrics: &ScanMetrics,
) -> Result<(object_store::ObjectMeta, String, ArrowReaderMetadata)> {
    // Revalidate the mutable path on every logical open. The footer cache is
    // keyed by the immutable identity returned by this HEAD, so replacing an
    // object can never reuse the former object's footer.
    let object_metadata = store.head(path).await.map_err(object_store_error)?;
    let identity = object_identity(&object_metadata)?;
    if let Some(metadata) = cached_metadata(cache_key, &identity) {
        metrics.object_metadata_cache_hit();
        return Ok((object_metadata, identity, metadata));
    }
    let load_lock = metadata_load_lock(&format!("{cache_key}:{identity}"));
    let _load_guard = load_lock.lock().await;
    let metadata = match cached_metadata(cache_key, &identity) {
        Some(metadata) => {
            metrics.object_metadata_cache_hit();
            metadata
        }
        None => {
            let mut object_reader = AdlsObjectReader::new(store, object_metadata.clone(), None);
            let metadata = ArrowReaderMetadata::load_async(&mut object_reader, Default::default())
                .await
                .map_err(parquet_error)?;
            cache_metadata(cache_key.to_owned(), identity.clone(), metadata.clone());
            metadata
        }
    };
    Ok((object_metadata, identity, metadata))
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum AdlsAuthMode {
    #[default]
    Environment,
    AzureCli,
}

pub struct AdlsBatchStream {
    schema: SchemaRef,
    inner: Pin<Box<dyn Stream<Item = parquet::errors::Result<RecordBatch>> + Send>>,
    metrics: ScanMetrics,
    output_projection: Option<Vec<usize>>,
    decoded_cache_key: Option<String>,
    decoded_batches: Vec<RecordBatch>,
}

impl AdlsBatchStream {
    #[must_use]
    pub fn schema(&self) -> &SchemaRef {
        &self.schema
    }

    #[must_use]
    pub fn metrics(&self) -> ScanMetrics {
        self.metrics.clone()
    }

    pub async fn next_batch(&mut self) -> Result<Option<RecordBatch>> {
        let started = Instant::now();
        let result = match self.inner.next().await.transpose() {
            Ok(result) => result,
            Err(error) => return Err(parquet_error(error)),
        };
        self.metrics.read_time(started.elapsed());
        if let Some(batch) = &result {
            self.metrics.emitted(batch.num_rows());
            if self.decoded_cache_key.is_some() {
                self.decoded_batches.push(batch.clone());
            }
        } else if let Some(key) = self.decoded_cache_key.take() {
            cache_decoded_batches(key, std::mem::take(&mut self.decoded_batches));
        }
        result
            .map(|batch| match &self.output_projection {
                Some(indices) => batch
                    .project(indices)
                    .map_err(|e| storage_error(e.to_string())),
                None => Ok(batch),
            })
            .transpose()
    }
}

pub struct AdlsBatchSource {
    schema: SchemaRef,
    receiver: mpsc::Receiver<Result<Option<RecordBatch>>>,
    metrics: ScanMetrics,
    exhausted: bool,
}

impl AdlsBatchSource {
    #[must_use]
    pub fn metrics(&self) -> ScanMetrics {
        self.metrics.clone()
    }
}

impl BatchSource for AdlsBatchSource {
    fn schema(&self) -> &SchemaRef {
        &self.schema
    }

    fn next_batch(&mut self) -> Result<Option<RecordBatch>> {
        if self.exhausted {
            return Ok(None);
        }
        let batch = self.receiver.recv().map_err(|_| {
            storage_error("ADLS reader terminated without an end-of-stream marker")
        })??;
        self.exhausted = batch.is_none();
        Ok(batch)
    }
}

#[derive(Clone)]
pub struct AdlsParquetReader {
    account: String,
    container: String,
    object_path: String,
    auth_mode: AdlsAuthMode,
    batch_size: usize,
    columns: Option<Vec<String>>,
    predicate: Option<StoragePredicate>,
    partition: Option<ScanPartition>,
    metrics: Option<ScanMetrics>,
}

impl AdlsParquetReader {
    pub fn new(
        account: impl Into<String>,
        container: impl Into<String>,
        object_path: impl Into<String>,
    ) -> Self {
        Self {
            account: account.into(),
            container: container.into(),
            object_path: object_path.into(),
            auth_mode: AdlsAuthMode::Environment,
            batch_size: DEFAULT_BATCH_SIZE,
            columns: None,
            predicate: None,
            partition: None,
            metrics: None,
        }
    }

    pub fn from_abfss_uri(uri: &str) -> Result<Self> {
        let remainder = uri
            .strip_prefix("abfss://")
            .ok_or_else(|| storage_error("ADLS URI must begin with abfss://"))?;
        let (authority, object_path) = remainder
            .split_once('/')
            .ok_or_else(|| storage_error("ADLS URI must include an object path"))?;
        let (container, host) = authority
            .split_once('@')
            .ok_or_else(|| storage_error("ADLS URI must use container@account authority"))?;
        let account = host
            .strip_suffix(".dfs.core.windows.net")
            .ok_or_else(|| storage_error("ADLS URI must use dfs.core.windows.net"))?;
        let reader = Self::new(account, container, object_path.trim_start_matches('/'));
        reader.validate()?;
        Ok(reader)
    }

    pub fn with_auth_mode(mut self, auth_mode: AdlsAuthMode) -> Self {
        self.auth_mode = auth_mode;
        self
    }

    pub fn with_batch_size(mut self, batch_size: usize) -> Self {
        self.batch_size = batch_size;
        self
    }

    pub fn with_columns(mut self, columns: Vec<String>) -> Self {
        self.columns = Some(columns);
        self
    }

    pub fn with_predicate(mut self, predicate: StoragePredicate) -> Self {
        self.predicate = Some(match self.predicate.take() {
            Some(existing) => StoragePredicate::And(vec![existing, predicate]),
            None => predicate,
        });
        self
    }

    pub fn with_partition(mut self, partition: ScanPartition) -> Self {
        self.partition = Some(partition);
        self
    }

    pub fn with_metrics(mut self, metrics: ScanMetrics) -> Self {
        self.metrics = Some(metrics);
        self
    }

    pub async fn read(&self) -> Result<AdlsBatchStream> {
        self.validate()?;
        let metrics = self.metrics.clone().unwrap_or_default();
        metrics.files_considered(1);
        let store_key = format!("{}/{}/{:?}", self.account, self.container, self.auth_mode);
        let store: Arc<dyn ObjectStore> = match cached_object_store(&store_key) {
            Some(store) => {
                metrics.object_store_cache_hit();
                store
            }
            None => {
                let store: Arc<dyn ObjectStore> = Arc::new(
                    MicrosoftAzureBuilder::from_env()
                        .with_account(&self.account)
                        .with_container_name(&self.container)
                        .with_use_azure_cli(self.auth_mode == AdlsAuthMode::AzureCli)
                        .build()
                        .map_err(object_store_error)?,
                );
                cache_object_store(store_key, store.clone());
                store
            }
        };
        let path =
            Path::parse(&self.object_path).map_err(|error| storage_error(error.to_string()))?;
        let footer_started = Instant::now();
        let cache_key = format!("{}/{}/{}", self.account, self.container, self.object_path);
        let (object_metadata, identity, metadata) =
            load_parquet_metadata(store.clone(), &path, &cache_key, &metrics).await?;
        let preload =
            should_preload_object(object_metadata.size, metadata.metadata().num_row_groups());
        let object_reader = AdlsObjectReader::new(
            store,
            object_metadata,
            preload.then(|| format!("{cache_key}:{identity}")),
        );
        let batch_size = effective_batch_size(self.batch_size, preload);
        let mut builder =
            ParquetRecordBatchStreamBuilder::new_with_metadata(object_reader, metadata)
                .with_batch_size(batch_size);
        metrics.footer_time(footer_started.elapsed());
        metrics.file_opened();

        let schema = Arc::clone(builder.schema());
        let projection = self
            .columns
            .as_ref()
            .map(|columns| projection_indices(&schema, columns))
            .transpose()?;
        if let Some(projection) = &projection {
            let mask = ProjectionMask::roots(builder.parquet_schema(), projection.clone());
            builder = builder.with_projection(mask);
        }

        if let Some(predicate) = self
            .predicate
            .as_ref()
            .and_then(|predicate| parquet_row_filter(builder.parquet_schema(), &schema, predicate))
        {
            builder = builder.with_row_filter(predicate);
        }

        let mut row_groups = if let Some(predicate) = &self.predicate {
            validate_predicate(predicate, &schema)?;
            matching_row_groups(builder.metadata().as_ref(), &schema, predicate)
        } else {
            (0..builder.metadata().num_row_groups()).collect()
        };
        if let Some(partition) = self.partition {
            row_groups.retain(|ordinal| partition.contains(*ordinal));
        }
        record_selection_metrics(
            builder.metadata().as_ref(),
            &row_groups,
            projection.as_deref(),
            &metrics,
        );
        let decoded_cache_key = preload.then(|| {
            format!(
                "{cache_key}:{identity}:batch={}:projection={projection:?}:predicate={:?}:row_groups={row_groups:?}",
                batch_size, self.predicate
            )
        });
        builder = builder.with_row_groups(row_groups);
        let stream: ParquetRecordBatchStream<AdlsObjectReader> =
            builder.build().map_err(parquet_error)?;
        let (schema, output_projection) = crate::parquet_reader::ordered_projection(
            Arc::clone(stream.schema()),
            self.columns.as_deref(),
        )?;
        let cached = decoded_cache_key
            .as_deref()
            .and_then(cached_decoded_batches);
        let cache_hit = cached.is_some();
        let inner: Pin<Box<dyn Stream<Item = parquet::errors::Result<RecordBatch>> + Send>> =
            match cached {
                Some(batches) => Box::pin(futures::stream::iter(
                    batches.iter().cloned().map(Ok).collect::<Vec<_>>(),
                )),
                None => Box::pin(stream),
            };
        Ok(AdlsBatchStream {
            schema,
            inner,
            metrics,
            output_projection,
            decoded_cache_key: (!cache_hit).then_some(decoded_cache_key).flatten(),
            decoded_batches: Vec::new(),
        })
    }

    pub fn read_blocking(self) -> Result<AdlsBatchSource> {
        self.validate()?;
        let (initial_sender, initial_receiver) = mpsc::sync_channel(1);
        let (batch_sender, batch_receiver) = mpsc::sync_channel(2);
        std::thread::Builder::new()
            .name("kaveon-adls-reader".into())
            .spawn(move || {
                let runtime = match tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                {
                    Ok(runtime) => runtime,
                    Err(error) => {
                        let _ = initial_sender.send(Err(storage_error(format!(
                            "failed to start ADLS runtime: {error}"
                        ))));
                        return;
                    }
                };
                runtime.block_on(async move {
                    let mut stream = match self.read().await {
                        Ok(stream) => stream,
                        Err(error) => {
                            let _ = initial_sender.send(Err(error));
                            return;
                        }
                    };
                    let schema = Arc::clone(stream.schema());
                    let metrics = stream.metrics();
                    if initial_sender.send(Ok((schema, metrics))).is_err() {
                        return;
                    }
                    loop {
                        match stream.next_batch().await {
                            Ok(Some(batch)) => {
                                if batch_sender.send(Ok(Some(batch))).is_err() {
                                    return;
                                }
                            }
                            Ok(None) => {
                                let _ = batch_sender.send(Ok(None));
                                return;
                            }
                            Err(error) => {
                                let _ = batch_sender.send(Err(error));
                                return;
                            }
                        }
                    }
                });
            })
            .map_err(|error| storage_error(format!("failed to spawn ADLS reader: {error}")))?;
        let (schema, metrics) = initial_receiver
            .recv()
            .map_err(|_| storage_error("ADLS reader stopped before initialization"))??;
        Ok(AdlsBatchSource {
            schema,
            receiver: batch_receiver,
            metrics,
            exhausted: false,
        })
    }

    fn validate(&self) -> Result<()> {
        for (name, value) in [
            ("ADLS account", self.account.as_str()),
            ("ADLS container", self.container.as_str()),
            ("ADLS object path", self.object_path.as_str()),
        ] {
            if value.trim().is_empty() {
                return Err(storage_error(format!("{name} cannot be empty")));
            }
        }
        if self.object_path.starts_with('/') || self.object_path.contains("..") {
            return Err(storage_error(
                "ADLS object path must be container-relative and cannot contain '..'",
            ));
        }
        if self.batch_size == 0 {
            return Err(storage_error("batch size must be greater than zero"));
        }
        Ok(())
    }
}

fn should_preload_object(size: usize, row_groups: usize) -> bool {
    size <= FULL_OBJECT_CACHE_LIMIT && row_groups >= FULL_OBJECT_CACHE_MIN_ROW_GROUPS
}

fn effective_batch_size(configured: usize, preloaded: bool) -> usize {
    if preloaded && configured == DEFAULT_BATCH_SIZE {
        PRELOADED_BATCH_SIZE
    } else {
        configured
    }
}

fn storage_error(message: impl Into<String>) -> KaveonError {
    KaveonError::Storage(message.into())
}

fn parquet_error(error: parquet::errors::ParquetError) -> KaveonError {
    storage_error(error.to_string())
}

fn object_store_error(error: object_store::Error) -> KaveonError {
    storage_error(error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use arrow::{
        array::Int64Array,
        datatypes::{DataType, Field, Schema},
    };
    use object_store::{ObjectStore, PutPayload, memory::InMemory};
    use parquet::arrow::ArrowWriter;

    fn parquet_bytes(rows: usize) -> Bytes {
        let schema = Arc::new(Schema::new(vec![Field::new(
            "value",
            DataType::Int64,
            false,
        )]));
        let batch = RecordBatch::try_new(
            schema.clone(),
            vec![Arc::new(Int64Array::from_iter_values(
                (0..rows).map(|value| value as i64),
            ))],
        )
        .unwrap();
        let mut writer = ArrowWriter::try_new(Vec::new(), schema, None).unwrap();
        writer.write(&batch).unwrap();
        writer.into_inner().unwrap().into()
    }

    #[tokio::test]
    async fn parquet_footer_cache_rechecks_identity_and_invalidates_replacement() {
        let store: Arc<dyn ObjectStore> = Arc::new(InMemory::new());
        let path = Path::from(format!("identity-cache-{}.parquet", std::process::id()));
        let key = format!("footer-cache-{}", std::process::id());
        store.put(&path, parquet_bytes(3).into()).await.unwrap();
        let metrics = ScanMetrics::default();
        let (first_object, first_identity, first_footer) =
            load_parquet_metadata(store.clone(), &path, &key, &metrics)
                .await
                .unwrap();
        assert_eq!(first_footer.metadata().file_metadata().num_rows(), 3);

        let (_, repeated_identity, repeated_footer) =
            load_parquet_metadata(store.clone(), &path, &key, &metrics)
                .await
                .unwrap();
        assert_eq!(repeated_identity, first_identity);
        assert_eq!(repeated_footer.metadata().file_metadata().num_rows(), 3);
        assert_eq!(metrics.snapshot().object_metadata_cache_hits, 1);

        store.put(&path, parquet_bytes(7).into()).await.unwrap();
        let (_, replacement_identity, replacement_footer) =
            load_parquet_metadata(store.clone(), &path, &key, &metrics)
                .await
                .unwrap();
        assert_ne!(replacement_identity, first_identity);
        assert_eq!(replacement_footer.metadata().file_metadata().num_rows(), 7);

        let mut pinned = AdlsObjectReader::new(store, first_object, None);
        assert!(pinned.get_bytes(0..1).await.is_err());
    }

    #[tokio::test]
    async fn footer_cache_refuses_an_unversioned_object_identity() {
        let store = InMemory::new();
        let path = Path::from("unversioned.parquet");
        store
            .put(&path, PutPayload::from_static(b"bytes"))
            .await
            .unwrap();
        let mut metadata = store.head(&path).await.unwrap();
        metadata.e_tag = None;
        metadata.version = None;
        assert!(object_identity(&metadata).is_err());
    }

    #[test]
    fn object_store_clients_are_reused_by_exact_connection_key() {
        let key = format!("store-cache-test-{}", std::process::id());
        let store: Arc<dyn ObjectStore> = Arc::new(InMemory::new());
        cache_object_store(key.clone(), store.clone());

        let cached = cached_object_store(&key).expect("exact store key must resolve");
        assert!(Arc::ptr_eq(&cached, &store));
        assert!(cached_object_store(&format!("{key}-other")).is_none());

        let metrics = ScanMetrics::default();
        metrics.object_store_cache_hit();
        assert_eq!(metrics.snapshot().object_store_cache_hits, 1);
    }

    #[test]
    fn cold_metadata_loads_share_an_exact_object_lock() {
        let key = format!("metadata-load-lock-{}", std::process::id());
        let first = metadata_load_lock(&key);
        let competing = metadata_load_lock(&key);
        let other = metadata_load_lock(&format!("{key}-other"));

        assert!(Arc::ptr_eq(&first, &competing));
        assert!(!Arc::ptr_eq(&first, &other));
    }

    #[test]
    fn decoded_batches_are_reused_by_exact_snapshot_key() {
        let key = format!("decoded-cache-test-{}", std::process::id());
        let batch = RecordBatch::try_from_iter(vec![(
            "value",
            Arc::new(Int64Array::from(vec![1, 2, 3])) as _,
        )])
        .unwrap();

        cache_decoded_batches(key.clone(), vec![batch]);

        let cached = cached_decoded_batches(&key).expect("exact cache key must resolve");
        assert_eq!(cached.len(), 1);
        assert_eq!(cached[0].num_rows(), 3);
        assert!(cached_decoded_batches(&format!("{key}:different-etag")).is_none());
    }

    #[test]
    fn validates_identity_path_and_batch_size_without_network_access() {
        assert!(
            AdlsParquetReader::new("", "data", "table/file.parquet")
                .validate()
                .is_err()
        );
        assert!(
            AdlsParquetReader::new("account", "", "table/file.parquet")
                .validate()
                .is_err()
        );
        assert!(
            AdlsParquetReader::new("account", "data", "")
                .validate()
                .is_err()
        );
        assert!(
            AdlsParquetReader::new("account", "data", "/absolute.parquet")
                .validate()
                .is_err()
        );
        assert!(
            AdlsParquetReader::new("account", "data", "../escape.parquet")
                .validate()
                .is_err()
        );
        assert!(
            AdlsParquetReader::new("account", "data", "table/file.parquet")
                .with_batch_size(0)
                .validate()
                .is_err()
        );
        assert!(
            AdlsParquetReader::new("account", "data", "table/file.parquet")
                .validate()
                .is_ok()
        );
    }

    #[test]
    fn parses_canonical_abfss_uri() {
        let reader = AdlsParquetReader::from_abfss_uri(
            "abfss://lake@account.dfs.core.windows.net/gold/orders/part.parquet",
        )
        .unwrap();
        assert_eq!(reader.account, "account");
        assert_eq!(reader.container, "lake");
        assert_eq!(reader.object_path, "gold/orders/part.parquet");
        assert!(AdlsParquetReader::from_abfss_uri("https://example.com/file.parquet").is_err());
        assert!(
            AdlsParquetReader::from_abfss_uri(
                "abfss://lake@account.blob.core.windows.net/file.parquet"
            )
            .is_err()
        );
    }

    #[tokio::test]
    async fn many_ranges_share_one_bounded_full_object_fetch() {
        let store: Arc<dyn ObjectStore> = Arc::new(InMemory::new());
        let path = Path::from("many-groups.parquet");
        store
            .put(&path, PutPayload::from_static(b"0123456789abcdef"))
            .await
            .unwrap();
        let metadata = store.head(&path).await.unwrap();
        let before = FULL_OBJECT_CACHE_BYTES.load(Ordering::Acquire);
        let key = "test-many-ranges-shared-cache".to_owned();
        let mut reader = AdlsObjectReader::new(store.clone(), metadata.clone(), Some(key.clone()));
        let mut concurrent = AdlsObjectReader::new(store, metadata, Some(key));
        let (first, competing) = tokio::join!(
            reader.get_byte_ranges(vec![0..2, 4..8, 12..16]),
            concurrent.get_bytes(2..4)
        );
        assert_eq!(first.unwrap()[0], Bytes::from_static(b"01"));
        assert_eq!(competing.unwrap(), Bytes::from_static(b"23"));
        // The failed 306-row-group AKS file assigned about 102 sequential
        // decoder fetch cycles to each of three workers.
        for _ in 1..102 {
            assert_eq!(
                reader
                    .get_byte_ranges(vec![0..2, 4..8, 12..16])
                    .await
                    .unwrap(),
                vec![
                    Bytes::from_static(b"01"),
                    Bytes::from_static(b"4567"),
                    Bytes::from_static(b"cdef")
                ]
            );
        }
        assert_eq!(
            reader.get_bytes(8..12).await.unwrap(),
            Bytes::from_static(b"89ab")
        );
        assert_eq!(
            reader
                .shared
                .as_ref()
                .unwrap()
                .fetches
                .load(Ordering::Acquire),
            1
        );
        assert_eq!(FULL_OBJECT_CACHE_BYTES.load(Ordering::Acquire), before + 16);
        drop(reader);
        drop(concurrent);
    }

    #[test]
    fn preload_is_limited_to_small_many_group_objects() {
        assert!(should_preload_object(60 * 1024 * 1024, 306));
        assert!(!should_preload_object(65 * 1024 * 1024, 306));
        assert!(!should_preload_object(60 * 1024 * 1024, 31));
    }

    #[test]
    fn preloaded_objects_coalesce_only_the_default_batch_size() {
        assert_eq!(
            effective_batch_size(DEFAULT_BATCH_SIZE, true),
            PRELOADED_BATCH_SIZE
        );
        assert_eq!(
            effective_batch_size(DEFAULT_BATCH_SIZE, false),
            DEFAULT_BATCH_SIZE
        );
        assert_eq!(effective_batch_size(1_024, true), 1_024);
    }
}
