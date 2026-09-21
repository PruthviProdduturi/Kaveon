use std::collections::HashMap;
use std::fmt::{Display, Formatter};
use std::io::Cursor;
use std::sync::{Arc, RwLock};

use arrow::datatypes::SchemaRef;
use arrow::ipc::reader::StreamReader;
use arrow::ipc::writer::StreamWriter;
use arrow::record_batch::RecordBatch;
use axum::Router;
use axum::body::Bytes;
use axum::extract::{DefaultBodyLimit, Path, State};
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use kaveon_core::{ExchangeId, TaskId};

const WIRE_MAGIC: [u8; 4] = *b"KVEX";
const WIRE_VERSION: u16 = 2;
const FIXED_HEADER_BYTES: usize = 62;
const CHECKSUM_OFFSET: usize = 54;
const FNV_OFFSET_BASIS: u64 = 14_695_981_039_346_656_037;
const FNV_PRIME: u64 = 1_099_511_628_211;
const BEARER_PREFIX: &str = "Bearer ";
// One exchange partition's payload: 1 GiB in 4 MiB chunks. A partial
// aggregate over tens of millions of groups needs the room; the producer
// encodes and uploads one partition at a time.
// A streamed output partition is bounded by the receiver's disk quotas,
// not by memory: 2 048 chunks of 4 MiB is 8 GiB per partition.
const DEFAULT_MAX_PAYLOAD_BYTES: usize = 8 * 1024 * 1024 * 1024;
const DEFAULT_MAX_CHUNK_BYTES: usize = 4 * 1024 * 1024;
const DEFAULT_MAX_CHUNKS: usize = 2048;
const MAX_WIRE_ENVELOPE_OVERHEAD_BYTES: usize = 64 * 1024;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ExchangeLimits {
    pub max_payload_bytes: usize,
    pub max_chunk_bytes: usize,
    pub max_chunks: usize,
}

impl Default for ExchangeLimits {
    fn default() -> Self {
        Self {
            max_payload_bytes: DEFAULT_MAX_PAYLOAD_BYTES,
            max_chunk_bytes: DEFAULT_MAX_CHUNK_BYTES,
            max_chunks: DEFAULT_MAX_CHUNKS,
        }
    }
}

