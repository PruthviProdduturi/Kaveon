//! A bounded append-only spool for exchange inputs.
//!
//! The writer can receive a network body while readers consume complete or
//! partial Arrow streams. Readers wait for bytes to arrive, can be forked from
//! offset zero for retry/multi-consumer execution, and never retain the whole
//! payload in memory.

use std::fs::{File, OpenOptions};
use std::io::{self, Read, Seek, SeekFrom, Write};
#[cfg(windows)]
use std::os::windows::fs::OpenOptionsExt;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::Duration;

struct State {
    written: u64,
    finished: bool,
    failure: Option<String>,
}

struct Inner {
    path: PathBuf,
    limit: u64,
    quota: Option<Quota>,
    state: Mutex<State>,
    changed: Condvar,
    cancelled: std::sync::atomic::AtomicBool,
}

#[derive(Clone, Copy)]
struct Quota {
    counter: &'static AtomicU64,
    limit: u64,
}

impl Drop for Inner {
    fn drop(&mut self) {
        if let Some(quota) = self.quota {
            let written = self
                .state
                .lock()
                .map(|state| state.written)
                .unwrap_or_default();
            quota.counter.fetch_sub(written, Ordering::AcqRel);
        }
        let _ = std::fs::remove_file(&self.path);
    }
}

/// A spool whose bytes remain replayable until all readers and the writer are
/// dropped. The file is bounded before bytes are written.
#[derive(Clone)]
pub struct ReplayableSpool {
    inner: Arc<Inner>,
}

pub struct SpoolWriter {
    inner: Arc<Inner>,
    file: Option<File>,
}

pub struct SpoolReader {
    inner: Arc<Inner>,
    file: File,
    offset: u64,
}

impl ReplayableSpool {
    pub fn new(limit: u64) -> io::Result<(Self, SpoolWriter)> {
        Self::new_inner(limit, None)
    }

    /// Create a spool charged against a process-wide byte counter. The
    /// counter is released when the last reader/writer drops the spool.
    pub fn new_with_quota(
        limit: u64,
        counter: &'static AtomicU64,
        quota_limit: u64,
    ) -> io::Result<(Self, SpoolWriter)> {
        Self::new_inner(
            limit,
            Some(Quota {
                counter,
                limit: quota_limit,
            }),
        )
    }

    fn new_inner(limit: u64, quota: Option<Quota>) -> io::Result<(Self, SpoolWriter)> {
        if limit == 0 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "spool limit must be positive",
            ));
        }
        let path =
            std::env::temp_dir().join(format!("kaveon-replayable-{}.arrow", uuid::Uuid::new_v4()));
        let file = open_spool(&path, true)?;
        let inner = Arc::new(Inner {
            path,
            limit,
            quota,
            state: Mutex::new(State {
                written: 0,
                finished: false,
                failure: None,
            }),
            changed: Condvar::new(),
            cancelled: std::sync::atomic::AtomicBool::new(false),
        });
        Ok((
            Self {
                inner: Arc::clone(&inner),
            },
            SpoolWriter {
                inner,
                file: Some(file),
            },
        ))
    }

    pub fn reader(&self) -> io::Result<SpoolReader> {
        Ok(SpoolReader {
            inner: Arc::clone(&self.inner),
            file: open_spool(&self.inner.path, false)?,
            offset: 0,
        })
    }

    /// Open an Arrow IPC reader over the append-only spool. This blocks until
    /// the IPC schema is available, so callers should invoke it from a
    /// blocking task. Once opened, Arrow reads batches as the writer appends
    /// them and waits at the spool boundary instead of seeing a premature EOF.
    pub fn open_arrow_reader(&self) -> io::Result<arrow::ipc::reader::StreamReader<SpoolReader>> {
        arrow::ipc::reader::StreamReader::try_new(self.reader()?, None)
            .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error.to_string()))
    }

    pub fn cancel(&self) {
        self.inner
            .cancelled
            .store(true, std::sync::atomic::Ordering::Release);
        self.inner.changed.notify_all();
    }

    pub fn bytes(&self) -> u64 {
        self.inner
            .state
            .lock()
            .map(|state| state.written)
            .unwrap_or_default()
    }
}

