//! Receive Arrow IPC incrementally into a bounded private disk spool, or
//! decode it batch by batch as it arrives (`receive_stream`).
//! The immutable spool permits decoding without retaining an encoded body copy.
use arrow::{datatypes::SchemaRef, ipc::reader::StreamReader, record_batch::RecordBatch};
use axum::body::Bytes;
use futures::StreamExt;
use std::{
    fs::{File, OpenOptions},
    io::{BufReader, Read, Write},
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
};

/// The most one exchange partition or task result may carry, matching the
/// encoder's ceiling so a payload a worker can produce is one its peer can
/// receive.
/// One exchange payload the consumer spools to its disk: the producer's
/// per-partition ceiling.
pub const MAX_PAYLOAD_BYTES: u64 = 8 * 1024 * 1024 * 1024;
/// Received payloads spool to disk; this is the process-wide ceiling on what
/// is spooled at once — a final task holds one payload per producing task.
/// Every spooled payload a worker holds at once, across queries: the
/// worker's ephemeral disk is what backs it.
const PROCESS_SPOOL_BYTES: u64 = 12 * 1024 * 1024 * 1024;
static RETAINED_BYTES: AtomicU64 = AtomicU64::new(0);
static CACHED_BYTES: AtomicU64 = AtomicU64::new(0);
pub struct CachedTaskResult {
    pub bytes: Vec<u8>,
    pub elapsed_us: u64,
    pub scan_metrics_header: Option<String>,
    pub execution_metrics_header: Option<String>,
    /// The task streamed its rows to the coordinator as it ran: `bytes` is
    /// empty and only the metrics are retained.
    pub streamed: bool,
}
impl CachedTaskResult {
    pub fn new(
        bytes: Vec<u8>,
        elapsed_us: u64,
        scan_metrics_header: Option<String>,
        execution_metrics_header: Option<String>,
    ) -> Result<Self, String> {
        CACHED_BYTES
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |used| {
                used.checked_add(bytes.capacity() as u64)
                    .filter(|total| *total <= 512 * 1024 * 1024)
            })
            .map_err(|_| "worker task result cache quota exceeded")?;
        Ok(Self {
            bytes,
            elapsed_us,
            scan_metrics_header,
            execution_metrics_header,
            streamed: false,
        })
    }
    /// The record of a streamed root task: its metrics, no bytes.
    pub fn streamed(
        elapsed_us: u64,
        scan_metrics_header: Option<String>,
        execution_metrics_header: Option<String>,
    ) -> Self {
        Self {
            bytes: Vec::new(),
            elapsed_us,
            scan_metrics_header,
            execution_metrics_header,
            streamed: true,
        }
    }
}
impl Drop for CachedTaskResult {
    fn drop(&mut self) {
        CACHED_BYTES.fetch_sub(self.bytes.capacity() as u64, Ordering::AcqRel);
    }
}

pub struct BoundedBuffer {
    bytes: Vec<u8>,
    limit: usize,
}
impl BoundedBuffer {
    pub fn new(limit: usize) -> Self {
        Self {
            bytes: Vec::new(),
            limit,
        }
    }
    pub fn into_bytes(self) -> Vec<u8> {
        self.bytes
    }
}
impl Write for BoundedBuffer {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        if self.bytes.len().saturating_add(bytes.len()) > self.limit {
            return Err(std::io::Error::other(
                "Arrow IPC encode byte limit exceeded",
            ));
        }
        if self.bytes.capacity() < self.bytes.len() + bytes.len() {
            self.bytes.reserve_exact(bytes.len());
        }
        self.bytes.extend_from_slice(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}
struct DiskSpool {
    path: PathBuf,
    bytes: u64,
}
impl Drop for DiskSpool {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
        RETAINED_BYTES.fetch_sub(self.bytes, Ordering::AcqRel);
    }
}
pub struct ArrowPayload {
    // Field order closes the reader before deleting the file on Windows.
    reader: StreamReader<BufReader<File>>,
    spool: std::sync::Arc<DiskSpool>,
}
impl ArrowPayload {
    pub fn schema(&self) -> SchemaRef {
        self.reader.schema()
    }
    pub fn fork(&self) -> Result<Self, String> {
        let reader = StreamReader::try_new(
            BufReader::new(File::open(&self.spool.path).map_err(|error| error.to_string())?),
            None,
        )
        .map_err(|error| error.to_string())?;
        Ok(Self {
            reader,
            spool: self.spool.clone(),
        })
    }
    pub fn bytes(&self) -> usize {
        self.spool.bytes as usize
    }
    pub fn next_batch(&mut self) -> Result<Option<RecordBatch>, String> {
        self.reader
            .next()
            .transpose()
            .map_err(|error| format!("invalid Arrow stream: {error}"))
    }
    pub fn collect(mut self) -> Result<(SchemaRef, Vec<RecordBatch>), String> {
        let schema = self.schema();
        let mut batches = Vec::new();
        let mut decoded_bytes = 0usize;
        while let Some(batch) = self.next_batch()? {
            decoded_bytes = decoded_bytes.saturating_add(batch.get_array_memory_size());
            if decoded_bytes > MAX_PAYLOAD_BYTES as usize {
                return Err("decoded Arrow payload exceeds 1 GiB limit".into());
            }
            batches.push(batch);
        }
        Ok((schema, batches))
    }
}