impl ExchangeLimits {
    fn validate(self) -> ExchangeResult<()> {
        if self.max_payload_bytes == 0 || self.max_chunk_bytes == 0 || self.max_chunks == 0 {
            return Err(ExchangeError::InvalidLimits);
        }
        Ok(())
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct ExchangeIdentity {
    pub exchange_id: ExchangeId,
    pub task_id: TaskId,
    pub output_partition: usize,
}

const DEFAULT_MAX_BUFFERED_EXCHANGES: usize = 1_024;
const DEFAULT_MAX_BUFFERED_BYTES: usize = 512 * 1024 * 1024;
const EXCHANGE_MEDIA_TYPE: &str = "application/vnd.kaveon.exchange.v2";
static DOWNLOAD_BYTES: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
struct DownloadPermit(usize);
impl DownloadPermit {
    fn new(bytes: usize) -> ExchangeResult<Self> {
        DOWNLOAD_BYTES
            .fetch_update(
                std::sync::atomic::Ordering::AcqRel,
                std::sync::atomic::Ordering::Acquire,
                |used| {
                    used.checked_add(bytes)
                        .filter(|total| *total <= DEFAULT_MAX_BUFFERED_BYTES)
                },
            )
            .map_err(|_| ExchangeError::StoreCapacityExceeded)?;
        Ok(Self(bytes))
    }
}
impl Drop for DownloadPermit {
    fn drop(&mut self) {
        DOWNLOAD_BYTES.fetch_sub(self.0, std::sync::atomic::Ordering::AcqRel);
    }
}

#[derive(Debug, Default)]
struct ExchangeStoreState {
    chunks: HashMap<ExchangeIdentity, Vec<ExchangeChunk>>,
    buffered_bytes: usize,
}

#[derive(Debug)]
pub struct ExchangeStore {
    state: RwLock<ExchangeStoreState>,
    max_buffered_exchanges: usize,
    max_buffered_bytes: usize,
}

impl Default for ExchangeStore {
    fn default() -> Self {
        Self {
            state: RwLock::new(ExchangeStoreState::default()),
            max_buffered_exchanges: DEFAULT_MAX_BUFFERED_EXCHANGES,
            max_buffered_bytes: DEFAULT_MAX_BUFFERED_BYTES,
        }
    }
}

impl ExchangeStore {
    pub fn insert(&self, chunk: ExchangeChunk) -> ExchangeResult<()> {
        let mut state = self
            .state
            .write()
            .map_err(|_| ExchangeError::StorePoisoned)?;
        if !state.chunks.contains_key(&chunk.identity)
            && state.chunks.len() >= self.max_buffered_exchanges
        {
            return Err(ExchangeError::StoreCapacityExceeded);
        }
        if let Some(existing) = state.chunks.get(&chunk.identity).and_then(|chunks| {
            chunks
                .iter()
                .find(|existing| existing.chunk_index == chunk.chunk_index)
        }) {
            return if existing == &chunk {
                Ok(())
            } else {
                Err(ExchangeError::ConflictingChunk)
            };
        }
        let buffered_bytes = state
            .buffered_bytes
            .checked_add(chunk.payload.len())
            .ok_or(ExchangeError::StoreCapacityExceeded)?;
        if buffered_bytes > self.max_buffered_bytes {
            return Err(ExchangeError::StoreCapacityExceeded);
        }
        state.buffered_bytes = buffered_bytes;
        state
            .chunks
            .entry(chunk.identity.clone())
            .or_default()
            .push(chunk);
        Ok(())
    }

    pub fn get(&self, identity: &ExchangeIdentity) -> ExchangeResult<Vec<ExchangeChunk>> {
        self.state
            .read()
            .map_err(|_| ExchangeError::StorePoisoned)?
            .chunks
            .get(identity)
            .cloned()
            .ok_or(ExchangeError::ExchangeNotFound)
    }

    pub fn remove(&self, identity: &ExchangeIdentity) -> ExchangeResult<bool> {
        let mut state = self
            .state
            .write()
            .map_err(|_| ExchangeError::StorePoisoned)?;
        let Some(chunks) = state.chunks.remove(identity) else {
            return Ok(false);
        };
        let released_bytes = chunks.iter().try_fold(0_usize, |total, chunk| {
            total.checked_add(chunk.payload.len())
        });
        state.buffered_bytes = state
            .buffered_bytes
            .checked_sub(released_bytes.ok_or(ExchangeError::StorePoisoned)?)
            .ok_or(ExchangeError::StorePoisoned)?;
        Ok(true)
    }

    pub fn buffered_bytes(&self) -> ExchangeResult<usize> {
        Ok(self
            .state
            .read()
            .map_err(|_| ExchangeError::StorePoisoned)?
            .buffered_bytes)
    }

    #[cfg(test)]
    fn with_capacity(max_buffered_exchanges: usize, max_buffered_bytes: usize) -> Self {
        Self {
            state: RwLock::new(ExchangeStoreState::default()),
            max_buffered_exchanges,
            max_buffered_bytes,
        }
    }
}

type ExchangePath = (String, String, u32, usize, u32, usize);

pub fn routes() -> Router<Arc<crate::AppState>> {
    Router::new()
        .route("/v1/internal/exchange", post(upload_exchange_chunk))
        .route(
            "/v1/internal/exchange/{query_id}/{exchange_id}/{stage_id}/{source_partition}/{attempt}/{output_partition}",
            get(download_exchange).delete(delete_exchange),
        )
        .layer(DefaultBodyLimit::max(
            DEFAULT_MAX_CHUNK_BYTES + MAX_WIRE_ENVELOPE_OVERHEAD_BYTES,
        ))
}

async fn upload_exchange_chunk(
    State(state): State<Arc<crate::AppState>>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    if let Err(error) = authorize(&state, &headers) {
        return exchange_error_response(error);
    }
    if state.disk_exchange_store.is_some() {
        // The store writes the chunk to disk: off the async runtime, or a
        // burst of uploads starves every other request — the health probe
        // included, which is how a coordinator under exchange load was
        // killed as unresponsive.
        let state = Arc::clone(&state);
        let outcome = tokio::task::spawn_blocking(move || {
            let store = state
                .disk_exchange_store
                .as_ref()
                .expect("checked before spawning");
            ExchangeChunk::decode(&body, ExchangeLimits::default())
                .map_err(ExchangeReceiveError::Protocol)
                .and_then(|chunk| store.insert(chunk).map_err(ExchangeReceiveError::Store))
        })
        .await;
        return match outcome {
            Ok(Ok(())) => StatusCode::ACCEPTED.into_response(),
            Ok(Err(ExchangeReceiveError::Store(error))) => {
                (StatusCode::INSUFFICIENT_STORAGE, error).into_response()
            }
            Ok(Err(ExchangeReceiveError::Protocol(error))) => exchange_error_response(error),
            Err(error) => (
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("exchange store task failed: {error}"),
            )
                .into_response(),
        };
    }
    match ExchangeChunk::decode(&body, ExchangeLimits::default())
        .and_then(|chunk| state.exchange_store.insert(chunk))
    {
        Ok(()) => StatusCode::ACCEPTED.into_response(),
        Err(error) => exchange_error_response(error),
    }
}

enum ExchangeReceiveError {
    Protocol(ExchangeError),
    Store(String),
}

async fn download_exchange(
    State(state): State<Arc<crate::AppState>>,
    headers: HeaderMap,
    Path(path): Path<ExchangePath>,
) -> Response {
    if let Err(error) = authorize(&state, &headers) {
        return exchange_error_response(error);
    }
    let identity = identity_from_path(path);
    if let Some(store) = &state.disk_exchange_store {
        return match store.body(&identity) {
            Ok(Some(body)) => ([(header::CONTENT_TYPE, EXCHANGE_MEDIA_TYPE)], body).into_response(),
            Ok(None) => StatusCode::NOT_FOUND.into_response(),
            Err(error) => (StatusCode::INTERNAL_SERVER_ERROR, error).into_response(),
        };
    }
    let result = state
        .exchange_store
        .get(&identity)
        .and_then(|chunks| ordered_chunks(chunks, ExchangeLimits::default()));
    match result {
        Ok(chunks) => {
            let bytes = chunks.iter().map(|chunk| chunk.payload.len()).sum();
            let permit = match DownloadPermit::new(bytes) {
                Ok(permit) => permit,
                Err(error) => return exchange_error_response(error),
            };
            let stream = futures::stream::unfold(
                (chunks.into_iter(), permit),
                |(mut chunks, permit)| async move {
                    chunks
                        .next()
                        .map(|chunk| (Ok::<_, std::io::Error>(chunk.payload), (chunks, permit)))
                },
            );
            (
                [(header::CONTENT_TYPE, EXCHANGE_MEDIA_TYPE)],
                axum::body::Body::from_stream(stream),
            )
                .into_response()
        }
        Err(error) => exchange_error_response(error),
    }
}

async fn delete_exchange(
    State(state): State<Arc<crate::AppState>>,
    headers: HeaderMap,
    Path(path): Path<ExchangePath>,
) -> Response {
    if let Err(error) = authorize(&state, &headers) {
        return exchange_error_response(error);
    }
    let identity = identity_from_path(path);
    if let Some(store) = &state.disk_exchange_store {
        return match store.remove(&identity) {
            Ok(true) => StatusCode::NO_CONTENT.into_response(),
            Ok(false) => StatusCode::NOT_FOUND.into_response(),
            Err(error) => (StatusCode::INTERNAL_SERVER_ERROR, error).into_response(),
        };
    }
    match state.exchange_store.remove(&identity) {
        Ok(true) => StatusCode::NO_CONTENT.into_response(),
        Ok(false) => StatusCode::NOT_FOUND.into_response(),
        Err(error) => exchange_error_response(error),
    }
}

fn authorize(state: &crate::AppState, headers: &HeaderMap) -> ExchangeResult<()> {
    let token = state
        .config
        .exchange_token
        .as_deref()
        .ok_or(ExchangeError::EmptyExpectedToken)?;
    let authorization = headers
        .get(header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok());
    validate_bearer_header(authorization, token)
}

fn identity_from_path(path: ExchangePath) -> ExchangeIdentity {
    ExchangeIdentity {
        exchange_id: ExchangeId(path.1),
        task_id: TaskId {
            query_id: path.0,
            stage_id: kaveon_core::StageId(path.2),
            partition: path.3,
            attempt: path.4,
        },
        output_partition: path.5,
    }
}

fn exchange_url(base_uri: &str, identity: &ExchangeIdentity) -> String {
    format!(
        "{}/v1/internal/exchange/{}/{}/{}/{}/{}/{}",
        base_uri.trim_end_matches('/'),
        identity.task_id.query_id,
        identity.exchange_id.0,
        identity.task_id.stage_id.0,
        identity.task_id.partition,
        identity.task_id.attempt,
        identity.output_partition
    )
}

pub async fn upload_chunks(
    client: &reqwest::Client,
    worker_uri: &str,
    token: &str,
    chunks: &[ExchangeChunk],
    limits: ExchangeLimits,
) -> ExchangeResult<()> {
    use futures::StreamExt;
    // The receiver stores chunks by index, so several may be in flight: one
    // 4 MiB request at a time left a 340 MB partition at ~60 MB/s.
    const IN_FLIGHT: usize = 4;
    let url = format!("{}/v1/internal/exchange", worker_uri.trim_end_matches('/'));
    let mut pending = Vec::with_capacity(chunks.len());
    for chunk in chunks {
        let client = client.clone();
        let url = url.clone();
        let token = token.to_owned();
        pending.push(async move {
            // Each chunk encodes when its upload starts, so at most
            // IN_FLIGHT copies exist at once.
            let body = chunk.encode(limits)?;
            let response = client
                .post(url)
                .bearer_auth(token)
                .header(header::CONTENT_TYPE.as_str(), EXCHANGE_MEDIA_TYPE)
                .body(body)
                .send()
                .await
                .map_err(|error| ExchangeError::Transport(error.to_string()))?;
            if !response.status().is_success() {
                return Err(ExchangeError::HttpStatus(response.status().as_u16()));
            }
            Ok::<(), ExchangeError>(())
        });
    }
    let mut uploads = futures::stream::iter(pending).buffer_unordered(IN_FLIGHT);
    while let Some(result) = uploads.next().await {
        result?;
    }
    Ok(())
}

pub async fn fetch_payload(
    client: &reqwest::Client,
    worker_uri: &str,
    token: &str,
    identity: &ExchangeIdentity,
) -> ExchangeResult<crate::transport::ArrowPayload> {
    let response = client
        .get(exchange_url(worker_uri, identity))
        .bearer_auth(token)
        .send()
        .await
        .map_err(|error| ExchangeError::Transport(error.to_string()))?;
    if !response.status().is_success() {
        return Err(ExchangeError::HttpStatus(response.status().as_u16()));
    }
    let payload = crate::transport::receive(response)
        .await
        .map_err(ExchangeError::Transport)?;
    Ok(payload)
}

pub async fn release_exchange(
    client: &reqwest::Client,
    worker_uri: &str,
    token: &str,
    identity: &ExchangeIdentity,
) -> ExchangeResult<()> {
    let response = client
        .delete(exchange_url(worker_uri, identity))
        .bearer_auth(token)
        .send()
        .await
        .map_err(|error| ExchangeError::Transport(error.to_string()))?;
    if !response.status().is_success() {
        return Err(ExchangeError::HttpStatus(response.status().as_u16()));
    }
    Ok(())
}

fn exchange_error_response(error: ExchangeError) -> Response {
    let status = match error {
        ExchangeError::Unauthorized => StatusCode::UNAUTHORIZED,
        ExchangeError::EmptyExpectedToken => StatusCode::SERVICE_UNAVAILABLE,
        ExchangeError::ExchangeNotFound => StatusCode::NOT_FOUND,
        ExchangeError::MissingChunks => StatusCode::CONFLICT,
        ExchangeError::StoreCapacityExceeded => StatusCode::INSUFFICIENT_STORAGE,
        _ => StatusCode::BAD_REQUEST,
    };
    (status, error.to_string()).into_response()
}

impl ExchangeIdentity {
    fn validate(&self) -> ExchangeResult<()> {
        validate_route_identifier(
            &self.task_id.query_id,
            "query ID is empty",
            "query ID contains unsupported URL characters",
        )?;
        validate_route_identifier(
            &self.exchange_id.0,
            "exchange ID is empty",
            "exchange ID contains unsupported URL characters",
        )?;
        Ok(())
    }
}

fn validate_route_identifier(
    value: &str,
    empty_reason: &'static str,
    unsupported_reason: &'static str,
) -> ExchangeResult<()> {
    if value.is_empty() {
        return Err(ExchangeError::InvalidIdentity(empty_reason));
    }
    if !value
        .bytes()
        .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'))
    {
        return Err(ExchangeError::InvalidIdentity(unsupported_reason));
    }
    Ok(())
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExchangeChunk {
    pub identity: ExchangeIdentity,
    pub chunk_index: usize,
    pub chunk_count: usize,
    pub payload: Bytes,
}

impl ExchangeChunk {
    pub fn encode(&self, limits: ExchangeLimits) -> ExchangeResult<Vec<u8>> {
        self.validate(limits)?;
        let query = self.identity.task_id.query_id.as_bytes();
        let exchange = self.identity.exchange_id.0.as_bytes();
        let query_len = u32::try_from(query.len()).map_err(|_| ExchangeError::IntegerOverflow)?;
        let exchange_len =
            u32::try_from(exchange.len()).map_err(|_| ExchangeError::IntegerOverflow)?;
        let source_partition = u64::try_from(self.identity.task_id.partition)
            .map_err(|_| ExchangeError::IntegerOverflow)?;
        let output_partition = u64::try_from(self.identity.output_partition)
            .map_err(|_| ExchangeError::IntegerOverflow)?;
        let chunk_index =
            u32::try_from(self.chunk_index).map_err(|_| ExchangeError::IntegerOverflow)?;
        let chunk_count =
            u32::try_from(self.chunk_count).map_err(|_| ExchangeError::IntegerOverflow)?;
        let payload_len =
            u64::try_from(self.payload.len()).map_err(|_| ExchangeError::IntegerOverflow)?;
        let capacity = FIXED_HEADER_BYTES
            .checked_add(query.len())
            .and_then(|size| size.checked_add(exchange.len()))
            .and_then(|size| size.checked_add(self.payload.len()))
            .ok_or(ExchangeError::IntegerOverflow)?;
        let mut wire = Vec::with_capacity(capacity);
        wire.extend_from_slice(&WIRE_MAGIC);
        wire.extend_from_slice(&WIRE_VERSION.to_be_bytes());
        wire.extend_from_slice(&query_len.to_be_bytes());
        wire.extend_from_slice(&exchange_len.to_be_bytes());
        wire.extend_from_slice(&self.identity.task_id.stage_id.0.to_be_bytes());
        wire.extend_from_slice(&source_partition.to_be_bytes());
        wire.extend_from_slice(&self.identity.task_id.attempt.to_be_bytes());
        wire.extend_from_slice(&output_partition.to_be_bytes());
        wire.extend_from_slice(&chunk_index.to_be_bytes());
        wire.extend_from_slice(&chunk_count.to_be_bytes());
        wire.extend_from_slice(&payload_len.to_be_bytes());
        wire.extend_from_slice(&0_u64.to_be_bytes());
        wire.extend_from_slice(query);
        wire.extend_from_slice(exchange);
        wire.extend_from_slice(&self.payload);
        let checksum = checksum(&wire[..CHECKSUM_OFFSET], &wire[FIXED_HEADER_BYTES..]);
        wire[CHECKSUM_OFFSET..FIXED_HEADER_BYTES].copy_from_slice(&checksum.to_be_bytes());
        Ok(wire)
    }

    pub fn decode(wire: &[u8], limits: ExchangeLimits) -> ExchangeResult<Self> {
        limits.validate()?;
        if wire.len() < FIXED_HEADER_BYTES {
            return Err(ExchangeError::TruncatedEnvelope);
        }
        if wire[..WIRE_MAGIC.len()] != WIRE_MAGIC {
            return Err(ExchangeError::InvalidMagic);
        }
        let mut offset = WIRE_MAGIC.len();
        let version = read_u16(wire, &mut offset)?;
        if version != WIRE_VERSION {
            return Err(ExchangeError::UnsupportedVersion(version));
        }
        let query_len = usize::try_from(read_u32(wire, &mut offset)?)
            .map_err(|_| ExchangeError::IntegerOverflow)?;
        let exchange_len = usize::try_from(read_u32(wire, &mut offset)?)
            .map_err(|_| ExchangeError::IntegerOverflow)?;
        let stage_id = read_u32(wire, &mut offset)?;
        let source_partition = usize::try_from(read_u64(wire, &mut offset)?)
            .map_err(|_| ExchangeError::IntegerOverflow)?;
        let attempt = read_u32(wire, &mut offset)?;
        let output_partition = usize::try_from(read_u64(wire, &mut offset)?)
            .map_err(|_| ExchangeError::IntegerOverflow)?;
        let chunk_index = usize::try_from(read_u32(wire, &mut offset)?)
            .map_err(|_| ExchangeError::IntegerOverflow)?;
        let chunk_count = usize::try_from(read_u32(wire, &mut offset)?)
            .map_err(|_| ExchangeError::IntegerOverflow)?;
        let payload_len = usize::try_from(read_u64(wire, &mut offset)?)
            .map_err(|_| ExchangeError::IntegerOverflow)?;
        let encoded_checksum = read_u64(wire, &mut offset)?;
        let expected_len = FIXED_HEADER_BYTES
            .checked_add(query_len)
            .and_then(|size| size.checked_add(exchange_len))
            .and_then(|size| size.checked_add(payload_len))
            .ok_or(ExchangeError::IntegerOverflow)?;
        if wire.len() != expected_len {
            return Err(ExchangeError::LengthMismatch);
        }
        let actual_checksum = checksum(&wire[..CHECKSUM_OFFSET], &wire[FIXED_HEADER_BYTES..]);
        if encoded_checksum != actual_checksum {
            return Err(ExchangeError::ChecksumMismatch);
        }
        let query_end = FIXED_HEADER_BYTES + query_len;
        let query_id = std::str::from_utf8(&wire[FIXED_HEADER_BYTES..query_end])
            .map_err(|_| ExchangeError::InvalidQueryId)?
            .to_owned();
        let exchange_end = query_end
            .checked_add(exchange_len)
            .ok_or(ExchangeError::IntegerOverflow)?;
        let exchange_id = std::str::from_utf8(&wire[query_end..exchange_end])
            .map_err(|_| ExchangeError::InvalidExchangeId)?
            .to_owned();
        let chunk = Self {
            identity: ExchangeIdentity {
                exchange_id: ExchangeId(exchange_id),
                task_id: TaskId {
                    query_id,
                    stage_id: kaveon_core::StageId(stage_id),
                    partition: source_partition,
                    attempt,
                },
                output_partition,
            },
            chunk_index,
            chunk_count,
            payload: Bytes::copy_from_slice(&wire[exchange_end..]),
        };
        chunk.validate(limits)?;
        Ok(chunk)
    }

    fn validate(&self, limits: ExchangeLimits) -> ExchangeResult<()> {
        limits.validate()?;
        self.identity.validate()?;
        // A streamed output does not know its chunk count until it ends:
        // its chunks carry 0 (open) and the last one the final count.
        if self.chunk_count > limits.max_chunks {
            return Err(ExchangeError::InvalidChunkCount);
        }
        let bound = if self.chunk_count == 0 {
            limits.max_chunks
        } else {
            self.chunk_count
        };
        if self.chunk_index >= bound {
            return Err(ExchangeError::InvalidChunkIndex);
        }
        if self.payload.len() > limits.max_chunk_bytes {
            return Err(ExchangeError::ChunkTooLarge);
        }
        Ok(())
    }
}

/// One output partition's payload written as it is produced: batches go
/// into an Arrow IPC stream, and every `max_chunk_bytes` of it leaves as a
/// chunk through `chunks` while the producer keeps running. Nothing of the
/// partition's output stays behind but the bytes of the chunk being
/// filled; the last chunk carries the final count.
pub struct StreamingOutput {
    identity: ExchangeIdentity,
    writer: Option<arrow::ipc::writer::StreamWriter<ChunkBuffer>>,
    chunks: tokio::sync::mpsc::Sender<ExchangeChunk>,
    limits: ExchangeLimits,
    next_index: usize,
    bytes: u64,
}

/// The IPC writer's sink: bytes accumulate here between chunk cuts.
#[derive(Default)]
struct ChunkBuffer(Vec<u8>);

impl std::io::Write for ChunkBuffer {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.0.extend_from_slice(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl StreamingOutput {
    pub fn new(
        identity: ExchangeIdentity,
        schema: &SchemaRef,
        chunks: tokio::sync::mpsc::Sender<ExchangeChunk>,
        limits: ExchangeLimits,
    ) -> ExchangeResult<Self> {
        limits.validate()?;
        identity.validate()?;
        let writer = arrow::ipc::writer::StreamWriter::try_new(ChunkBuffer::default(), schema)
            .map_err(|error| ExchangeError::Arrow(error.to_string()))?;
        Ok(Self {
            identity,
            writer: Some(writer),
            chunks,
            limits,
            next_index: 0,
            bytes: 0,
        })
    }

    pub fn write(&mut self, batch: &RecordBatch) -> ExchangeResult<()> {
        let writer = self.writer.as_mut().ok_or(ExchangeError::MissingChunks)?;
        writer
            .write(batch)
            .map_err(|error| ExchangeError::Arrow(error.to_string()))?;
        self.cut(false)
    }

    /// Emit every full chunk the buffer holds; with `all`, the remainder
    /// too, as the final chunk carrying the count.
    fn cut(&mut self, all: bool) -> ExchangeResult<()> {
        let writer = self.writer.as_mut().ok_or(ExchangeError::MissingChunks)?;
        let buffer = &mut writer.get_mut().0;
        let size = self.limits.max_chunk_bytes;
        while buffer.len() >= size || (all && !buffer.is_empty()) {
            let take = buffer.len().min(size);
            let payload: Vec<u8> = buffer.drain(..take).collect();
            let last = all && buffer.is_empty();
            let chunk = ExchangeChunk {
                identity: self.identity.clone(),
                chunk_index: self.next_index,
                chunk_count: if last { self.next_index + 1 } else { 0 },
                payload: Bytes::from(payload),
            };
            chunk.validate(self.limits)?;
            self.bytes = self.bytes.saturating_add(chunk.payload.len() as u64);
            self.next_index += 1;
            // From the executing (blocking) thread: waits while the lane is
            // full, which is the back-pressure.
            self.chunks
                .blocking_send(chunk)
                .map_err(|_| ExchangeError::Transport("exchange uploader stopped".into()))?;
        }
        Ok(())
    }

    /// Close the stream; the remaining bytes leave as the last chunk. An
    /// output with no batches still sends its schema and end marker.
    pub fn finish(mut self) -> ExchangeResult<u64> {
        let mut writer = self.writer.take().ok_or(ExchangeError::MissingChunks)?;
        writer
            .finish()
            .map_err(|error| ExchangeError::Arrow(error.to_string()))?;
        self.writer = Some(writer);
        self.cut(true)?;
        Ok(self.bytes)
    }
}

/// Upload chunks as they arrive on `chunks`, a few in flight, until the
/// sender closes. Returns the bytes uploaded.
pub async fn upload_chunk_stream(
    client: reqwest::Client,
    worker_uri: String,
    token: String,
    mut chunks: tokio::sync::mpsc::Receiver<ExchangeChunk>,
    limits: ExchangeLimits,
) -> ExchangeResult<u64> {
    use futures::StreamExt;
    use futures::stream::FuturesUnordered;
    const IN_FLIGHT: usize = 4;
    let url = format!("{}/v1/internal/exchange", worker_uri.trim_end_matches('/'));
    let upload = |chunk: ExchangeChunk| {
        let client = client.clone();
        let url = url.clone();
        let token = token.clone();
        async move {
            let bytes = chunk.payload.len() as u64;
            let body = chunk.encode(limits)?;
            let response = client
                .post(url)
                .bearer_auth(token)
                .header(header::CONTENT_TYPE.as_str(), EXCHANGE_MEDIA_TYPE)
                .body(body)
                .send()
                .await
                .map_err(|error| ExchangeError::Transport(error.to_string()))?;
            if !response.status().is_success() {
                return Err(ExchangeError::HttpStatus(response.status().as_u16()));
            }
            Ok::<u64, ExchangeError>(bytes)
        }
    };
    let mut in_flight = FuturesUnordered::new();
    let mut total = 0u64;
    loop {
        if in_flight.len() >= IN_FLIGHT {
            let Some(result) = in_flight.next().await else {
                break;
            };
            total = total.saturating_add(result?);
            continue;
        }
        match chunks.recv().await {
            Some(chunk) => in_flight.push(upload(chunk)),
            None => break,
        }
    }
    while let Some(result) = in_flight.next().await {
        total = total.saturating_add(result?);
    }
    Ok(total)
}

pub fn encode_batches(
    identity: ExchangeIdentity,
    schema: &SchemaRef,
    batches: &[RecordBatch],
    limits: ExchangeLimits,
) -> ExchangeResult<Vec<ExchangeChunk>> {
    limits.validate()?;
    identity.validate()?;
    let mut payload = crate::transport::BoundedBuffer::new(limits.max_payload_bytes);
    {
        let mut writer = StreamWriter::try_new(&mut payload, schema)
            .map_err(|error| ExchangeError::Arrow(error.to_string()))?;
        for batch in batches {
            writer
                .write(batch)
                .map_err(|error| ExchangeError::Arrow(error.to_string()))?;
        }
        writer
            .finish()
            .map_err(|error| ExchangeError::Arrow(error.to_string()))?;
    }
    let payload = Bytes::from(payload.into_bytes());
    if payload.len() > limits.max_payload_bytes {
        return Err(ExchangeError::PayloadTooLarge);
    }
    let chunk_count = payload.len().div_ceil(limits.max_chunk_bytes).max(1);
    if chunk_count > limits.max_chunks {
        return Err(ExchangeError::InvalidChunkCount);
    }
    Ok((0..chunk_count)
        .map(|chunk_index| ExchangeChunk {
            identity: identity.clone(),
            chunk_index,
            chunk_count,
            payload: payload.slice(
                chunk_index * limits.max_chunk_bytes
                    ..((chunk_index + 1) * limits.max_chunk_bytes).min(payload.len()),
            ),
        })
        .collect())
}

pub fn decode_batches(
    chunks: Vec<ExchangeChunk>,
    limits: ExchangeLimits,
) -> ExchangeResult<(ExchangeIdentity, SchemaRef, Vec<RecordBatch>)> {
    let (identity, payload) = assemble_chunks(chunks, limits)?;
    let reader = StreamReader::try_new(Cursor::new(payload), None)
        .map_err(|error| ExchangeError::Arrow(error.to_string()))?;
    let schema = reader.schema();
    let batches = reader
        .map(|batch| batch.map_err(|error| ExchangeError::Arrow(error.to_string())))
        .collect::<ExchangeResult<Vec<_>>>()?;
    Ok((identity, schema, batches))
}

fn ordered_chunks(
    mut chunks: Vec<ExchangeChunk>,
    limits: ExchangeLimits,
) -> ExchangeResult<Vec<ExchangeChunk>> {
    limits.validate()?;
    let first = chunks.first().ok_or(ExchangeError::MissingChunks)?;
    first.validate(limits)?;
    let identity = first.identity.clone();
    // The count comes from the chunk that carries it (the last of a
    // streamed output, every chunk of an encoded one); open chunks agree
    // with whatever it is.
    let chunk_count = chunks
        .iter()
        .map(|chunk| chunk.chunk_count)
        .max()
        .unwrap_or_default();
    if chunk_count == 0 || chunks.len() != chunk_count {
        return Err(ExchangeError::MissingChunks);
    }
    for chunk in &chunks {
        chunk.validate(limits)?;
        if chunk.identity != identity
            || (chunk.chunk_count != 0 && chunk.chunk_count != chunk_count)
        {
            return Err(ExchangeError::MixedChunkSet);
        }
    }
    chunks.sort_unstable_by_key(|chunk| chunk.chunk_index);
    let mut bytes = 0usize;
    for (index, chunk) in chunks.iter().enumerate() {
        if chunk.chunk_index != index {
            return Err(ExchangeError::DuplicateChunk);
        }
        bytes = bytes
            .checked_add(chunk.payload.len())
            .ok_or(ExchangeError::IntegerOverflow)?;
        if bytes > limits.max_payload_bytes {
            return Err(ExchangeError::PayloadTooLarge);
        }
    }
    Ok(chunks)
}

pub fn assemble_chunks(
    chunks: Vec<ExchangeChunk>,
    limits: ExchangeLimits,
) -> ExchangeResult<(ExchangeIdentity, Vec<u8>)> {
    let chunks = ordered_chunks(chunks, limits)?;
    let identity = chunks[0].identity.clone();
    let payload = chunks.into_iter().flat_map(|chunk| chunk.payload).collect();
    Ok((identity, payload))
}

pub fn validate_bearer_header(header: Option<&str>, expected_token: &str) -> ExchangeResult<()> {
    if expected_token.is_empty() {
        return Err(ExchangeError::EmptyExpectedToken);
    }
    let supplied = header
        .and_then(|value| value.strip_prefix(BEARER_PREFIX))
        .ok_or(ExchangeError::Unauthorized)?;
    if !constant_time_equal(supplied.as_bytes(), expected_token.as_bytes()) {
        return Err(ExchangeError::Unauthorized);
    }
    Ok(())
}

fn constant_time_equal(left: &[u8], right: &[u8]) -> bool {
    let mut difference = left.len() ^ right.len();
    let comparison_len = left.len().max(right.len());
    for index in 0..comparison_len {
        difference |= usize::from(
            left.get(index).copied().unwrap_or_default()
                ^ right.get(index).copied().unwrap_or_default(),
        );
    }
    difference == 0
}

fn checksum(header: &[u8], body: &[u8]) -> u64 {
    header
        .iter()
        .chain(body)
        .fold(FNV_OFFSET_BASIS, |hash, byte| {
            (hash ^ u64::from(*byte)).wrapping_mul(FNV_PRIME)
        })
}

fn read_bytes<'a>(wire: &'a [u8], offset: &mut usize, count: usize) -> ExchangeResult<&'a [u8]> {
    let end = offset
        .checked_add(count)
        .ok_or(ExchangeError::IntegerOverflow)?;
    let bytes = wire
        .get(*offset..end)
        .ok_or(ExchangeError::TruncatedEnvelope)?;
    *offset = end;
    Ok(bytes)
}

fn read_u16(wire: &[u8], offset: &mut usize) -> ExchangeResult<u16> {
    let bytes: [u8; 2] = read_bytes(wire, offset, 2)?
        .try_into()
        .map_err(|_| ExchangeError::TruncatedEnvelope)?;
    Ok(u16::from_be_bytes(bytes))
}

fn read_u32(wire: &[u8], offset: &mut usize) -> ExchangeResult<u32> {
    let bytes: [u8; 4] = read_bytes(wire, offset, 4)?
        .try_into()
        .map_err(|_| ExchangeError::TruncatedEnvelope)?;
    Ok(u32::from_be_bytes(bytes))
}

fn read_u64(wire: &[u8], offset: &mut usize) -> ExchangeResult<u64> {
    let bytes: [u8; 8] = read_bytes(wire, offset, 8)?
        .try_into()
        .map_err(|_| ExchangeError::TruncatedEnvelope)?;
    Ok(u64::from_be_bytes(bytes))
}

pub type ExchangeResult<T> = std::result::Result<T, ExchangeError>;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ExchangeError {
    InvalidLimits,
    InvalidIdentity(&'static str),
    InvalidMagic,
    UnsupportedVersion(u16),
    TruncatedEnvelope,
    LengthMismatch,
    ChecksumMismatch,
    InvalidQueryId,
    InvalidExchangeId,
    InvalidChunkCount,
    InvalidChunkIndex,
    ChunkTooLarge,
    PayloadTooLarge,
    MissingChunks,
    MixedChunkSet,
    DuplicateChunk,
    IntegerOverflow,
    Arrow(String),
    EmptyExpectedToken,
    Unauthorized,
    StorePoisoned,
    StoreCapacityExceeded,
    ConflictingChunk,
    ExchangeNotFound,
    Transport(String),
    HttpStatus(u16),
}

impl Display for ExchangeError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidLimits => write!(formatter, "exchange limits must be greater than zero"),
            Self::InvalidIdentity(reason) => {
                write!(formatter, "invalid exchange identity: {reason}")
            }
            Self::InvalidMagic => write!(formatter, "invalid exchange wire magic"),
            Self::UnsupportedVersion(version) => {
                write!(formatter, "unsupported exchange wire version {version}")
            }
            Self::TruncatedEnvelope => write!(formatter, "truncated exchange envelope"),
            Self::LengthMismatch => write!(formatter, "exchange envelope length mismatch"),
            Self::ChecksumMismatch => write!(formatter, "exchange envelope checksum mismatch"),
            Self::InvalidQueryId => write!(formatter, "exchange query ID is not valid UTF-8"),
            Self::InvalidExchangeId => {
                write!(formatter, "exchange ID is not valid UTF-8")
            }
            Self::InvalidChunkCount => write!(formatter, "invalid exchange chunk count"),
            Self::InvalidChunkIndex => write!(formatter, "invalid exchange chunk index"),
            Self::ChunkTooLarge => write!(formatter, "exchange chunk exceeds configured limit"),
            Self::PayloadTooLarge => write!(formatter, "exchange payload exceeds configured limit"),
            Self::MissingChunks => write!(formatter, "exchange chunk set is incomplete"),
            Self::MixedChunkSet => {
                write!(formatter, "exchange chunk set contains mixed identities")
            }
            Self::DuplicateChunk => {
                write!(formatter, "exchange chunk set contains duplicate indices")
            }
            Self::IntegerOverflow => write!(formatter, "exchange integer conversion overflow"),
            Self::Arrow(reason) => write!(formatter, "Arrow IPC exchange error: {reason}"),
            Self::EmptyExpectedToken => {
                write!(formatter, "exchange bearer token is not configured")
            }
            Self::Unauthorized => write!(formatter, "exchange bearer token is invalid"),
            Self::StorePoisoned => write!(formatter, "exchange store lock is poisoned"),
            Self::StoreCapacityExceeded => write!(formatter, "exchange store capacity exceeded"),
            Self::ConflictingChunk => {
                write!(formatter, "exchange chunk conflicts with stored data")
            }
            Self::ExchangeNotFound => write!(formatter, "exchange was not found"),
            Self::Transport(reason) => write!(formatter, "exchange transport error: {reason}"),
            Self::HttpStatus(status) => {
                write!(formatter, "exchange endpoint returned HTTP {status}")
            }
        }
    }
}

impl std::error::Error for ExchangeError {}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use arrow::array::{Int64Array, StringArray};
    use arrow::datatypes::{DataType, Field, Schema};

