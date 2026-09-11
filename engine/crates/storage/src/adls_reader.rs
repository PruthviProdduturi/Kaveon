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
const MAX_OBJECT_METADATA_CACHE_ENTRIES: usize = 256;
const MAX_OBJECT_STORE_CACHE_ENTRIES: usize = 32;
const FULL_OBJECT_CACHE_LIMIT: usize = 64 * 1024 * 1024;
const FULL_OBJECT_CACHE_MIN_ROW_GROUPS: usize = 32;
const PROCESS_FULL_OBJECT_CACHE_LIMIT: usize = 256 * 1024 * 1024;
const PROCESS_DECODED_BATCH_CACHE_LIMIT: usize = 256 * 1024 * 1024;
static FULL_OBJECT_CACHE_BYTES: AtomicUsize = AtomicUsize::new(0);
static FULL_OBJECT_CACHE: OnceLock<Mutex<HashMap<String, Arc<FullObjectEntry>>>> = OnceLock::new();
static DECODED_BATCH_CACHE: OnceLock<Mutex<DecodedBatchCache>> = OnceLock::new();
static OBJECT_METADATA_CACHE: OnceLock<Mutex<HashMap<String, object_store::ObjectMeta>>> =
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

fn cached_object_metadata(key: &str) -> Option<object_store::ObjectMeta> {
    OBJECT_METADATA_CACHE
        .get_or_init(Default::default)
        .lock()
        .ok()?
        .get(key)
        .cloned()
}

fn cache_object_metadata(key: String, metadata: object_store::ObjectMeta) {
    // A cached size is safe only when every subsequent byte request can be
    // pinned to an immutable object identity.
    if metadata.e_tag.is_none() && metadata.version.is_none() {
        return;
    }
    let Ok(mut cache) = OBJECT_METADATA_CACHE.get_or_init(Default::default).lock() else {
        return;
    };
    if cache.len() >= MAX_OBJECT_METADATA_CACHE_ENTRIES && !cache.contains_key(&key) {
        cache.clear();
    }
    cache.insert(key, metadata);
}

fn invalidate_object_metadata(key: &str) {
    if let Ok(mut cache) = OBJECT_METADATA_CACHE.get_or_init(Default::default).lock() {
        cache.remove(key);
    }
}

struct DecodedBatchEntry {
    batches: Arc<Vec<RecordBatch>>,
    _reservation: DecodedCacheReservation,
}

struct DecodedCacheReservation {
    bytes: usize,
    live_bytes: Arc<AtomicUsize>,
}

impl Drop for DecodedCacheReservation {
    fn drop(&mut self) {
        self.live_bytes.fetch_sub(self.bytes, Ordering::AcqRel);
    }
}

struct DecodedFillState {
    completed: tokio::sync::watch::Sender<bool>,
}

impl DecodedFillState {
    fn new() -> Self {
        let (completed, _) = tokio::sync::watch::channel(false);
        Self { completed }
    }

    fn complete(&self) {
        self.completed.send_replace(true);
    }
}

enum DecodedCacheSlot {
    Filling(Arc<DecodedFillState>),
    Ready {
        entry: Arc<DecodedBatchEntry>,
        last_used: u64,
    },
}

struct DecodedBatchCache {
    entries: HashMap<String, DecodedCacheSlot>,
    clock: u64,
    limit_bytes: usize,
    live_bytes: Arc<AtomicUsize>,
}

impl DecodedBatchCache {
    fn new(limit_bytes: usize) -> Self {
        Self {
            entries: HashMap::new(),
            clock: 0,
            limit_bytes,
            live_bytes: Arc::new(AtomicUsize::new(0)),
        }
    }

    fn tick(&mut self) -> u64 {
        self.clock = self.clock.wrapping_add(1);
        self.clock
    }