pub async fn receive(response: reqwest::Response) -> Result<ArrowPayload, String> {
    receive_with_limit(response, MAX_PAYLOAD_BYTES).await
}
/// Where received payloads spool: `KAVEON_IPC_SPOOL_ROOT`, else the
/// system temporary directory. The directory must exist.
fn ipc_spool_root() -> std::path::PathBuf {
    std::env::var_os("KAVEON_IPC_SPOOL_ROOT")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(std::env::temp_dir)
}

async fn receive_with_limit(
    mut response: reqwest::Response,
    limit: u64,
) -> Result<ArrowPayload, String> {
    if response.content_length().is_some_and(|bytes| bytes > limit) {
        return Err("Arrow payload Content-Length exceeds receive limit".into());
    }
    let path = ipc_spool_root().join(format!("kaveon-ipc-{}.arrow", uuid::Uuid::new_v4()));
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut spool = DiskSpool { path, bytes: 0 };
    let mut output = options
        .open(&spool.path)
        .map_err(|error| error.to_string())?;
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|error| format!("network receive: {error}"))?
    {
        let bytes = chunk.len() as u64;
        if spool.bytes + bytes > limit {
            return Err("Arrow payload exceeds receive limit".into());
        }
        RETAINED_BYTES
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |used| {
                used.checked_add(bytes)
                    .filter(|total| *total <= PROCESS_SPOOL_BYTES)
            })
            .map_err(|_| "process IPC disk spool quota exceeded")?;
        spool.bytes += bytes;
        output
            .write_all(&chunk)
            .map_err(|error| error.to_string())?;
    }
    output.flush().map_err(|error| error.to_string())?;
    drop(output);
    let reader = StreamReader::try_new(
        BufReader::new(File::open(&spool.path).map_err(|error| error.to_string())?),
        None,
    )
    .map_err(|error| error.to_string())?;
    Ok(ArrowPayload {
        reader,
        spool: std::sync::Arc::new(spool),
    })
}

/// Encoded bytes of one streamed task result held between the network and
/// the decoder. The pump stops reading the body once this much is waiting,
/// so a slow consumer holds the worker back through TCP instead of making
/// the coordinator buffer the task's output.
const STREAM_IN_FLIGHT_BYTES: usize = 8 * 1024 * 1024;
/// Decoded batches held between the decoder and the consumer.
const STREAM_DECODED_BATCHES: usize = 2;

/// Process spool quota held by bytes in flight; released when consumed.
struct RetainedBytes(u64);
impl Drop for RetainedBytes {
    fn drop(&mut self) {
        RETAINED_BYTES.fetch_sub(self.0, Ordering::AcqRel);
    }
}

/// One received chunk with the in-flight permit and the process quota it
/// holds until the decoder has read it.
struct InFlightChunk {
    bytes: Bytes,
    _permit: tokio::sync::OwnedSemaphorePermit,
    _retained: RetainedBytes,
}

/// The decoder's `Read` over the chunks the pump hands it; blocks on the
/// channel from the decoder's blocking thread. `eof` records that the
/// producer closed the channel while the reader still wanted bytes — an
/// Arrow stream reader treats end-of-file as end-of-stream, so this is
/// what tells a truncated stream from a finished one.
struct ChunkRead {
    chunks: tokio::sync::mpsc::UnboundedReceiver<Result<InFlightChunk, String>>,
    current: Option<InFlightChunk>,
    offset: usize,
    eof: bool,
    failure: Option<String>,
}