    use super::*;

    fn identity() -> ExchangeIdentity {
        ExchangeIdentity {
            exchange_id: ExchangeId("exchange-9".into()),
            task_id: TaskId {
                query_id: "query-42".into(),
                stage_id: kaveon_core::StageId(3),
                partition: 7,
                attempt: 2,
            },
            output_partition: 11,
        }
    }

    fn limits() -> ExchangeLimits {
        ExchangeLimits {
            max_payload_bytes: 16 * 1024,
            max_chunk_bytes: 128,
            max_chunks: 128,
        }
    }

    fn chunk_for(identity: ExchangeIdentity) -> ExchangeChunk {
        ExchangeChunk {
            identity,
            chunk_index: 0,
            chunk_count: 1,
            payload: Bytes::from_static(b"stale"),
        }
    }

    #[test]
    fn a_chunk_of_a_superseded_attempt_never_answers_the_current_attempt() {
        // A producer re-executed after a worker loss runs as the next
        // attempt; the attempt is part of the identity, so what the old
        // attempt uploads late lands under its own key and is never what
        // the consumer, fetching the new attempt, receives.
        let stale = identity();
        let mut current = identity();
        current.task_id.attempt = stale.task_id.attempt + 1;
        let store = ExchangeStore::default();
        store.insert(chunk_for(stale.clone())).unwrap();
        assert!(matches!(
            store.get(&current),
            Err(ExchangeError::ExchangeNotFound)
        ));
        assert_ne!(
            exchange_url("http://w", &stale),
            exchange_url("http://w", &current)
        );
        let root =
            std::env::temp_dir().join(format!("kaveon-stale-attempt-{}", uuid::Uuid::new_v4()));
        let disk = crate::disk_exchange::DiskExchangeStore::new(&root, 1 << 20).unwrap();
        disk.insert(chunk_for(stale.clone())).unwrap();
        assert!(disk.body(&stale).unwrap().is_some());
        assert!(disk.body(&current).unwrap().is_none());
        // A process that starts over the same spool root serves nothing of
        // what the process before it held: a returning worker's old spools
        // are not served.
        let returned = crate::disk_exchange::DiskExchangeStore::new(&root, 1 << 20).unwrap();
        assert!(returned.body(&stale).unwrap().is_none());
        drop(disk);
        drop(returned);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[tokio::test]
    async fn streamed_output_cuts_open_chunks_and_reassembles_to_the_same_batches() {
        // Batches written one at a time leave as 128-byte chunks with an
        // open count; the last chunk carries the count; the set decodes
        // to the batches that went in. An output with no batches is a
        // complete empty stream with the right schema.
        let schema = Arc::new(Schema::new(vec![Field::new("v", DataType::Int64, false)]));
        let batches = (0..5i64)
            .map(|k| {
                RecordBatch::try_new(
                    schema.clone(),
                    vec![Arc::new(Int64Array::from_iter_values(k * 40..(k + 1) * 40))],
                )
                .unwrap()
            })
            .collect::<Vec<_>>();
        let (sender, mut receiver) = tokio::sync::mpsc::channel(64);
        let written = tokio::task::spawn_blocking({
            let schema = schema.clone();
            let batches = batches.clone();
            move || {
                let mut output =
                    StreamingOutput::new(identity(), &schema, sender, limits()).unwrap();
                for batch in &batches {
                    output.write(batch).unwrap();
                }
                output.finish().unwrap()
            }
        })
        .await
        .unwrap();
        let mut chunks = Vec::new();
        while let Some(chunk) = receiver.recv().await {
            chunks.push(chunk);
        }
        assert!(chunks.len() > 3, "several chunks for five batches");
        assert_eq!(
            chunks
                .iter()
                .map(|chunk| chunk.payload.len() as u64)
                .sum::<u64>(),
            written
        );
        assert!(
            chunks[..chunks.len() - 1]
                .iter()
                .all(|chunk| chunk.chunk_count == 0)
        );
        assert_eq!(chunks.last().unwrap().chunk_count, chunks.len());
        assert!(
            chunks[..chunks.len() - 1]
                .iter()
                .all(|chunk| chunk.payload.len() == limits().max_chunk_bytes)
        );
        // Order of arrival does not matter to the receiver.
        chunks.reverse();
        let (decoded_identity, decoded_schema, decoded) =
            decode_batches(chunks.clone(), limits()).unwrap();
        assert_eq!(decoded_identity, identity());
        assert_eq!(decoded_schema, schema);
        assert_eq!(decoded, batches);
        // Without the counting chunk the set is incomplete, not decodable.
        let open_only = chunks
            .iter()
            .filter(|chunk| chunk.chunk_count == 0)
            .cloned()
            .collect::<Vec<_>>();
        assert_eq!(
            decode_batches(open_only, limits()).unwrap_err(),
            ExchangeError::MissingChunks
        );

        let (sender, mut receiver) = tokio::sync::mpsc::channel(4);
        tokio::task::spawn_blocking({
            let schema = schema.clone();
            move || {
                StreamingOutput::new(identity(), &schema, sender, limits())
                    .unwrap()
                    .finish()
                    .unwrap()
            }
        })
        .await
        .unwrap();
        let mut chunks = Vec::new();
        while let Some(chunk) = receiver.recv().await {
            chunks.push(chunk);
        }
        assert_eq!(chunks.last().unwrap().chunk_count, chunks.len());
        let (_, decoded_schema, decoded) = decode_batches(chunks, limits()).unwrap();
        assert_eq!(decoded_schema, schema);
        assert!(decoded.is_empty());
    }

    #[test]
    fn envelope_round_trip_is_deterministic() {
        let chunk = ExchangeChunk {
            identity: identity(),
            chunk_index: 1,
            chunk_count: 3,
            payload: vec![1, 2, 3, 4].into(),
        };
        let first = chunk.encode(limits()).unwrap();
        let second = chunk.encode(limits()).unwrap();
        assert_eq!(first, second);
        assert_eq!(ExchangeChunk::decode(&first, limits()).unwrap(), chunk);
    }

    #[test]
    fn envelope_rejects_the_previous_identity_version() {
        let chunk = ExchangeChunk {
            identity: identity(),
            chunk_index: 0,
            chunk_count: 1,
            payload: vec![1].into(),
        };
        let mut wire = chunk.encode(limits()).unwrap();
        wire[WIRE_MAGIC.len()..WIRE_MAGIC.len() + 2].copy_from_slice(&1_u16.to_be_bytes());
        assert_eq!(
            ExchangeChunk::decode(&wire, limits()),
            Err(ExchangeError::UnsupportedVersion(1))
        );
    }

    #[test]
    fn arrow_batches_survive_out_of_order_chunks() {
        let schema = Arc::new(Schema::new(vec![
            Field::new("key", DataType::Utf8, false),
            Field::new("value", DataType::Int64, false),
        ]));
        let batch = RecordBatch::try_new(
            Arc::clone(&schema),
            vec![
                Arc::new(StringArray::from(vec!["a", "b", "c"])),
                Arc::new(Int64Array::from(vec![10, 20, 30])),
            ],
        )
        .unwrap();
        let mut chunks =
            encode_batches(identity(), &schema, std::slice::from_ref(&batch), limits()).unwrap();
        chunks.reverse();
        let (decoded_identity, decoded_schema, decoded_batches) =
            decode_batches(chunks, limits()).unwrap();
        assert_eq!(decoded_identity, identity());
        assert_eq!(decoded_schema, schema);
        assert_eq!(decoded_batches, vec![batch]);
    }

    #[test]
    fn checksum_rejects_corruption() {
        let chunk = ExchangeChunk {
            identity: identity(),
            chunk_index: 0,
            chunk_count: 1,
            payload: vec![1, 2, 3].into(),
        };
        let mut wire = chunk.encode(limits()).unwrap();
        let last = wire.len() - 1;
        wire[last] ^= 1;
        assert_eq!(
            ExchangeChunk::decode(&wire, limits()),
            Err(ExchangeError::ChecksumMismatch)
        );
    }

    #[test]
    fn assembly_rejects_duplicate_and_mixed_chunks() {
        let first = ExchangeChunk {
            identity: identity(),
            chunk_index: 0,
            chunk_count: 2,
            payload: vec![1].into(),
        };
        assert_eq!(
            assemble_chunks(vec![first.clone(), first], limits()),
            Err(ExchangeError::DuplicateChunk)
        );
        let mut other = identity();
        other.task_id.attempt += 1;
        let second = ExchangeChunk {
            identity: other,
            chunk_index: 1,
            chunk_count: 2,
            payload: vec![2].into(),
        };
        let first = ExchangeChunk {
            identity: identity(),
            chunk_index: 0,
            chunk_count: 2,
            payload: vec![1].into(),
        };
        assert_eq!(
            assemble_chunks(vec![first, second], limits()),
            Err(ExchangeError::MixedChunkSet)
        );
    }

    #[test]
    fn configured_limits_are_enforced() {
        let chunk = ExchangeChunk {
            identity: identity(),
            chunk_index: 0,
            chunk_count: 1,
            payload: vec![0; limits().max_chunk_bytes + 1].into(),
        };
        assert_eq!(chunk.encode(limits()), Err(ExchangeError::ChunkTooLarge));
    }

    #[test]
    fn bearer_authentication_is_strict() {
        assert_eq!(
            validate_bearer_header(None, "secret"),
            Err(ExchangeError::Unauthorized)
        );
        assert_eq!(
            validate_bearer_header(Some("bearer secret"), "secret"),
            Err(ExchangeError::Unauthorized)
        );
        assert_eq!(
            validate_bearer_header(Some("Bearer wrong"), "secret"),
            Err(ExchangeError::Unauthorized)
        );
        validate_bearer_header(Some("Bearer secret"), "secret").unwrap();
    }

    #[test]
    fn store_accepts_idempotent_retries_and_rejects_conflicts() {
        let store = ExchangeStore::default();
        let chunk = ExchangeChunk {
            identity: identity(),
            chunk_index: 0,
            chunk_count: 1,
            payload: vec![1, 2, 3].into(),
        };

        store.insert(chunk.clone()).unwrap();
        store.insert(chunk.clone()).unwrap();
        assert_eq!(store.get(&identity()).unwrap(), vec![chunk.clone()]);

        let conflicting = ExchangeChunk {
            payload: vec![9].into(),
            ..chunk
        };
        assert_eq!(
            store.insert(conflicting),
            Err(ExchangeError::ConflictingChunk)
        );
        assert!(store.remove(&identity()).unwrap());
        assert_eq!(store.get(&identity()), Err(ExchangeError::ExchangeNotFound));
    }

    #[test]
    fn store_keeps_outputs_for_distinct_exchanges_separate() {
        let store = ExchangeStore::default();
        let first = ExchangeChunk {
            identity: identity(),
            chunk_index: 0,
            chunk_count: 1,
            payload: vec![1].into(),
        };
        let mut second = first.clone();
        second.identity.exchange_id = ExchangeId("exchange-10".into());
        second.payload = vec![2].into();

        store.insert(first.clone()).unwrap();
        store.insert(second.clone()).unwrap();

        assert_eq!(store.get(&first.identity).unwrap(), vec![first]);
        assert_eq!(store.get(&second.identity).unwrap(), vec![second]);
    }

    #[test]
    fn store_bounds_and_releases_buffered_payload_bytes() {
        let store = ExchangeStore::with_capacity(2, 4);
        let chunk = ExchangeChunk {
            identity: identity(),
            chunk_index: 0,
            chunk_count: 2,
            payload: vec![1, 2, 3].into(),
        };
        store.insert(chunk.clone()).unwrap();
        store.insert(chunk).unwrap();
        assert_eq!(store.buffered_bytes().unwrap(), 3);

        let second = ExchangeChunk {
            identity: identity(),
            chunk_index: 1,
            chunk_count: 2,
            payload: vec![4, 5].into(),
        };
        assert_eq!(
            store.insert(second),
            Err(ExchangeError::StoreCapacityExceeded)
        );
        assert_eq!(store.buffered_bytes().unwrap(), 3);
        assert!(store.remove(&identity()).unwrap());
        assert_eq!(store.buffered_bytes().unwrap(), 0);
    }

    #[test]
    fn failed_attempt_cleanup_releases_exchange_capacity_for_retry() {
        let store = ExchangeStore::with_capacity(1, 4);
        let first = ExchangeChunk {
            identity: identity(),
            chunk_index: 0,
            chunk_count: 2,
            payload: vec![1, 2, 3].into(),
        };
        store.insert(first.clone()).unwrap();

        // A failed attempt must not strand its partial exchange and block the
        // retry attempt from using the bounded store.
        let mut retry = first.clone();
        retry.identity.task_id.attempt += 1;
        assert_eq!(
            store.insert(retry.clone()),
            Err(ExchangeError::StoreCapacityExceeded)
        );
        assert!(store.remove(&first.identity).unwrap());
        assert_eq!(store.buffered_bytes().unwrap(), 0);
        let retry_identity = retry.identity.clone();
        store.insert(retry).unwrap();
        assert_eq!(store.buffered_bytes().unwrap(), 3);
        assert!(
            !store.remove(&first.identity).unwrap(),
            "old attempt must be absent"
        );
        assert!(store.remove(&retry_identity).unwrap());
        assert_eq!(store.buffered_bytes().unwrap(), 0);
    }

    #[test]
    fn identity_rejects_query_ids_that_cannot_be_used_in_routes() {
        let mut invalid = identity();
        invalid.task_id.query_id = "query/42".into();
        let chunk = ExchangeChunk {
            identity: invalid,
            chunk_index: 0,
            chunk_count: 1,
            payload: Vec::new().into(),
        };
        assert!(matches!(
            chunk.encode(limits()),
            Err(ExchangeError::InvalidIdentity(_))
        ));
    }

    #[test]
    fn identity_rejects_exchange_ids_that_cannot_be_used_in_routes() {
        let mut invalid = identity();
        invalid.exchange_id = ExchangeId("exchange/9".into());
        let chunk = ExchangeChunk {
            identity: invalid,
            chunk_index: 0,
            chunk_count: 1,
            payload: Vec::new().into(),
        };
        assert!(matches!(
            chunk.encode(limits()),
            Err(ExchangeError::InvalidIdentity(_))
        ));
    }

    #[test]
    fn exchange_url_contains_the_exchange_id() {
        assert_eq!(
            exchange_url("http://worker:8080/", &identity()),
            "http://worker:8080/v1/internal/exchange/query-42/exchange-9/3/7/2/11"
        );
    }
}
