//! Receive Arrow IPC incrementally into a bounded private disk spool.
//! The immutable spool permits decoding without retaining an encoded body copy.
use arrow::{datatypes::SchemaRef, ipc::reader::StreamReader, record_batch::RecordBatch};
use std::{
    fs::{File, OpenOptions},
    io::{BufReader, Write},
    path::PathBuf,
    sync::atomic::{AtomicU64, Ordering},
};

pub const MAX_PAYLOAD_BYTES: u64 = 128 * 1024 * 1024;
const PROCESS_SPOOL_BYTES: u64 = 512 * 1024 * 1024;
static RETAINED_BYTES: AtomicU64 = AtomicU64::new(0);
static CACHED_BYTES: AtomicU64 = AtomicU64::new(0);
pub struct CachedTaskResult {
    pub bytes: Vec<u8>,
    pub elapsed_us: u64,
    pub scan_metrics_header: Option<String>,
    pub execution_metrics_header: Option<String>,
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
        })
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
                return Err("decoded Arrow payload exceeds 128 MiB limit".into());
            }
            batches.push(batch);
        }
        Ok((schema, batches))
    }
}

pub async fn receive(response: reqwest::Response) -> Result<ArrowPayload, String> {
    receive_with_limit(response, MAX_PAYLOAD_BYTES).await
}
async fn receive_with_limit(
    mut response: reqwest::Response,
    limit: u64,
) -> Result<ArrowPayload, String> {
    if response.content_length().is_some_and(|bytes| bytes > limit) {
        return Err("Arrow payload Content-Length exceeds receive limit".into());
    }
    let path = std::env::temp_dir().join(format!("kaveon-ipc-{}.arrow", uuid::Uuid::new_v4()));
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