    fn reserve(&mut self, bytes: usize) -> (Option<DecodedCacheReservation>, u64) {
        let mut evictions = 0;
        loop {
            if self
                .live_bytes
                .fetch_update(Ordering::AcqRel, Ordering::Acquire, |current| {
                    current
                        .checked_add(bytes)
                        .filter(|&next| next <= self.limit_bytes)
                })
                .is_ok()
            {
                return (
                    Some(DecodedCacheReservation {
                        bytes,
                        live_bytes: Arc::clone(&self.live_bytes),
                    }),
                    evictions,
                );
            }
            let lru = self
                .entries
                .iter()
                .filter_map(|(key, slot)| match slot {
                    DecodedCacheSlot::Ready { entry, last_used }
                        if Arc::strong_count(entry) == 1 =>
                    {
                        Some((key.clone(), *last_used))
                    }
                    DecodedCacheSlot::Filling(_) => None,
                    DecodedCacheSlot::Ready { .. } => None,
                })
                .min_by_key(|(_, last_used)| *last_used)
                .map(|(key, _)| key);
            let Some(lru) = lru else {
                return (None, evictions);
            };
            self.entries.remove(&lru);
            evictions += 1;
        }
    }
}

impl Default for DecodedBatchCache {
    fn default() -> Self {
        Self::new(PROCESS_DECODED_BATCH_CACHE_LIMIT)
    }
}

struct DecodedCacheFill {
    key: String,
    state: Arc<DecodedFillState>,
    cache: &'static OnceLock<Mutex<DecodedBatchCache>>,
    published: bool,
}

impl DecodedCacheFill {
    fn publish(mut self, batches: Vec<RecordBatch>, metrics: &ScanMetrics) {
        let bytes = batches
            .iter()
            .map(RecordBatch::get_array_memory_size)
            .sum::<usize>();
        if bytes == 0 || bytes > PROCESS_DECODED_BATCH_CACHE_LIMIT {
            return;
        }
        let Ok(mut cache) = self.cache.get_or_init(Default::default).lock() else {
            return;
        };
        let owns_fill = matches!(
            cache.entries.get(&self.key),
            Some(DecodedCacheSlot::Filling(state)) if Arc::ptr_eq(state, &self.state)
        );
        if !owns_fill {
            return;
        }
        let (reservation, evictions) = cache.reserve(bytes);
        metrics.decoded_batch_cache_evictions(evictions);
        let Some(reservation) = reservation else {
            cache.entries.remove(&self.key);
            return;
        };
        let last_used = cache.tick();
        cache.entries.insert(
            self.key.clone(),
            DecodedCacheSlot::Ready {
                entry: Arc::new(DecodedBatchEntry {
                    batches: Arc::new(batches),
                    _reservation: reservation,
                }),
                last_used,
            },
        );
        self.published = true;
    }
}

impl Drop for DecodedCacheFill {
    fn drop(&mut self) {
        if !self.published
            && let Ok(mut cache) = self.cache.get_or_init(Default::default).lock()
            && matches!(
                cache.entries.get(&self.key),
                Some(DecodedCacheSlot::Filling(state)) if Arc::ptr_eq(state, &self.state)
            )
        {
            cache.entries.remove(&self.key);
        }
        self.state.complete();
    }
}

enum DecodedCacheAcquire {
    Hit(Arc<DecodedBatchEntry>),
    Fill(DecodedCacheFill),
}

async fn acquire_decoded_cache(key: String, metrics: &ScanMetrics) -> DecodedCacheAcquire {
    acquire_decoded_cache_from(&DECODED_BATCH_CACHE, key, metrics).await
}