impl Read for ChunkRead {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        if buf.is_empty() {
            return Ok(0);
        }
        loop {
            if let Some(chunk) = &self.current {
                let remaining = &chunk.bytes[self.offset..];
                if !remaining.is_empty() {
                    let count = remaining.len().min(buf.len());
                    buf[..count].copy_from_slice(&remaining[..count]);
                    self.offset += count;
                    if self.offset == chunk.bytes.len() {
                        self.current = None;
                        self.offset = 0;
                    }
                    return Ok(count);
                }
                self.current = None;
                self.offset = 0;
            }
            if self.eof {
                return Ok(0);
            }
            match self.chunks.blocking_recv() {
                Some(Ok(chunk)) => {
                    self.current = Some(chunk);
                    self.offset = 0;
                }
                Some(Err(message)) => {
                    self.eof = true;
                    self.failure = Some(message.clone());
                    return Err(std::io::Error::other(message));
                }
                None => {
                    self.eof = true;
                    return Ok(0);
                }
            }
        }
    }
}

/// A task result decoded as its bytes arrive: the schema first, then each
/// batch as soon as its message is complete. Dropping it stops the decoder
/// and the network pump.
pub struct ReceivedStream {
    schema: SchemaRef,
    batches: tokio::sync::mpsc::Receiver<Result<RecordBatch, String>>,
    received: Arc<AtomicU64>,
}

impl ReceivedStream {
    pub fn schema(&self) -> SchemaRef {
        Arc::clone(&self.schema)
    }
    /// Encoded bytes received from the network so far.
    pub fn bytes(&self) -> u64 {
        self.received.load(Ordering::Acquire)
    }
    /// The next batch; `None` once the stream ended with its end-of-stream
    /// marker. A stream that ends without the marker, a network failure or
    /// an undecodable message is the final `Err`.
    pub async fn next_batch(&mut self) -> Option<Result<RecordBatch, String>> {
        self.batches.recv().await
    }
}