impl SpoolWriter {
    pub fn append(&mut self, bytes: &[u8]) -> io::Result<()> {
        if self
            .inner
            .cancelled
            .load(std::sync::atomic::Ordering::Acquire)
        {
            return Err(io::Error::new(
                io::ErrorKind::Interrupted,
                "spool is cancelled",
            ));
        }
        let mut state = self.inner.state.lock().expect("spool state poisoned");
        if state.finished {
            return Err(io::Error::new(
                io::ErrorKind::BrokenPipe,
                "spool is finished",
            ));
        }
        let end = state
            .written
            .checked_add(bytes.len() as u64)
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "spool size overflow"))?;
        if end > self.inner.limit {
            return Err(io::Error::new(
                io::ErrorKind::StorageFull,
                "replayable spool limit exceeded",
            ));
        }
        if let Some(quota) = self.inner.quota {
            quota
                .counter
                .try_update(Ordering::AcqRel, Ordering::Acquire, |used| {
                    used.checked_add(bytes.len() as u64)
                        .filter(|total| *total <= quota.limit)
                })
                .map_err(|_| {
                    io::Error::new(io::ErrorKind::StorageFull, "process spool quota exceeded")
                })?;
        }
        self.file
            .as_mut()
            .expect("writer file open")
            .write_all(bytes)
            .inspect_err(|_error| {
                if let Some(quota) = self.inner.quota {
                    quota
                        .counter
                        .fetch_sub(bytes.len() as u64, Ordering::AcqRel);
                }
            })?;
        self.file.as_mut().expect("writer file open").flush()?;
        state.written = end;
        drop(state);
        self.inner.changed.notify_all();
        Ok(())
    }

    pub fn finish(self) -> io::Result<()> {
        self.finish_with_error(None)
    }

    pub fn finish_with_error(mut self, error: Option<String>) -> io::Result<()> {
        if let Some(file) = self.file.as_mut() {
            file.flush()?;
        }
        let mut state = self.inner.state.lock().expect("spool state poisoned");
        state.failure = error;
        state.finished = true;
        drop(state);
        self.inner.changed.notify_all();
        self.file.take();
        Ok(())
    }
}

impl Drop for SpoolWriter {
    fn drop(&mut self) {
        if self.file.is_some() {
            let mut state = self.inner.state.lock().expect("spool state poisoned");
            state.failure = Some("spool writer dropped before completion".into());
            state.finished = true;
            drop(state);
            self.inner.changed.notify_all();
        }
    }
}

impl SpoolReader {
    pub fn fork(&self) -> io::Result<Self> {
        Ok(Self {
            inner: Arc::clone(&self.inner),
            file: open_spool(&self.inner.path, false)?,
            offset: 0,
        })
    }
}

impl Read for SpoolReader {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        if buffer.is_empty() {
            return Ok(0);
        }
        loop {
            if self
                .inner
                .cancelled
                .load(std::sync::atomic::Ordering::Acquire)
            {
                return Err(io::Error::new(
                    io::ErrorKind::Interrupted,
                    "spool read cancelled",
                ));
            }
            let state = self.inner.state.lock().expect("spool state poisoned");
            if self.offset < state.written {
                let available = (state.written - self.offset) as usize;
                let count = available.min(buffer.len());
                drop(state);
                self.file.seek(SeekFrom::Start(self.offset))?;
                let read = self.file.read(&mut buffer[..count])?;
                if read == 0 {
                    continue;
                }
                self.offset += read as u64;
                return Ok(read);
            }
            if state.finished {
                if let Some(error) = &state.failure {
                    return Err(io::Error::new(io::ErrorKind::UnexpectedEof, error.clone()));
                }
                return Ok(0);
            }
            let _ = self
                .inner
                .changed
                .wait_timeout(state, Duration::from_millis(50))
                .expect("spool state poisoned");
        }
    }
}

