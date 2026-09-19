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
use parquet::{
    errors::ParquetError,
    file::metadata::{ParquetMetaData, ParquetMetaDataReader},
};

use crate::{
    ScanMetrics, ScanPartition,
    parquet_reader::{
        bloom_prune_async, matching_row_groups, projection_indices, record_selection_metrics,
        validate_predicate,
    },
    scan_predicate::{BatchPredicate, LateMaterialisation, RowFilterPlan},
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
// Keep a fragment's remote range fan-out bounded. Parquet may request one
// range per projected column/row group; issuing all of those requests at once
// turns a single scan into an unbounded connection and memory burst on ADLS.
// `buffered` preserves the caller's range order while applying this limit.
const MAX_REMOTE_RANGE_CONCURRENCY: usize = 16;
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

pub(crate) fn cache_object_store(key: String, store: Arc<dyn ObjectStore>) {
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
    metrics: ScanMetrics,
}

/// Page ranges the decoder asks for under an offset index come one per
/// page; neighbours closer than this are read as one request.
const RANGE_COALESCE_GAP: usize = 1024 * 1024;

/// Merge ascending ranges whose gaps are at most `gap` bytes. Returns the
/// merged ranges and, per input range, the merged range it lies in and its
/// offset there. Ranges that are not ascending are returned as asked.
fn coalesce_ranges(
    ranges: &[std::ops::Range<usize>],
    gap: usize,
) -> (Vec<std::ops::Range<usize>>, Vec<(usize, usize)>) {
    let mut merged: Vec<std::ops::Range<usize>> = Vec::new();
    let mut placement = Vec::with_capacity(ranges.len());
    for range in ranges {
        match merged.last_mut() {
            Some(last) if range.start >= last.start && range.start <= last.end + gap => {
                last.end = last.end.max(range.end);
            }
            Some(last) if range.start < last.start => {
                return (
                    ranges.to_vec(),
                    (0..ranges.len()).map(|index| (index, 0)).collect(),
                );
            }
            _ => merged.push(range.clone()),
        }
        let index = merged.len() - 1;
        placement.push((index, range.start - merged[index].start));
    }
    (merged, placement)
}

impl AdlsObjectReader {
    fn new(
        store: Arc<dyn ObjectStore>,
        metadata: object_store::ObjectMeta,
        cache_key: Option<String>,
        metrics: ScanMetrics,
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
            metrics,
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
        self.metrics.bytes_read(range.len());
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
        self.metrics
            .bytes_read(ranges.iter().map(std::ops::Range::len).sum());
        if self.shared.is_none() {
            let store = self.store.clone();
            let path = self.path.clone();
            let e_tag = self.e_tag.clone();
            let version = self.version.clone();
            let (requests, placement) = coalesce_ranges(&ranges, RANGE_COALESCE_GAP);
            return async move {
                let answered = futures::stream::iter(requests.into_iter().map(|range| {
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
                .buffered(MAX_REMOTE_RANGE_CONCURRENCY)
                .collect::<Vec<_>>()
                .await
                .into_iter()
                .collect::<parquet::errors::Result<Vec<Bytes>>>()?;
                Ok(ranges
                    .iter()
                    .zip(placement)
                    .map(|(range, (request, offset))| {
                        answered[request].slice(offset..offset + range.len())
                    })
                    .collect())
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

/// Decoder lanes per scan of a large object. Defaults to the cores the process
/// may use, capped at four: decode is CPU-bound per lane and a worker's
/// container limit is what "cores" means here. `KAVEON_SCAN_PARALLELISM`
/// overrides it; 1 restores the single sequential decoder.
pub fn scan_parallelism() -> usize {
    let cores = std::thread::available_parallelism().map_or(1, usize::from);
    let configured = std::env::var("KAVEON_SCAN_PARALLELISM")
        .ok()
        .and_then(|value| value.trim().parse::<usize>().ok())
        .filter(|value| *value > 0);
    configured
        .unwrap_or_else(|| cores.min(MAX_SCAN_LANES))
        .max(1)
}

const MAX_SCAN_LANES: usize = 4;

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

/// The object reader with identity-pinned caches: ADLS first, but any
/// `object_store` backend through [`AdlsParquetReader::over_store`]. Its
/// caches are namespaced by `account/container`, so a file read through a
/// directory table and the same file read as a single-object table share
/// their footer, object metadata and decoded batches.
#[derive(Clone)]
pub struct AdlsParquetReader {
    account: String,
    container: String,
    object_path: String,
    auth_mode: AdlsAuthMode,
    /// A store supplied by the caller (a directory table's store, a test
    /// store) in place of the Azure client built from `account`/`container`.
    store: Option<Arc<dyn ObjectStore>>,
    batch_size: usize,
    columns: Option<Vec<String>>,
    predicate: Option<StoragePredicate>,
    partition: Option<ScanPartition>,
    /// The schema the catalog serves for the table, for the partition
    /// columns of a directory table at this location.
    catalog_schema: Option<SchemaRef>,
    metrics: Option<ScanMetrics>,
    late_materialisation: LateMaterialisation,
}

/// An object whose identity and footer are resolved: what a scan needs before
/// it decides how to decode.
pub(crate) struct OpenedObject {
    store: Arc<dyn ObjectStore>,
    cache_key: String,
    object_metadata: object_store::ObjectMeta,
    identity: String,
    metadata: ArrowReaderMetadata,
}

impl OpenedObject {
    /// The file's full Arrow schema, before projection.
    pub(crate) fn schema(&self) -> &SchemaRef {
        self.metadata.schema()
    }

    pub(crate) fn row_count(&self) -> Result<u64> {
        u64::try_from(self.metadata.metadata().file_metadata().num_rows())
            .map_err(|_| storage_error("Parquet metadata contains a negative row count"))
    }

    pub(crate) fn row_group_count(&self) -> usize {
        self.metadata.metadata().num_row_groups()
    }

    pub(crate) fn profile(&self) -> crate::FooterProfile {
        crate::FooterProfile::from_parquet(
            self.metadata.metadata(),
            self.object_metadata.size as u64,
            Some(self.object_metadata.last_modified.timestamp_millis()),
        )
    }
}

/// Why an object could not be opened: the location holds no object (a
/// directory table, or nothing at all), or a failure that stands.
pub(crate) enum OpenError {
    /// No object at this path.
    NotFound(String),
    Failed(KaveonError),
}

impl From<OpenError> for KaveonError {
    fn from(value: OpenError) -> Self {
        match value {
            OpenError::NotFound(path) => storage_error(format!("no object at '{path}'")),
            OpenError::Failed(error) => error,
        }
    }
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
            store: None,
            batch_size: DEFAULT_BATCH_SIZE,
            columns: None,
            predicate: None,
            partition: None,
            catalog_schema: None,
            metrics: None,
            late_materialisation: LateMaterialisation::from_environment(),
        }
    }

    /// A reader over a store the caller already holds. `account` and
    /// `container` only name the cache namespace; for an ADLS store they are
    /// the real ones so the caches are shared with [`Self::from_abfss_uri`].
    pub(crate) fn over_store(
        store: Arc<dyn ObjectStore>,
        account: impl Into<String>,
        container: impl Into<String>,
        object_path: impl Into<String>,
    ) -> Self {
        let mut reader = Self::new(account, container, object_path);
        reader.store = Some(store);
        reader
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

    /// The cache namespace (`account`, `container`) and the object path.
    pub(crate) fn namespace_and_path(&self) -> (String, String, String) {
        (
            self.account.clone(),
            self.container.clone(),
            self.object_path.clone(),
        )
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

    /// The schema the catalog serves for the table: when the location is a
    /// directory table, a partition column it names is read as the type it
    /// gives.
    pub fn with_catalog_schema(mut self, schema: SchemaRef) -> Self {
        self.catalog_schema = Some(schema);
        self
    }

    pub fn with_metrics(mut self, metrics: ScanMetrics) -> Self {
        self.metrics = Some(metrics);
        self
    }

    /// Whether the predicate's evaluable part runs inside the decoder
    /// (see [`LateMaterialisation`]); the process default otherwise.
    pub fn with_late_materialisation(mut self, mode: LateMaterialisation) -> Self {
        self.late_materialisation = mode;
        self
    }

    pub async fn read(&self) -> Result<AdlsBatchStream> {
        self.validate()?;
        let metrics = self.metrics.clone().unwrap_or_default();
        metrics.files_considered(1);
        let store = self.store(&metrics)?;
        let opened = self.open(store, &metrics).await?;
        metrics.file_opened();
        self.stream(opened, metrics).await
    }

    /// The store this reader reads from: the one supplied, else the cached
    /// Azure client for `account`/`container`, built once per process.
    pub(crate) fn store(&self, metrics: &ScanMetrics) -> Result<Arc<dyn ObjectStore>> {
        if let Some(store) = &self.store {
            return Ok(Arc::clone(store));
        }
        adls_store(&self.account, &self.container, self.auth_mode, metrics)
    }

    /// Resolve the object's identity and footer, through the single-flight
    /// caches. `NotFound` is reported apart from other failures because a
    /// location that holds no object may be a directory table.
    pub(crate) async fn open(
        &self,
        store: Arc<dyn ObjectStore>,
        metrics: &ScanMetrics,
    ) -> std::result::Result<OpenedObject, OpenError> {
        let path = Path::parse(&self.object_path)
            .map_err(|error| OpenError::Failed(storage_error(error.to_string())))?;
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
                        let metadata = match store.head(&path).await {
                            Ok(metadata) => metadata,
                            Err(object_store::Error::NotFound { .. }) => {
                                return Err(OpenError::NotFound(self.object_path.clone()));
                            }
                            Err(error) => return Err(OpenError::Failed(object_store_error(error))),
                        };
                        cache_object_metadata(cache_key.clone(), metadata.clone());
                        metadata
                    }
                };
                let identity = object_identity(&object_metadata);
                let metadata = match cached_metadata(&cache_key, &identity) {
                    Some(metadata) => metadata,
                    None => {
                        let mut object_reader = AdlsObjectReader::new(
                            store.clone(),
                            object_metadata.clone(),
                            None,
                            metrics.clone(),
                        );
                        let metadata =
                            ArrowReaderMetadata::load_async(&mut object_reader, Default::default())
                                .await
                                .map_err(|error| OpenError::Failed(parquet_error(error)))?;
                        cache_metadata(cache_key.clone(), identity.clone(), metadata.clone());
                        metadata
                    }
                };
                (object_metadata, identity, metadata)
            }
        };
        metrics.footer_time(footer_started.elapsed());
        Ok(OpenedObject {
            store,
            cache_key,
            object_metadata,
            identity,
            metadata,
        })
    }

    /// Decode an opened object: projection, row-group pruning, the partition's
    /// row groups, and the lanes.
    pub(crate) async fn stream(
        &self,
        opened: OpenedObject,
        metrics: ScanMetrics,
    ) -> Result<AdlsBatchStream> {
        let OpenedObject {
            store,
            cache_key,
            object_metadata,
            identity,
            metadata,
        } = opened;
        let preload =
            should_preload_object(object_metadata.size, metadata.metadata().num_row_groups());
        let object_cache_key = preload.then(|| format!("{cache_key}:{identity}"));
        let batch_size = effective_batch_size(self.batch_size, preload);
        let schema = Arc::clone(metadata.schema());
        let projection = self
            .columns
            .as_ref()
            .map(|columns| projection_indices(&schema, columns))
            .transpose()?;
        let coerced = self
            .predicate
            .as_ref()
            .map(|predicate| predicate.coerced_for(&schema));
        let mut row_groups = if let Some(predicate) = &coerced {
            validate_predicate(predicate, &schema)?;
            matching_row_groups(metadata.metadata().as_ref(), &schema, predicate)
        } else {
            (0..metadata.metadata().num_row_groups()).collect()
        };
        if let Some(partition) = self.partition {
            row_groups.retain(|ordinal| partition.contains(*ordinal));
        }
        if let Some(predicate) = &coerced {
            // The filters come through their own object reader so their
            // bytes are counted as Bloom filter bytes, not as bytes the
            // decoder read.
            let mut probe = ParquetRecordBatchStreamBuilder::new_with_metadata(
                AdlsObjectReader::new(
                    store.clone(),
                    object_metadata.clone(),
                    object_cache_key.clone(),
                    ScanMetrics::default(),
                ),
                metadata.clone(),
            );
            row_groups =
                bloom_prune_async(&mut probe, &schema, predicate, row_groups, &metrics).await?;
        }
        record_selection_metrics(
            metadata.metadata().as_ref(),
            &row_groups,
            projection.as_deref(),
            &metrics,
        );
        // Late materialisation: the evaluable part of the predicate runs
        // inside the decoder, one stage per conjunct, so the rows it
        // rejects are never decoded for the rest of the projection and a
        // row group it empties is never read for it. It costs a second
        // fetch round per row group, so it applies when the rest of the
        // projection outweighs the predicate's columns (or the object is
        // preloaded, where the second round reads memory).
        let row_filter_plan = coerced
            .as_ref()
            .and_then(|predicate| RowFilterPlan::new(predicate, &schema))
            .filter(|plan| {
                self.late_materialisation.applies(
                    metadata.metadata().as_ref(),
                    &row_groups,
                    projection.as_deref(),
                    &plan.columns(),
                    preload,
                )
            })
            .map(|mut plan| {
                plan.order_by_bytes(metadata.metadata().as_ref(), &row_groups);
                plan
            });
        // With a row filter, the decoder reads only the pages the selection
        // touches when the file carries an offset index; load it once per
        // object and keep it with the footer.
        let metadata = if row_filter_plan.is_some() {
            self.with_offset_index(
                &store,
                &cache_key,
                &object_metadata,
                &identity,
                metadata,
                &metrics,
            )
            .await?
        } else {
            metadata
        };
        // One decoder per lane, each over its own object reader; the footer,
        // projection and predicate are shared, the row groups are not.
        let build_stream =
            |groups: Vec<usize>| -> Result<ParquetRecordBatchStream<AdlsObjectReader>> {
                let reader = AdlsObjectReader::new(
                    store.clone(),
                    object_metadata.clone(),
                    object_cache_key.clone(),
                    metrics.clone(),
                );
                let mut builder =
                    ParquetRecordBatchStreamBuilder::new_with_metadata(reader, metadata.clone())
                        .with_batch_size(batch_size);
                if let Some(projection) = &projection {
                    let mask = ProjectionMask::roots(builder.parquet_schema(), projection.clone());
                    builder = builder.with_projection(mask);
                }
                if let Some(plan) = &row_filter_plan {
                    let row_filter = plan.row_filter(builder.parquet_schema(), &metrics);
                    builder = builder.with_row_filter(row_filter);
                }
                builder
                    .with_row_groups(groups)
                    .build()
                    .map_err(parquet_error)
            };
        // The decoded batches depend on the predicate (a row filter or a
        // lane predicate drops rows), so it is part of the key: a later
        // scan with another predicate never sees these batches.
        let decoded_cache_key = preload.then(|| {
            format!(
                "{cache_key}:{identity}:batch={}:projection={projection:?}:predicate={:?}:row_groups={row_groups:?}",
                batch_size, self.predicate
            )
        });
        // A preloaded object is decoded once and shared through the decoded
        // cache, in file order; a large object is decoded by several lanes at
        // once so network fetch and decode overlap and every core the worker
        // was given does work. Lanes interleave row groups round-robin so
        // each sees the whole file's range of days.
        let lanes = if preload {
            1
        } else {
            scan_parallelism().min(row_groups.len()).max(1)
        };
        // The decoder emits projected columns in file order; the caller's
        // order is restored per batch, exactly as before the lanes.
        let projected_schema = match &projection {
            Some(indices) => {
                let mut in_file_order = indices.clone();
                in_file_order.sort_unstable();
                in_file_order.dedup();
                Arc::new(
                    schema
                        .project(&in_file_order)
                        .map_err(|error| storage_error(error.to_string()))?,
                )
            }
            None => Arc::clone(&schema),
        };
        // Without a row filter, the lanes evaluate the same predicate on
        // each decoded batch (see BatchPredicate) so rejected rows never
        // leave the lane; the executor still applies the whole predicate.
        let lane_predicate = if row_filter_plan.is_some() {
            None
        } else {
            coerced
                .as_ref()
                .and_then(|predicate| BatchPredicate::new(&projected_schema, predicate))
                .map(Arc::new)
        };
        let (schema, output_projection) =
            crate::parquet_reader::ordered_projection(projected_schema, self.columns.as_deref())?;
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
                None if lanes <= 1 => {
                    let stream = build_stream(row_groups)?;
                    match lane_predicate {
                        Some(predicate) => Box::pin(
                            stream.map(move |item| item.and_then(|batch| predicate.apply(batch))),
                        ),
                        None => Box::pin(stream),
                    }
                }
                None => {
                    let mut assignments: Vec<Vec<usize>> = vec![Vec::new(); lanes];
                    for (index, group) in row_groups.iter().enumerate() {
                        assignments[index % lanes].push(*group);
                    }
                    let (sender, receiver) = tokio::sync::mpsc::channel(lanes * 2);
                    for groups in assignments.into_iter().filter(|groups| !groups.is_empty()) {
                        let mut stream = build_stream(groups)?;
                        let sender = sender.clone();
                        let predicate = lane_predicate.clone();
                        let lane_metrics = metrics.clone();
                        tokio::spawn(async move {
                            let started = std::time::Instant::now();
                            let mut rows = 0u64;
                            while let Some(item) = stream.next().await {
                                let item = match &predicate {
                                    Some(predicate) => {
                                        item.and_then(|batch| predicate.apply(batch))
                                    }
                                    None => item,
                                };
                                if let Ok(batch) = &item {
                                    rows += batch.num_rows() as u64;
                                }
                                let failed = item.is_err();
                                if sender.send(item).await.is_err() || failed {
                                    return;
                                }
                            }
                            lane_metrics.lane_finished(rows, started.elapsed());
                        });
                    }
                    drop(sender);
                    Box::pin(futures::stream::unfold(
                        receiver,
                        |mut receiver| async move { receiver.recv().await.map(|item| (item, receiver)) },
                    ))
                }
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

    /// The footer with the file's offset index loaded, from the cache or
    /// from the object once; the footer alone when the file carries none.
    async fn with_offset_index(
        &self,
        store: &Arc<dyn ObjectStore>,
        cache_key: &str,
        object_metadata: &object_store::ObjectMeta,
        identity: &str,
        metadata: ArrowReaderMetadata,
        metrics: &ScanMetrics,
    ) -> Result<ArrowReaderMetadata> {
        let parquet = metadata.metadata();
        let carries_index = parquet
            .row_groups()
            .iter()
            .flat_map(|group| group.columns())
            .any(|column| column.offset_index_offset().is_some());
        if !carries_index
            || parquet
                .offset_index()
                .is_some_and(|index| !index.is_empty())
        {
            return Ok(metadata);
        }
        let load_lock = metadata_load_lock(cache_key);
        let _load_guard = load_lock.lock().await;
        if let Some(cached) = cached_metadata(cache_key, identity)
            && cached
                .metadata()
                .offset_index()
                .is_some_and(|index| !index.is_empty())
        {
            return Ok(cached);
        }
        let started = Instant::now();
        let mut object_reader = AdlsObjectReader::new(
            Arc::clone(store),
            object_metadata.clone(),
            None,
            metrics.clone(),
        );
        let mut reader = ParquetMetaDataReader::new_with_metadata(parquet.as_ref().clone())
            .with_offset_indexes(true)
            .with_column_indexes(false);
        reader
            .load_page_index(&mut object_reader)
            .await
            .map_err(parquet_error)?;
        let with_index = ArrowReaderMetadata::try_new(
            Arc::new(reader.finish().map_err(parquet_error)?),
            Default::default(),
        )
        .map_err(parquet_error)?;
        metrics.footer_time(started.elapsed());
        cache_metadata(
            cache_key.to_owned(),
            identity.to_owned(),
            with_index.clone(),
        );
        Ok(with_index)
    }

    pub fn read_blocking(self) -> Result<AdlsBatchSource> {
        self.validate()?;
        let (initial_sender, initial_receiver) = mpsc::sync_channel(1);
        let (batch_sender, batch_receiver) = mpsc::sync_channel(2);
        std::thread::Builder::new()
            .name("kaveon-adls-reader".into())
            .spawn(move || {
                let runtime = match tokio::runtime::Builder::new_multi_thread()
                    .worker_threads(scan_parallelism())
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
                    let metrics = self.metrics.clone().unwrap_or_default();
                    let store = match self.store(&metrics) {
                        Ok(store) => store,
                        Err(error) => {
                            let _ = initial_sender.send(Err(error));
                            return;
                        }
                    };
                    let opened = match self.open(Arc::clone(&store), &metrics).await {
                        Ok(opened) => opened,
                        Err(OpenError::NotFound(_)) => {
                            // No object at the location: a directory of
                            // Parquet files is a table too.
                            self.directory_reader(store, metrics)
                                .run(initial_sender, batch_sender)
                                .await;
                            return;
                        }
                        Err(OpenError::Failed(error)) => {
                            let _ = initial_sender.send(Err(error));
                            return;
                        }
                    };
                    metrics.files_considered(1);
                    metrics.file_opened();
                    let mut stream = match self.stream(opened, metrics).await {
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

    /// The same location read as a directory table, with this reader's
    /// projection, predicate, partition and metrics.
    fn directory_reader(
        &self,
        store: Arc<dyn ObjectStore>,
        metrics: ScanMetrics,
    ) -> crate::ObjectDirectoryReader {
        let mut reader = crate::ObjectDirectoryReader::new(
            store,
            &self.account,
            &self.container,
            Path::from(self.object_path.as_str()),
        )
        .with_batch_size(self.batch_size)
        .with_metrics(metrics);
        if let Some(columns) = &self.columns {
            reader = reader.with_columns(columns.clone());
        }
        if let Some(predicate) = &self.predicate {
            reader = reader.with_predicate(predicate.clone());
        }
        if let Some(partition) = self.partition {
            reader = reader.with_partition(partition);
        }
        if let Some(schema) = &self.catalog_schema {
            reader = reader.with_catalog_schema(Arc::clone(schema));
        }
        reader
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

/// The cached Azure client for one account, container and credential mode.
pub(crate) fn adls_store(
    account: &str,
    container: &str,
    auth_mode: AdlsAuthMode,
    metrics: &ScanMetrics,
) -> Result<Arc<dyn ObjectStore>> {
    let store_key = format!("{account}/{container}/{auth_mode:?}");
    if let Some(store) = cached_object_store(&store_key) {
        metrics.object_store_cache_hit();
        return Ok(store);
    }
    let store: Arc<dyn ObjectStore> = Arc::new(
        MicrosoftAzureBuilder::from_env()
            .with_account(account)
            .with_container_name(container)
            .with_use_azure_cli(auth_mode == AdlsAuthMode::AzureCli)
            .build()
            .map_err(object_store_error)?,
    );
    cache_object_store(store_key, store.clone());
    Ok(store)
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
    use crate::ScanMetricsSnapshot;
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
        let mut pinned = AdlsObjectReader::new(store, first, None, ScanMetrics::default());
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

    /// Six row groups of the sequence 0..600, decoded by several lanes at once:
    /// the same rows come out, whatever the lane count and interleaving.
    async fn lanes_fixture(account: &str) -> AdlsParquetReader {
        use parquet::arrow::ArrowWriter;
        use parquet::file::properties::WriterProperties;
        let schema = Arc::new(arrow::datatypes::Schema::new(vec![
            arrow::datatypes::Field::new("label", arrow::datatypes::DataType::Utf8, false),
            arrow::datatypes::Field::new("value", arrow::datatypes::DataType::Int64, false),
            arrow::datatypes::Field::new("other", arrow::datatypes::DataType::Int64, false),
        ]));
        let batch = RecordBatch::try_new(
            Arc::clone(&schema),
            vec![
                Arc::new(arrow::array::StringArray::from(
                    (0..600).map(|i| format!("row-{i}")).collect::<Vec<_>>(),
                )),
                Arc::new(Int64Array::from((0..600).collect::<Vec<i64>>())),
                Arc::new(Int64Array::from(vec![7_i64; 600])),
            ],
        )
        .unwrap();
        let mut bytes = Vec::new();
        let properties = WriterProperties::builder()
            .set_max_row_group_size(100)
            .build();
        let mut writer = ArrowWriter::try_new(&mut bytes, schema, Some(properties)).unwrap();
        writer.write(&batch).unwrap();
        writer.close().unwrap();
        let store: Arc<dyn ObjectStore> = Arc::new(InMemory::new());
        store
            .put(&Path::from("lanes.parquet"), PutPayload::from(bytes))
            .await
            .unwrap();
        cache_object_store(
            format!("{account}/lanes/{:?}", AdlsAuthMode::Environment),
            store,
        );
        AdlsParquetReader::new(account, "lanes", "lanes.parquet")
    }

    async fn sum_of_values(reader: AdlsParquetReader) -> (i64, usize, u64) {
        // Request the columns out of file order; `value` must come back first.
        let mut stream = reader
            .with_columns(vec!["other".into(), "value".into()])
            .read()
            .await
            .unwrap();
        assert_eq!(stream.schema().field(0).name(), "other");
        let (mut total, mut rows) = (0_i64, 0_usize);
        while let Some(batch) = stream.next_batch().await.unwrap() {
            assert_eq!(batch.num_columns(), 2);
            assert_eq!(batch.schema().field(1).name(), "value");
            let values = batch
                .column(1)
                .as_primitive::<arrow::datatypes::Int64Type>();
            total += values.iter().flatten().sum::<i64>();
            rows += batch.num_rows();
        }
        let snapshot = stream.metrics().snapshot();
        // Every lane that ran reported itself, and together they decoded
        // every row: the spread is real per-lane work, not a guess.
        if snapshot.lanes > 0 {
            assert!(snapshot.lanes as usize <= scan_parallelism().max(1));
            assert!(snapshot.lane_rows_min <= snapshot.lane_rows_max);
            assert!(snapshot.lane_rows_max as usize <= rows);
            assert!(snapshot.lane_elapsed_min <= snapshot.lane_elapsed_max);
        }
        (total, rows, snapshot.row_groups_selected)
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 3)]
    async fn parallel_lanes_decode_every_row_group_exactly_once() {
        let account = format!("lanes-{}", std::process::id());
        let reader = lanes_fixture(&account).await;
        assert_eq!(
            sum_of_values(reader.clone().with_batch_size(64)).await,
            ((0..600).sum::<i64>(), 600, 6)
        );
        // A predicate prunes row groups before the lanes are formed.
        let predicate = StoragePredicate::Compare {
            column: "value".into(),
            op: kaveon_core::CompareOp::Ge,
            value: kaveon_core::ScalarValue::Int64(400),
        };
        assert_eq!(
            sum_of_values(reader.with_batch_size(64).with_predicate(predicate)).await,
            ((400..600).sum::<i64>(), 200, 2)
        );
    }

    #[test]
    fn scan_parallelism_is_bounded_and_overridable() {
        assert!(scan_parallelism() >= 1);
        assert!(scan_parallelism() <= MAX_SCAN_LANES.max(1));
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
    async fn remote_range_reads_are_bounded_and_keep_request_order() {
        let store: Arc<dyn ObjectStore> = Arc::new(InMemory::new());
        let path = Path::from("ordered-ranges.parquet");
        store
            .put(&path, PutPayload::from_static(b"0123456789abcdef"))
            .await
            .unwrap();
        let metadata = store.head(&path).await.unwrap();
        // No shared full-object cache: this exercises the remote range path.
        let mut reader = AdlsObjectReader::new(store, metadata, None, ScanMetrics::default());
        let ranges = vec![12..16, 0..2, 8..12, 4..8];
        let result = reader.get_byte_ranges(ranges).await.unwrap();
        assert_eq!(
            result,
            vec![
                Bytes::from_static(b"cdef"),
                Bytes::from_static(b"01"),
                Bytes::from_static(b"89ab"),
                Bytes::from_static(b"4567"),
            ]
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
        let mut reader = AdlsObjectReader::new(
            store.clone(),
            metadata.clone(),
            Some(key.clone()),
            ScanMetrics::default(),
        );
        let mut concurrent =
            AdlsObjectReader::new(store, metadata, Some(key), ScanMetrics::default());
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

    /// A text column with a `%google%` hit every fiftieth row beside a wide
    /// payload column, in row groups of 100 rows and pages of 20, as
    /// dictionary or plain text, with or without an offset index.
    async fn row_filter_fixture(
        account: &str,
        rows: usize,
        row_group_rows: usize,
        dictionary: bool,
        offset_index: bool,
    ) -> AdlsParquetReader {
        use arrow::array::{StringArray, StringDictionaryBuilder};
        use arrow::datatypes::{DataType, Field, Int32Type, Schema};
        use parquet::arrow::ArrowWriter;
        use parquet::file::properties::WriterProperties;
        let text_type = if dictionary {
            DataType::Dictionary(Box::new(DataType::Int32), Box::new(DataType::Utf8))
        } else {
            DataType::Utf8
        };
        let schema = Arc::new(Schema::new(vec![
            Field::new("id", DataType::Int64, false),
            Field::new("url", text_type, false),
            Field::new("payload", DataType::Utf8, false),
        ]));
        let urls = (0..rows).map(|row| {
            if row.is_multiple_of(50) {
                format!("http://www.google.com/search?q={row}")
            } else {
                format!("http://site-{row}.example/path")
            }
        });
        let url: arrow::array::ArrayRef = if dictionary {
            let mut builder = StringDictionaryBuilder::<Int32Type>::new();
            for value in urls {
                builder.append_value(value);
            }
            Arc::new(builder.finish())
        } else {
            Arc::new(StringArray::from_iter_values(urls))
        };
        let batch = RecordBatch::try_new(
            Arc::clone(&schema),
            vec![
                Arc::new(Int64Array::from((0..rows as i64).collect::<Vec<_>>())),
                url,
                Arc::new(StringArray::from_iter_values(
                    (0..rows).map(|row| format!("{row:0>200}")),
                )),
            ],
        )
        .unwrap();
        let mut bytes = Vec::new();
        let properties = WriterProperties::builder()
            .set_max_row_group_size(row_group_rows)
            .set_write_batch_size(20)
            .set_data_page_row_count_limit(20)
            .set_dictionary_enabled(dictionary)
            .set_offset_index_disabled(!offset_index)
            .build();
        let mut writer = ArrowWriter::try_new(&mut bytes, schema, Some(properties)).unwrap();
        writer.write(&batch).unwrap();
        writer.close().unwrap();
        let store: Arc<dyn ObjectStore> = Arc::new(InMemory::new());
        store
            .put(&Path::from("wide.parquet"), PutPayload::from(bytes))
            .await
            .unwrap();
        cache_object_store(
            format!("{account}/wide/{:?}", AdlsAuthMode::Environment),
            store,
        );
        AdlsParquetReader::new(account, "wide", "wide.parquet")
    }

    fn google_urls() -> StoragePredicate {
        StoragePredicate::Like {
            column: "url".into(),
            pattern: "%google%".into(),
            negated: false,
            case_insensitive: false,
        }
    }

    /// The ids a scan returns, in file order, and its metrics.
    async fn ids_of(reader: AdlsParquetReader) -> (Vec<i64>, ScanMetricsSnapshot) {
        let metrics = ScanMetrics::default();
        let mut stream = reader.with_metrics(metrics.clone()).read().await.unwrap();
        let mut ids = Vec::new();
        while let Some(batch) = stream.next_batch().await.unwrap() {
            let column = batch
                .column(stream.schema().index_of("id").unwrap())
                .as_primitive::<arrow::datatypes::Int64Type>();
            ids.extend(column.values().iter().copied());
        }
        ids.sort_unstable();
        (ids, metrics.snapshot())
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 3)]
    async fn row_filter_admits_exactly_the_matching_rows_over_object_storage() {
        let expected = (0..600).step_by(50).map(i64::from).collect::<Vec<_>>();
        for dictionary in [true, false] {
            let mut bytes_read = Vec::new();
            for offset_index in [true, false] {
                let account = format!(
                    "row-filter-{}-{dictionary}-{offset_index}",
                    std::process::id()
                );
                let reader = row_filter_fixture(&account, 600, 100, dictionary, offset_index)
                    .await
                    .with_predicate(google_urls());
                // The lanes filter decoded batches: the same rows, no row
                // filter ran.
                let (ids, snapshot) = ids_of(
                    reader
                        .clone()
                        .with_late_materialisation(LateMaterialisation::Never),
                )
                .await;
                assert_eq!(ids, expected);
                assert_eq!(snapshot.row_filter_rows_examined, 0);
                assert_eq!(snapshot.row_groups_selected, 6);
                // The row filter: every row examined, the twelve admitted,
                // and only they are decoded for the payload.
                let (ids, snapshot) = ids_of(
                    reader
                        .clone()
                        .with_late_materialisation(LateMaterialisation::Always),
                )
                .await;
                assert_eq!(ids, expected);
                assert_eq!(snapshot.row_filter_rows_examined, 600);
                assert_eq!(snapshot.row_filter_rows_admitted, 12);
                assert_eq!(snapshot.rows_emitted, 12);
                assert!(snapshot.compressed_bytes_read > 0);
                bytes_read.push(snapshot.compressed_bytes_read);
                // Auto: the payload outweighs the url, so the filter runs
                // for the whole projection; for `id, url` it does not.
                let (ids, snapshot) = ids_of(
                    reader
                        .clone()
                        .with_late_materialisation(LateMaterialisation::Auto),
                )
                .await;
                assert_eq!(ids, expected);
                assert_eq!(snapshot.row_filter_rows_examined, 600);
                let (ids, snapshot) = ids_of(
                    reader
                        .clone()
                        .with_columns(vec!["id".into(), "url".into()])
                        .with_late_materialisation(LateMaterialisation::Auto),
                )
                .await;
                assert_eq!(ids, expected);
                assert_eq!(snapshot.row_filter_rows_examined, 0);
                // A conjunct the storage layer cannot evaluate is left to
                // the executor; the rest still runs as a stage.
                let (ids, snapshot) = ids_of(
                    reader
                        .clone()
                        .with_predicate(StoragePredicate::Compare {
                            column: "id".into(),
                            op: kaveon_core::CompareOp::Ge,
                            value: kaveon_core::ScalarValue::Int64(300),
                        })
                        .with_late_materialisation(LateMaterialisation::Always),
                )
                .await;
                assert_eq!(ids, vec![300, 350, 400, 450, 500, 550]);
                // Statistics dropped the first three row groups; the
                // filter examined the rest.
                assert_eq!(snapshot.row_groups_selected, 3);
                assert_eq!(snapshot.row_filter_rows_examined, 300);
                assert_eq!(snapshot.row_filter_rows_admitted, 6);
            }
            // With the offset index the decoder reads only the pages the
            // selection touches (two of five per row group for the payload);
            // without it, every page.
            assert!(
                bytes_read[0] < bytes_read[1],
                "dictionary {dictionary}: {} bytes with the offset index, {} without",
                bytes_read[0],
                bytes_read[1]
            );
        }
    }

    /// A small object of many row groups is held in memory and decoded once
    /// per predicate: the row filter runs (the second round reads memory),
    /// the decoded batches are keyed by the predicate, and a scan with
    /// another predicate never sees them.
    #[tokio::test(flavor = "multi_thread", worker_threads = 3)]
    async fn preloaded_objects_row_filter_and_key_the_decoded_cache_by_predicate() {
        let account = format!("row-filter-preload-{}", std::process::id());
        let reader = row_filter_fixture(&account, 400, 10, false, true).await;
        let expected = (0..400).step_by(50).map(i64::from).collect::<Vec<_>>();
        let (ids, snapshot) = ids_of(reader.clone().with_predicate(google_urls())).await;
        assert_eq!(ids, expected);
        assert_eq!(snapshot.row_filter_rows_examined, 400);
        assert_eq!(snapshot.row_filter_rows_admitted, 8);
        assert_eq!(snapshot.decoded_batch_cache_misses, 1);
        assert_eq!(snapshot.decoded_batch_cache_hits, 0);
        let (ids, snapshot) = ids_of(reader.clone().with_predicate(StoragePredicate::Compare {
            column: "id".into(),
            op: kaveon_core::CompareOp::Ge,
            value: kaveon_core::ScalarValue::Int64(300),
        }))
        .await;
        assert_eq!(ids, (300..400).collect::<Vec<_>>());
        assert_eq!(snapshot.decoded_batch_cache_misses, 1);
        assert_eq!(snapshot.decoded_batch_cache_hits, 0);
        // The same predicate again is served from the cache, whichever
        // way the rows were filtered: the row filter and the lane predicate
        // admit the same rows.
        let (ids, snapshot) = ids_of(
            reader
                .clone()
                .with_predicate(google_urls())
                .with_late_materialisation(LateMaterialisation::Never),
        )
        .await;
        assert_eq!(ids, expected);
        assert_eq!(snapshot.decoded_batch_cache_hits, 1);
        assert_eq!(snapshot.row_filter_rows_examined, 0);
        // The operator's `never` is honoured for a preloaded object too.
        let (ids, snapshot) = ids_of(
            reader
                .with_predicate(StoragePredicate::Compare {
                    column: "id".into(),
                    op: kaveon_core::CompareOp::Lt,
                    value: kaveon_core::ScalarValue::Int64(20),
                })
                .with_late_materialisation(LateMaterialisation::Never),
        )
        .await;
        assert_eq!(ids, (0..20).collect::<Vec<_>>());
        assert_eq!(snapshot.decoded_batch_cache_misses, 1);
        assert_eq!(snapshot.row_filter_rows_examined, 0);
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