impl futures::Stream for ReceivedStream {
    type Item = Result<RecordBatch, String>;
    fn poll_next(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Option<Self::Item>> {
        self.batches.poll_recv(cx)
    }
}

/// Decode a task result's Arrow IPC stream from the response body as it
/// arrives. Returns once the schema is decoded.
pub async fn receive_stream(response: reqwest::Response) -> Result<ReceivedStream, String> {
    let content_length = response.content_length();
    let body = futures::stream::unfold(response, |mut response| async move {
        match response.chunk().await {
            Ok(Some(chunk)) => Some((Ok(chunk), response)),
            Ok(None) => None,
            Err(error) => Some((Err(error.to_string()), response)),
        }
    });
    receive_stream_from(body, content_length, MAX_PAYLOAD_BYTES).await
}

/// `receive_stream` over any chunk source. The pump reads chunks into a
/// channel bounded by `STREAM_IN_FLIGHT_BYTES` and the process quota; the
/// decoder runs on a blocking thread and hands batches over a channel of
/// `STREAM_DECODED_BATCHES`.
pub async fn receive_stream_from<S>(
    body: S,
    content_length: Option<u64>,
    limit: u64,
) -> Result<ReceivedStream, String>
where
    S: futures::Stream<Item = Result<Bytes, String>> + Send + 'static,
{
    if content_length.is_some_and(|bytes| bytes > limit) {
        return Err("Arrow payload Content-Length exceeds receive limit".into());
    }
    let (chunk_sender, chunk_receiver) = tokio::sync::mpsc::unbounded_channel();
    let (schema_sender, schema_receiver) = tokio::sync::oneshot::channel();
    let (batch_sender, batch_receiver) = tokio::sync::mpsc::channel(STREAM_DECODED_BATCHES);
    let received = Arc::new(AtomicU64::new(0));
    tokio::spawn(pump_chunks(
        body,
        limit,
        chunk_sender,
        Arc::clone(&received),
    ));
    tokio::task::spawn_blocking(move || {
        decode_chunks(chunk_receiver, schema_sender, batch_sender);
    });
    let schema = schema_receiver
        .await
        .map_err(|_| "Arrow stream decoder stopped before reading the schema".to_owned())??;
    Ok(ReceivedStream {
        schema,
        batches: batch_receiver,
        received,
    })
}

async fn pump_chunks<S>(
    body: S,
    limit: u64,
    chunks: tokio::sync::mpsc::UnboundedSender<Result<InFlightChunk, String>>,
    received: Arc<AtomicU64>,
) where
    S: futures::Stream<Item = Result<Bytes, String>>,
{
    let in_flight = Arc::new(tokio::sync::Semaphore::new(STREAM_IN_FLIGHT_BYTES));
    let mut total = 0u64;
    let mut body = std::pin::pin!(body);
    while let Some(next) = body.next().await {
        let chunk = match next {
            Ok(chunk) => chunk,
            Err(error) => {
                let _ = chunks.send(Err(format!("network receive: {error}")));
                return;
            }
        };
        let mut offset = 0;
        while offset < chunk.len() {
            let end = (offset + STREAM_IN_FLIGHT_BYTES).min(chunk.len());
            let piece = chunk.slice(offset..end);
            offset = end;
            let bytes = piece.len() as u64;
            total = total.saturating_add(bytes);
            if total > limit {
                let _ = chunks.send(Err("Arrow payload exceeds receive limit".into()));
                return;
            }
            if RETAINED_BYTES
                .fetch_update(Ordering::AcqRel, Ordering::Acquire, |used| {
                    used.checked_add(bytes)
                        .filter(|total| *total <= PROCESS_SPOOL_BYTES)
                })
                .is_err()
            {
                let _ = chunks.send(Err("process IPC disk spool quota exceeded".into()));
                return;
            }
            let retained = RetainedBytes(bytes);
            let Ok(permit) = Arc::clone(&in_flight)
                .acquire_many_owned(piece.len() as u32)
                .await
            else {
                return;
            };
            received.fetch_add(bytes, Ordering::AcqRel);
            if chunks
                .send(Ok(InFlightChunk {
                    bytes: piece,
                    _permit: permit,
                    _retained: retained,
                }))
                .is_err()
            {
                // The decoder stopped: nothing reads what is left.
                return;
            }
        }
    }
}

fn decode_chunks(
    chunks: tokio::sync::mpsc::UnboundedReceiver<Result<InFlightChunk, String>>,
    schema: tokio::sync::oneshot::Sender<Result<SchemaRef, String>>,
    batches: tokio::sync::mpsc::Sender<Result<RecordBatch, String>>,
) {
    let read = ChunkRead {
        chunks,
        current: None,
        offset: 0,
        eof: false,
        failure: None,
    };
    let mut reader = match StreamReader::try_new(read, None) {
        Ok(reader) => reader,
        Err(error) => {
            let _ = schema.send(Err(format!("invalid Arrow stream: {error}")));
            return;
        }
    };
    if schema.send(Ok(reader.schema())).is_err() {
        return;
    }
    loop {
        match reader.next() {
            Some(Ok(batch)) => {
                if batches.blocking_send(Ok(batch)).is_err() {
                    return;
                }
            }
            Some(Err(error)) => {
                let message = reader
                    .get_mut()
                    .failure
                    .take()
                    .unwrap_or_else(|| format!("invalid Arrow stream: {error}"));
                let _ = batches.blocking_send(Err(message));
                return;
            }
            None => {
                if reader.get_ref().eof {
                    let _ = batches.blocking_send(Err(
                        "truncated Arrow stream: the producer ended before the end-of-stream marker"
                            .into(),
                    ));
                }
                return;
            }
        }
    }
}

impl ArrowPayload {
    /// A payload spooled from IPC stream bytes, as `receive` would spool a
    /// response body. Tests only.
    #[cfg(test)]
    pub(crate) fn from_ipc_bytes(bytes: &[u8]) -> Result<Self, String> {
        let path = ipc_spool_root().join(format!("kaveon-ipc-{}.arrow", uuid::Uuid::new_v4()));
        std::fs::write(&path, bytes).map_err(|error| error.to_string())?;
        RETAINED_BYTES.fetch_add(bytes.len() as u64, Ordering::AcqRel);
        let spool = DiskSpool {
            path,
            bytes: bytes.len() as u64,
        };
        let reader = StreamReader::try_new(
            BufReader::new(File::open(&spool.path).map_err(|error| error.to_string())?),
            None,
        )
        .map_err(|error| error.to_string())?;
        Ok(Self {
            reader,
            spool: std::sync::Arc::new(spool),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn bounded_encoder_never_appends_past_limit() {
        let mut buffer = BoundedBuffer::new(4);
        buffer.write_all(&[1, 2, 3, 4]).unwrap();
        assert!(buffer.write_all(&[5]).is_err());
        assert_eq!(buffer.into_bytes(), vec![1, 2, 3, 4]);
    }

    #[tokio::test]
    async fn chunked_receive_and_interrupted_body_release_spool() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        for complete in [true, false] {
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let address = listener.local_addr().unwrap();
            let task = tokio::spawn(async move {
                let (mut socket, _) = listener.accept().await.unwrap();
                let mut request = [0; 1024];
                let _ = socket.read(&mut request).await;
                socket
                    .write_all(b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n40\r\n")
                    .await
                    .unwrap();
                socket.write_all(&[0; 64]).await.unwrap();
                socket.write_all(b"\r\n").await.unwrap();
                if complete {
                    socket.write_all(b"40\r\n").await.unwrap();
                    socket.write_all(&[0; 64]).await.unwrap();
                    socket.write_all(b"\r\n0\r\n\r\n").await.unwrap();
                }
            });
            let response = reqwest::get(format!("http://{address}/")).await.unwrap();
            let error = receive_with_limit(response, 96).await.err().unwrap();
            assert!(if complete {
                error.contains("receive limit")
            } else {
                error.starts_with("network receive:")
            });
            task.await.unwrap();
        }
    }

    #[tokio::test]
    async fn arrow_spool_round_trips_and_releases_private_file() {
        let mut bytes = Vec::new();
        let schema = arrow::datatypes::Schema::empty();
        {
            let mut writer =
                arrow::ipc::writer::StreamWriter::try_new(&mut bytes, &schema).unwrap();
            writer.finish().unwrap();
        }
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let task = tokio::spawn(async move {
            axum::serve(
                listener,
                axum::Router::new().route(
                    "/",
                    axum::routing::get(move || {
                        let bytes = bytes.clone();
                        async move { bytes }
                    }),
                ),
            )
            .await
            .unwrap();
        });
        let mut payload = receive(reqwest::get(format!("http://{address}/")).await.unwrap())
            .await
            .unwrap();
        assert_eq!(*payload.schema(), schema);
        assert!(payload.next_batch().unwrap().is_none());
        let path = payload.spool.path.clone();
        assert!(path.exists());
        let mut fork = payload.fork().unwrap();
        drop(payload);
        assert!(path.exists());
        assert_eq!(*fork.schema(), schema);
        assert!(fork.next_batch().unwrap().is_none());
        drop(fork);
        assert!(!path.exists());
        task.abort();
    }
    /// An IPC stream of three one-column batches, cut at its message
    /// boundaries: schema, batch 0, batch 1, batch 2, end-of-stream.
    fn ipc_messages() -> (SchemaRef, Vec<Vec<u8>>) {
        use arrow::array::Int64Array;
        use arrow::datatypes::{DataType, Field, Schema};
        let schema = Arc::new(Schema::new(vec![Field::new("v", DataType::Int64, false)]));
        let mut bytes = Vec::new();
        let mut cuts = Vec::new();
        {
            let mut writer =
                arrow::ipc::writer::StreamWriter::try_new(&mut bytes, &schema).unwrap();
            cuts.push(writer.get_ref().len());
            for round in 0..3i64 {
                let batch = RecordBatch::try_new(
                    Arc::clone(&schema),
                    vec![Arc::new(Int64Array::from(vec![round; 4]))],
                )
                .unwrap();
                writer.write(&batch).unwrap();
                cuts.push(writer.get_ref().len());
            }
            writer.finish().unwrap();
            cuts.push(writer.get_ref().len());
        }
        let mut messages = Vec::new();
        let mut start = 0;
        for cut in cuts {
            messages.push(bytes[start..cut].to_vec());
            start = cut;
        }
        (schema, messages)
    }

    fn chunk_source() -> (
        tokio::sync::mpsc::UnboundedSender<Result<Bytes, String>>,
        impl futures::Stream<Item = Result<Bytes, String>> + Send + 'static,
    ) {
        let (sender, receiver) = tokio::sync::mpsc::unbounded_channel();
        let stream = futures::stream::unfold(receiver, |mut receiver| async move {
            receiver.recv().await.map(|chunk| (chunk, receiver))
        });
        (sender, stream)
    }

    #[tokio::test]
    async fn streamed_receive_decodes_each_batch_as_its_bytes_arrive() {
        let (schema, messages) = ipc_messages();
        let (sender, body) = chunk_source();
        // The schema alone unblocks the receiver; no batch has arrived.
        sender.send(Ok(Bytes::from(messages[0].clone()))).unwrap();
        let mut stream = receive_stream_from(body, None, MAX_PAYLOAD_BYTES)
            .await
            .unwrap();
        assert_eq!(stream.schema(), schema);
        assert_eq!(stream.bytes(), messages[0].len() as u64);
        // Batch 0 in one chunk, batch 1 split across two: each is decoded
        // while the sender is still open.
        sender.send(Ok(Bytes::from(messages[1].clone()))).unwrap();
        let first = stream.next_batch().await.unwrap().unwrap();
        assert_eq!(first.num_rows(), 4);
        let half = messages[2].len() / 2;
        sender
            .send(Ok(Bytes::from(messages[2][..half].to_vec())))
            .unwrap();
        sender
            .send(Ok(Bytes::from(messages[2][half..].to_vec())))
            .unwrap();
        let second = stream.next_batch().await.unwrap().unwrap();
        assert_eq!(second.num_rows(), 4);
        sender.send(Ok(Bytes::from(messages[3].clone()))).unwrap();
        sender.send(Ok(Bytes::from(messages[4].clone()))).unwrap();
        assert!(stream.next_batch().await.unwrap().is_ok());
        drop(sender);
        assert!(stream.next_batch().await.is_none());
        assert_eq!(
            stream.bytes(),
            messages
                .iter()
                .map(|message| message.len() as u64)
                .sum::<u64>()
        );
    }

    #[tokio::test]
    async fn streamed_receive_reports_a_stream_that_ends_without_its_marker() {
        // Cut at a message boundary: an Arrow reader would call that a
        // finished stream; the receiver knows the marker never came.
        let (_, messages) = ipc_messages();
        let (sender, body) = chunk_source();
        sender.send(Ok(Bytes::from(messages[0].clone()))).unwrap();
        sender.send(Ok(Bytes::from(messages[1].clone()))).unwrap();
        let mut stream = receive_stream_from(body, None, MAX_PAYLOAD_BYTES)
            .await
            .unwrap();
        assert!(stream.next_batch().await.unwrap().is_ok());
        drop(sender);
        let error = stream.next_batch().await.unwrap().unwrap_err();
        assert!(error.starts_with("truncated Arrow stream"), "{error}");
        assert!(stream.next_batch().await.is_none());

        // Cut inside a message: the decoder fails on the message.
        let (sender, body) = chunk_source();
        sender.send(Ok(Bytes::from(messages[0].clone()))).unwrap();
        let half = messages[1].len() / 2;
        sender
            .send(Ok(Bytes::from(messages[1][..half].to_vec())))
            .unwrap();
        let mut stream = receive_stream_from(body, None, MAX_PAYLOAD_BYTES)
            .await
            .unwrap();
        drop(sender);
        assert!(stream.next_batch().await.unwrap().is_err());

        // A network failure mid-stream is reported as such.
        let (sender, body) = chunk_source();
        sender.send(Ok(Bytes::from(messages[0].clone()))).unwrap();
        let mut stream = receive_stream_from(body, None, MAX_PAYLOAD_BYTES)
            .await
            .unwrap();
        sender.send(Err("connection reset".into())).unwrap();
        let error = stream.next_batch().await.unwrap().unwrap_err();
        assert_eq!(error, "network receive: connection reset");

        // Nothing before the schema is a failed receive, not a stream.
        let (sender, body) = chunk_source();
        drop(sender);
        assert!(
            receive_stream_from(body, None, MAX_PAYLOAD_BYTES)
                .await
                .is_err()
        );
    }

    #[tokio::test]
    async fn receive_cap_rejects_large_body_and_cleans_up() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let task = tokio::spawn(async move {
            axum::serve(
                listener,
                axum::Router::new().route("/", axum::routing::get(|| async { vec![0u8; 1024] })),
            )
            .await
            .unwrap();
        });
        let response = reqwest::get(format!("http://{address}/")).await.unwrap();
        assert!(receive_with_limit(response, 64).await.is_err());
        task.abort();
    }
}