async fn acquire_decoded_cache_from(
    decoded_cache: &'static OnceLock<Mutex<DecodedBatchCache>>,
    key: String,
    metrics: &ScanMetrics,
) -> DecodedCacheAcquire {
    loop {
        let wait = {
            let mut cache = decoded_cache
                .get_or_init(Default::default)
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let last_used = cache.tick();
            match cache.entries.get_mut(&key) {
                Some(DecodedCacheSlot::Ready {
                    entry,
                    last_used: used,
                }) => {
                    *used = last_used;
                    metrics.decoded_batch_cache_hit();
                    return DecodedCacheAcquire::Hit(Arc::clone(entry));
                }
                Some(DecodedCacheSlot::Filling(state)) => Some(Arc::clone(state)),
                None => {
                    let state = Arc::new(DecodedFillState::new());
                    cache
                        .entries
                        .insert(key.clone(), DecodedCacheSlot::Filling(Arc::clone(&state)));
                    metrics.decoded_batch_cache_miss();
                    return DecodedCacheAcquire::Fill(DecodedCacheFill {
                        key,
                        state,
                        cache: decoded_cache,
                        published: false,
                    });
                }
            }
        };
        let state = wait.expect("filling cache entry has a wait state");
        metrics.decoded_batch_cache_singleflight_wait();
        let mut completed = state.completed.subscribe();
        if !*completed.borrow_and_update() {
            let _ = completed.changed().await;
        }
    }
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

#[derive(Clone)]
struct CachedMetadata {
    object_identity: String,
    metadata: ArrowReaderMetadata,
}

static METADATA_CACHE: OnceLock<Mutex<HashMap<String, CachedMetadata>>> = OnceLock::new();

fn object_identity(metadata: &object_store::ObjectMeta) -> String {
    format!(
        "{}:{}:{}:{}",
        metadata.size,
        metadata
            .last_modified
            .timestamp_nanos_opt()
            .unwrap_or_default(),
        metadata.e_tag.as_deref().unwrap_or_default(),
        metadata.version.as_deref().unwrap_or_default()
    )
}

fn cached_metadata(key: &str, identity: &str) -> Option<ArrowReaderMetadata> {
    METADATA_CACHE
        .get_or_init(Default::default)
        .lock()
        .ok()?
        .get(key)
        .filter(|entry| entry.object_identity == identity)
        .map(|entry| entry.metadata.clone())
}

fn cache_metadata(key: String, identity: String, metadata: ArrowReaderMetadata) {
    let Ok(mut cache) = METADATA_CACHE.get_or_init(Default::default).lock() else {
        return;
    };
    if cache.len() >= MAX_METADATA_CACHE_ENTRIES && !cache.contains_key(&key) {
        cache.clear();
    }
    cache.insert(
        key,
        CachedMetadata {
            object_identity: identity,
            metadata,
        },
    );
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
    decoded_cache_fill: Option<DecodedCacheFill>,
    // Keep the entry and its byte reservation alive while cached batches are
    // being consumed, even if a newer fill evicts this key from the LRU.
    _decoded_cache_entry: Option<Arc<DecodedBatchEntry>>,
    decoded_batches: Vec<RecordBatch>,
    object_cache_key: String,
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
            Err(error) => {
                invalidate_object_metadata(&self.object_cache_key);
                return Err(parquet_error(error));
            }
        };
        self.metrics.read_time(started.elapsed());
        if let Some(batch) = &result {
            self.metrics.emitted(batch.num_rows());
            if self.decoded_cache_fill.is_some() {
                self.decoded_batches.push(batch.clone());
            }
        } else if let Some(fill) = self.decoded_cache_fill.take() {
            fill.publish(std::mem::take(&mut self.decoded_batches), &self.metrics);
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
        // A query can schedule several fragments for the same object at once.
        // Single-flight the first HEAD + footer load so a cold process performs
        // one remote initialization rather than one per fragment. Recheck both
        // caches after acquiring the lock because another fragment may have
        // populated them while this one was waiting.
        let cached_pair = cached_object_metadata(&cache_key).and_then(|object_metadata| {
            let identity = object_identity(&object_metadata);
            cached_metadata(&cache_key, &identity)
                .map(|metadata| (object_metadata, identity, metadata))
        });
        let (object_metadata, identity, metadata) = match cached_pair {
            Some(pair) => {
                metrics.object_metadata_cache_hit();
                pair
            }
            None => {
                let load_lock = metadata_load_lock(&cache_key);
                let _load_guard = load_lock.lock().await;
                let object_metadata = match cached_object_metadata(&cache_key) {
                    Some(metadata) => {
                        metrics.object_metadata_cache_hit();
                        metadata
                    }
                    None => {
                        let metadata = store.head(&path).await.map_err(object_store_error)?;
                        cache_object_metadata(cache_key.clone(), metadata.clone());
                        metadata
                    }
                };
                let identity = object_identity(&object_metadata);
                let metadata = match cached_metadata(&cache_key, &identity) {
                    Some(metadata) => metadata,
                    None => {
                        let mut object_reader =
                            AdlsObjectReader::new(store.clone(), object_metadata.clone(), None);
                        let metadata =
                            ArrowReaderMetadata::load_async(&mut object_reader, Default::default())
                                .await
                                .map_err(parquet_error)?;
                        cache_metadata(cache_key.clone(), identity.clone(), metadata.clone());
                        metadata
                    }
                };
                (object_metadata, identity, metadata)
            }
        };
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
        let acquired = match decoded_cache_key {
            Some(key) => Some(acquire_decoded_cache(key, &metrics).await),
            None => None,
        };
        let (cached, decoded_cache_fill, decoded_cache_entry) = match acquired {
            Some(DecodedCacheAcquire::Hit(entry)) => {
                (Some(Arc::clone(&entry.batches)), None, Some(entry))
            }
            Some(DecodedCacheAcquire::Fill(fill)) => (None, Some(fill), None),
            None => (None, None, None),
        };
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
            decoded_cache_fill,
            _decoded_cache_entry: decoded_cache_entry,
            decoded_batches: Vec::new(),
            object_cache_key: cache_key,
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
    use arrow::array::{AsArray, Int64Array};
    use object_store::{ObjectStore, PutPayload, memory::InMemory};

    fn decoded_batch(value: i64) -> RecordBatch {
        RecordBatch::try_from_iter(vec![(
            "value",
            Arc::new(Int64Array::from(vec![value; 32])) as arrow::array::ArrayRef,
        )])
        .unwrap()
    }

    fn decoded_test_cache(limit: usize) -> &'static OnceLock<Mutex<DecodedBatchCache>> {
        let cache = Box::leak(Box::new(OnceLock::new()));
        cache.set(Mutex::new(DecodedBatchCache::new(limit))).ok();
        cache
    }

    async fn publish_decoded(
        cache: &'static OnceLock<Mutex<DecodedBatchCache>>,
        key: &str,
        batch: RecordBatch,
        metrics: &ScanMetrics,
    ) {
        let DecodedCacheAcquire::Fill(fill) =
            acquire_decoded_cache_from(cache, key.into(), metrics).await
        else {
            panic!("test key unexpectedly cached");
        };
        fill.publish(vec![batch], metrics);
    }

    #[tokio::test]
    async fn decoded_cache_singleflights_concurrent_exact_identity_fills() {
        let batch = decoded_batch(7);
        let cache = decoded_test_cache(batch.get_array_memory_size() * 2);
        let owner_metrics = ScanMetrics::default();
        let waiter_metrics = ScanMetrics::default();
        let DecodedCacheAcquire::Fill(owner) =
            acquire_decoded_cache_from(cache, "object:etag-1".into(), &owner_metrics).await
        else {
            panic!("first lookup must own the fill");
        };
        let waiter_metrics_clone = waiter_metrics.clone();
        let waiter = tokio::spawn(async move {
            acquire_decoded_cache_from(cache, "object:etag-1".into(), &waiter_metrics_clone).await
        });
        tokio::task::yield_now().await;
        owner.publish(vec![batch], &owner_metrics);

        let DecodedCacheAcquire::Hit(entry) = waiter.await.unwrap() else {
            panic!("waiter must consume the published fill");
        };
        assert_eq!(entry.batches[0].num_rows(), 32);
        assert_eq!(owner_metrics.snapshot().decoded_batch_cache_misses, 1);
        assert_eq!(
            waiter_metrics
                .snapshot()
                .decoded_batch_cache_singleflight_waits,
            1
        );
        assert_eq!(waiter_metrics.snapshot().decoded_batch_cache_hits, 1);
    }

    #[tokio::test]
    async fn decoded_cache_lru_is_byte_bounded_and_live_readers_survive_eviction() {
        let batch_bytes = decoded_batch(1).get_array_memory_size();
        let limit = batch_bytes * 2;
        let cache = decoded_test_cache(limit);
        let metrics = ScanMetrics::default();
        publish_decoded(cache, "object:etag-a", decoded_batch(1), &metrics).await;
        publish_decoded(cache, "object:etag-b", decoded_batch(2), &metrics).await;
        let DecodedCacheAcquire::Hit(a) =
            acquire_decoded_cache_from(cache, "object:etag-a".into(), &metrics).await
        else {
            panic!("identity a must be cached");
        };
        // Touching a makes b the least-recently-used idle entry.
        publish_decoded(cache, "object:etag-c", decoded_batch(3), &metrics).await;
        assert_eq!(metrics.snapshot().decoded_batch_cache_evictions, 1);
        let DecodedCacheAcquire::Fill(evicted_b) =
            acquire_decoded_cache_from(cache, "object:etag-b".into(), &metrics).await
        else {
            panic!("least-recently-used identity b must be evicted");
        };
        drop(evicted_b);
        let DecodedCacheAcquire::Hit(c) =
            acquire_decoded_cache_from(cache, "object:etag-c".into(), &metrics).await
        else {
            panic!("identity c must be cached");
        };

        // Both resident entries are leased by active readers, so a fourth fill
        // is not published past the byte cap. Their Arrow buffers stay valid.
        publish_decoded(cache, "object:etag-d", decoded_batch(4), &metrics).await;
        assert_eq!(
            cache
                .get()
                .unwrap()
                .lock()
                .unwrap()
                .live_bytes
                .load(Ordering::Acquire),
            limit
        );
        assert_eq!(a.batches[0].num_rows(), 32);
        assert_eq!(c.batches[0].num_rows(), 32);
        drop(a);
        drop(c);

        publish_decoded(cache, "object:etag-d", decoded_batch(4), &metrics).await;
        let DecodedCacheAcquire::Hit(d) =
            acquire_decoded_cache_from(cache, "object:etag-d".into(), &metrics).await
        else {
            panic!("identity d must publish after old leases release");
        };
        assert_eq!(
            d.batches[0]
                .column(0)
                .as_primitive::<arrow::datatypes::Int64Type>()
                .value(0),
            4
        );
    }

    #[tokio::test]
    async fn abandoned_decoded_fill_is_never_published_and_wakes_a_retry() {
        let batch = decoded_batch(1);
        let cache = decoded_test_cache(batch.get_array_memory_size());
        let metrics = ScanMetrics::default();
        let DecodedCacheAcquire::Fill(abandoned) =
            acquire_decoded_cache_from(cache, "object:etag-failed".into(), &metrics).await
        else {
            panic!("first lookup must own the fill");
        };
        let waiter_metrics = metrics.clone();
        let retry = tokio::spawn(async move {
            acquire_decoded_cache_from(cache, "object:etag-failed".into(), &waiter_metrics).await
        });
        tokio::task::yield_now().await;
        drop(abandoned);

        let DecodedCacheAcquire::Fill(retry) = retry.await.unwrap() else {
            panic!("failed or cancelled fill must not become a hit");
        };
        retry.publish(vec![batch], &metrics);
        assert_eq!(metrics.snapshot().decoded_batch_cache_misses, 2);
    }

    #[tokio::test]
    async fn cached_object_metadata_is_identity_pinned_and_invalidatable() {
        let store: Arc<dyn ObjectStore> = Arc::new(InMemory::new());
        let path = Path::from(format!("identity-cache-{}.parquet", std::process::id()));
        store
            .put(&path, PutPayload::from_static(b"first"))
            .await
            .unwrap();
        let first = store.head(&path).await.unwrap();
        assert!(first.e_tag.is_some() || first.version.is_some());
        let key = path.to_string();
        cache_object_metadata(key.clone(), first.clone());
        assert_eq!(cached_object_metadata(&key).unwrap(), first);

        store
            .put(&path, PutPayload::from_static(b"second"))
            .await
            .unwrap();
        let mut pinned = AdlsObjectReader::new(store, first, None);
        assert!(pinned.get_bytes(0..1).await.is_err());

        invalidate_object_metadata(&key);
        assert!(cached_object_metadata(&key).is_none());
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

    #[tokio::test]
    async fn decoded_batches_are_reused_by_exact_snapshot_key() {
        let key = format!("decoded-cache-test-{}", std::process::id());
        let batch = RecordBatch::try_from_iter(vec![(
            "value",
            Arc::new(Int64Array::from(vec![1, 2, 3])) as _,
        )])
        .unwrap();
        let cache = decoded_test_cache(batch.get_array_memory_size() * 2);
        let metrics = ScanMetrics::default();

        publish_decoded(cache, &key, batch, &metrics).await;

        let DecodedCacheAcquire::Hit(cached) =
            acquire_decoded_cache_from(cache, key.clone(), &metrics).await
        else {
            panic!("exact cache key must resolve");
        };
        assert_eq!(cached.batches.len(), 1);
        assert_eq!(cached.batches[0].num_rows(), 3);
        assert!(matches!(
            acquire_decoded_cache_from(cache, format!("{key}:different-etag"), &metrics).await,
            DecodedCacheAcquire::Fill(_)
        ));
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