fn open_spool(path: &PathBuf, truncate: bool) -> io::Result<File> {
    let mut options = OpenOptions::new();
    options.read(true).write(truncate).create(truncate);
    if truncate {
        options.truncate(true);
    }
    #[cfg(windows)]
    options.share_mode(0x00000007);
    options.open(path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc;
    use std::thread;

    #[test]
    fn reader_waits_for_partial_writes_and_reads_incrementally() {
        let (spool, mut writer) = ReplayableSpool::new(32).unwrap();
        let mut reader = spool.reader().unwrap();
        let (received, sent) = mpsc::channel();
        let task = thread::spawn(move || {
            let mut bytes = [0; 4];
            let first = reader.read(&mut bytes).unwrap();
            received.send(bytes[..first].to_vec()).unwrap();
            let second = reader.read(&mut bytes).unwrap();
            received.send(bytes[..second].to_vec()).unwrap();
        });
        writer.append(b"abcd").unwrap();
        assert_eq!(sent.recv().unwrap(), b"abcd");
        writer.append(b"efgh").unwrap();
        assert_eq!(sent.recv().unwrap(), b"efgh");
        writer.finish().unwrap();
        task.join().unwrap();
    }

    #[test]
    fn forks_replay_completed_bytes_for_retries_and_multiple_consumers() {
        let (spool, mut writer) = ReplayableSpool::new(64).unwrap();
        writer.append(b"replay-me").unwrap();
        writer.finish().unwrap();
        let mut first = spool.reader().unwrap();
        let mut retry = first.fork().unwrap();
        let mut one = Vec::new();
        let mut two = Vec::new();
        first.read_to_end(&mut one).unwrap();
        retry.read_to_end(&mut two).unwrap();
        assert_eq!(one, b"replay-me");
        assert_eq!(two, one);
    }

    #[test]
    fn cancellation_releases_a_reader_waiting_for_the_next_chunk() {
        let (spool, _writer) = ReplayableSpool::new(32).unwrap();
        let mut reader = spool.reader().unwrap();
        let task = thread::spawn(move || {
            let mut bytes = [0; 1];
            reader.read(&mut bytes).unwrap_err().kind()
        });
        thread::sleep(Duration::from_millis(20));
        spool.cancel();
        assert_eq!(task.join().unwrap(), io::ErrorKind::Interrupted);
    }

    #[test]
    fn cancellation_stops_late_writer_appends_without_charging_bytes() {
        let (spool, mut writer) = ReplayableSpool::new(32).unwrap();
        spool.cancel();
        assert_eq!(
            writer.append(b"late payload").unwrap_err().kind(),
            io::ErrorKind::Interrupted
        );
        assert_eq!(spool.bytes(), 0);
    }

    #[test]
    fn quota_bounds_concurrent_spools_and_releases_after_retry_cleanup() {
        static USED: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        USED.store(0, std::sync::atomic::Ordering::Release);

        let (spool, mut writer) = ReplayableSpool::new_with_quota(8, &USED, 8).unwrap();
        writer.append(b"12345678").unwrap();
        let (blocked, mut blocked_writer) = ReplayableSpool::new_with_quota(8, &USED, 8).unwrap();
        assert_eq!(
            blocked_writer.append(b"retry").unwrap_err().kind(),
            io::ErrorKind::StorageFull
        );
        drop(blocked_writer);
        drop(blocked);
        drop(writer);
        drop(spool);

        let (_retry, mut retry_writer) = ReplayableSpool::new_with_quota(8, &USED, 8).unwrap();
        retry_writer.append(b"retry").unwrap();
    }

    #[test]
    fn arrow_reader_waits_for_later_ipc_batches() {
        use arrow::array::{Int64Array, RecordBatch};
        use arrow::datatypes::{DataType, Field, Schema};
        use arrow::ipc::writer::StreamWriter;

        let schema = Arc::new(Schema::new(vec![Field::new("id", DataType::Int64, false)]));
        let mut encoded = Vec::new();
        {
            let mut writer = StreamWriter::try_new(&mut encoded, &schema).unwrap();
            writer
                .write(
                    &RecordBatch::try_new(
                        schema.clone(),
                        vec![Arc::new(Int64Array::from(vec![1, 2]))],
                    )
                    .unwrap(),
                )
                .unwrap();
            writer.finish().unwrap();
        }
        let (spool, mut writer) = ReplayableSpool::new(encoded.len() as u64 + 8).unwrap();
        let split = encoded.len() / 2;
        let (ready_tx, ready_rx) = mpsc::channel();
        let (done_tx, done_rx) = mpsc::channel();
        let reader_spool = spool.clone();
        let task = thread::spawn(move || {
            let mut reader = reader_spool.open_arrow_reader().unwrap();
            let batch = reader.next().unwrap().unwrap();
            ready_tx.send(batch.num_rows()).unwrap();
            assert!(reader.next().is_none());
            done_tx.send(()).unwrap();
        });
        writer.append(&encoded[..split]).unwrap();
        writer.append(&encoded[split..]).unwrap();
        writer.finish().unwrap();
        assert_eq!(ready_rx.recv().unwrap(), 2);
        done_rx.recv().unwrap();
        task.join().unwrap();
    }
}
