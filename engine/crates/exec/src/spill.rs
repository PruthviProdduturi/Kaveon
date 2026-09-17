use std::{
    fs::{self, File, OpenOptions},
    io::{BufWriter, Write},
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
    time::Instant,
};

use arrow::{
    datatypes::SchemaRef,
    ipc::{reader::StreamReader, writer::StreamWriter},
    record_batch::RecordBatch,
};
use kaveon_core::{KaveonError, Result};

static NEXT_SPILL_DIRECTORY: AtomicU64 = AtomicU64::new(0);
static NEXT_RUN: AtomicU64 = AtomicU64::new(0);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SpillSnapshot {
    pub current_bytes: u64,
    pub peak_bytes: u64,
    pub limit_bytes: u64,
    pub bytes_written: u64,
    pub runs_written: u64,
    pub compactions: u64,
    pub compaction_input_bytes: u64,
    pub write_us: u64,
    pub read_us: u64,
}

#[derive(Debug)]
struct SpillInner {
    directory: PathBuf,
    limit_bytes: u64,
    current_bytes: AtomicU64,
    peak_bytes: AtomicU64,
    bytes_written: AtomicU64,
    runs_written: AtomicU64,
    compactions: AtomicU64,
    compaction_input_bytes: AtomicU64,
    write_us: AtomicU64,
    read_us: AtomicU64,
}

impl SpillInner {
    fn reserve(&self, bytes: u64) -> Result<()> {
        let mut current = self.current_bytes.load(Ordering::Acquire);
        loop {
            let Some(next) = current.checked_add(bytes) else {
                return Err(self.limit_error(bytes, current));
            };
            if next > self.limit_bytes {
                return Err(self.limit_error(bytes, current));
            }
            match self.current_bytes.compare_exchange_weak(
                current,
                next,
                Ordering::AcqRel,
                Ordering::Acquire,
            ) {
                Ok(_) => {
                    self.peak_bytes.fetch_max(next, Ordering::AcqRel);
                    return Ok(());
                }
                Err(observed) => current = observed,
            }
        }
    }

    fn limit_error(&self, requested: u64, current: u64) -> KaveonError {
        KaveonError::Execution(format!(
            "spill limit exceeded: cannot write {requested} bytes with {current} of {} bytes already used",
            self.limit_bytes
        ))
    }

    fn release(&self, bytes: u64) {
        let previous = self.current_bytes.fetch_sub(bytes, Ordering::AcqRel);
        debug_assert!(previous >= bytes, "spill accounting underflow");
    }
}

impl Drop for SpillInner {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.directory);
    }
}

/// Owns a private spill directory and enforces one byte budget across its runs.
#[derive(Debug, Clone)]
pub struct SpillManager {
    inner: Arc<SpillInner>,
}

impl SpillManager {
    pub fn new(root: impl AsRef<Path>, limit_bytes: u64) -> Result<Self> {
        if limit_bytes == 0 {
            return Err(KaveonError::Execution(
                "spill limit must be greater than zero".into(),
            ));
        }

        fs::create_dir_all(root.as_ref())?;
        let directory = loop {
            let directory_id = NEXT_SPILL_DIRECTORY.fetch_add(1, Ordering::Relaxed);
            let candidate = root.as_ref().join(format!(
                "kaveon-spill-{}-{directory_id}",
                std::process::id()
            ));
            match fs::create_dir(&candidate) {
                Ok(()) => break candidate,
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(error) => return Err(error.into()),
            }
        };

        Ok(Self {
            inner: Arc::new(SpillInner {
                directory,
                limit_bytes,
                current_bytes: AtomicU64::new(0),
                peak_bytes: AtomicU64::new(0),
                bytes_written: AtomicU64::new(0),
                runs_written: AtomicU64::new(0),
                compactions: AtomicU64::new(0),
                compaction_input_bytes: AtomicU64::new(0),
                write_us: AtomicU64::new(0),
                read_us: AtomicU64::new(0),
            }),
        })
    }

    #[must_use]
    pub fn snapshot(&self) -> SpillSnapshot {
        SpillSnapshot {
            current_bytes: self.inner.current_bytes.load(Ordering::Acquire),
            peak_bytes: self.inner.peak_bytes.load(Ordering::Acquire),
            limit_bytes: self.inner.limit_bytes,
            bytes_written: self.inner.bytes_written.load(Ordering::Acquire),
            runs_written: self.inner.runs_written.load(Ordering::Acquire),
            compactions: self.inner.compactions.load(Ordering::Acquire),
            compaction_input_bytes: self.inner.compaction_input_bytes.load(Ordering::Acquire),
            write_us: self.inner.write_us.load(Ordering::Acquire),
            read_us: self.inner.read_us.load(Ordering::Acquire),
        }
    }

    pub fn record_compaction(&self, input_bytes: u64) {
        self.inner.compactions.fetch_add(1, Ordering::AcqRel);
        self.inner
            .compaction_input_bytes
            .fetch_add(input_bytes, Ordering::AcqRel);
    }

    pub fn write_run(&self, schema: &SchemaRef, batches: &[RecordBatch]) -> Result<SpillRun> {
        self.write_run_stream(schema, batches.iter().cloned().map(Ok))
    }

    pub fn write_run_stream<I>(&self, schema: &SchemaRef, batches: I) -> Result<SpillRun>
    where
        I: IntoIterator<Item = Result<RecordBatch>>,
    {
        let mut writer = self.begin_run(schema)?;
        for batch in batches {
            writer.write(&batch?)?;
        }
        writer.finish()
    }

    /// Open a run to be written batch by batch — several runs can be open
    /// at once, each taking its bytes from the shared limit as they are
    /// written. The run exists once `finish` returns; a writer dropped
    /// before that leaves nothing behind.
    pub fn begin_run(&self, schema: &SchemaRef) -> Result<SpillRunWriter> {
        let run_id = NEXT_RUN.fetch_add(1, Ordering::Relaxed);
        let path = self.inner.directory.join(format!("run-{run_id}.arrow"));
        let file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)?;
        let output = BoundedSpillWriter::new(BufWriter::new(file), Arc::clone(&self.inner));
        let writer = match StreamWriter::try_new(output, schema) {
            Ok(writer) => writer,
            Err(error) => {
                let _ = fs::remove_file(&path);
                return Err(error.into());
            }
        };
        Ok(SpillRunWriter {
            writer: Some(writer),
            schema: Arc::clone(schema),
            inner: Arc::clone(&self.inner),
            path,
        })
    }
}

/// A run being written: an Arrow IPC stream on the spill directory whose
/// bytes are reserved against the shared limit as they land. The write
/// time it records is the time inside its calls — several writers open
/// side by side do not count each other's.
pub struct SpillRunWriter {
    writer: Option<StreamWriter<BoundedSpillWriter<BufWriter<File>>>>,
    schema: SchemaRef,
    inner: Arc<SpillInner>,
    path: PathBuf,
}

impl SpillRunWriter {
    pub fn write(&mut self, batch: &RecordBatch) -> Result<()> {
        if batch.schema().as_ref() != self.schema.as_ref() {
            return Err(KaveonError::Execution(
                "spill batch schema does not match the run schema".into(),
            ));
        }
        let started = Instant::now();
        let writer = self
            .writer
            .as_mut()
            .ok_or_else(|| KaveonError::Execution("spill run already finished".into()))?;
        let written = writer.write(batch);
        self.inner
            .write_us
            .fetch_add(elapsed_us(started), Ordering::AcqRel);
        written?;
        Ok(())
    }

    /// Close the stream; the run and its byte reservation are the
    /// caller's from here.
    pub fn finish(mut self) -> Result<SpillRun> {
        let started = Instant::now();
        let mut writer = self
            .writer
            .take()
            .ok_or_else(|| KaveonError::Execution("spill run already finished".into()))?;
        let finished = (|| -> Result<u64> {
            writer.finish()?;
            let mut output = writer.into_inner()?;
            output.flush()?;
            Ok(output.commit())
        })();
        self.inner
            .write_us
            .fetch_add(elapsed_us(started), Ordering::AcqRel);
        let bytes = match finished {
            Ok(bytes) => bytes,
            Err(error) => {
                let _ = fs::remove_file(&self.path);
                return Err(error);
            }
        };
        self.inner.bytes_written.fetch_add(bytes, Ordering::AcqRel);
        self.inner.runs_written.fetch_add(1, Ordering::AcqRel);
        Ok(SpillRun {
            inner: Arc::clone(&self.inner),
            path: std::mem::take(&mut self.path),
            bytes,
        })
    }
}

impl Drop for SpillRunWriter {
    fn drop(&mut self) {
        // Unfinished: the bounded writer returns its bytes, the file goes.
        if let Some(writer) = self.writer.take() {
            drop(writer);
            let _ = fs::remove_file(&self.path);
        }
    }
}

/// A durable Arrow IPC run whose file and byte reservation are released on drop.
#[derive(Debug)]
pub struct SpillRun {
    inner: Arc<SpillInner>,
    path: PathBuf,
    bytes: u64,
}

impl SpillRun {
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }

    #[must_use]
    pub fn bytes(&self) -> u64 {
        self.bytes
    }

    pub fn read(&self) -> Result<Vec<RecordBatch>> {
        let started = Instant::now();
        let file = File::open(&self.path)?;
        let batches = StreamReader::try_new(file, None)?
            .map(|batch| batch.map_err(KaveonError::from))
            .collect();
        self.inner
            .read_us
            .fetch_add(elapsed_us(started), Ordering::AcqRel);
        batches
    }

    pub fn reader(&self) -> Result<SpillRunReader> {
        let file = File::open(&self.path)?;
        Ok(SpillRunReader {
            reader: StreamReader::try_new(file, None)?,
            inner: Arc::clone(&self.inner),
        })
    }
}

pub struct SpillRunReader {
    reader: StreamReader<File>,
    inner: Arc<SpillInner>,
}

impl Iterator for SpillRunReader {
    type Item = Result<RecordBatch>;

    fn next(&mut self) -> Option<Self::Item> {
        let started = Instant::now();
        let batch = self
            .reader
            .next()
            .map(|batch| batch.map_err(KaveonError::from));
        self.inner
            .read_us
            .fetch_add(elapsed_us(started), Ordering::AcqRel);
        batch
    }
}

fn elapsed_us(started: Instant) -> u64 {
    u64::try_from(started.elapsed().as_micros()).unwrap_or(u64::MAX)
}

impl Drop for SpillRun {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
        self.inner.release(self.bytes);
    }
}

struct BoundedSpillWriter<W> {
    output: W,
    inner: Arc<SpillInner>,
    reserved_bytes: u64,
    committed: bool,
}

impl<W> BoundedSpillWriter<W> {
    fn new(output: W, inner: Arc<SpillInner>) -> Self {
        Self {
            output,
            inner,
            reserved_bytes: 0,
            committed: false,
        }
    }

    fn commit(mut self) -> u64 {
        self.committed = true;
        self.reserved_bytes
    }
}

impl<W: Write> Write for BoundedSpillWriter<W> {
    fn write(&mut self, buffer: &[u8]) -> std::io::Result<usize> {
        let requested = u64::try_from(buffer.len()).map_err(std::io::Error::other)?;
        self.inner
            .reserve(requested)
            .map_err(std::io::Error::other)?;

        match self.output.write(buffer) {
            Ok(written) => {
                let written = u64::try_from(written).map_err(std::io::Error::other)?;
                self.reserved_bytes += written;
                self.inner.release(requested - written);
                Ok(usize::try_from(written).expect("written byte count originated as usize"))
            }
            Err(error) => {
                self.inner.release(requested);
                Err(error)
            }
        }
    }

    fn flush(&mut self) -> std::io::Result<()> {
        self.output.flush()
    }
}

impl<W> Drop for BoundedSpillWriter<W> {
    fn drop(&mut self) {
        if !self.committed {
            self.inner.release(self.reserved_bytes);
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use arrow::{
        array::{Int64Array, StringArray},
        datatypes::{DataType, Field, Schema},
    };

    use super::*;

    const SPILL_LIMIT: u64 = 64 * 1_024;

    fn test_root(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!("kaveon-spill-test-{}-{name}", std::process::id()))
    }

    fn batch() -> RecordBatch {
        RecordBatch::try_new(
            Arc::new(Schema::new(vec![
                Field::new("id", DataType::Int64, false),
                Field::new("region", DataType::Utf8, false),
            ])),
            vec![
                Arc::new(Int64Array::from(vec![1, 2, 3])),
                Arc::new(StringArray::from(vec!["east", "west", "north"])),
            ],
        )
        .unwrap()
    }

    #[test]
    fn round_trips_arrow_batches_and_accounts_for_disk_bytes() {
        let root = test_root("round-trip");
        let manager = SpillManager::new(&root, SPILL_LIMIT).unwrap();
        let input = batch();
        let run = manager
            .write_run(&input.schema(), std::slice::from_ref(&input))
            .unwrap();

        assert!(run.bytes() > 0);
        let snapshot = manager.snapshot();
        assert_eq!(snapshot.current_bytes, run.bytes());
        assert_eq!(snapshot.bytes_written, run.bytes());
        assert_eq!(snapshot.runs_written, 1);
        assert_eq!(snapshot.compactions, 0);
        assert_eq!(run.read().unwrap(), vec![input]);
        let after_read = manager.snapshot();
        assert!(after_read.write_us >= snapshot.write_us);
        assert!(after_read.read_us >= snapshot.read_us);
        let path = run.path().to_owned();
        drop(run);

        assert!(!path.exists());
        assert_eq!(manager.snapshot().current_bytes, 0);
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn runs_open_side_by_side_take_their_bytes_as_written_and_leave_nothing_unfinished() {
        let root = test_root("side-by-side");
        let manager = SpillManager::new(&root, SPILL_LIMIT).unwrap();
        let input = batch();
        let mut first = manager.begin_run(&input.schema()).unwrap();
        let mut second = manager.begin_run(&input.schema()).unwrap();
        let mut abandoned = manager.begin_run(&input.schema()).unwrap();
        for _ in 0..3 {
            first.write(&input).unwrap();
            second.write(&input).unwrap();
            abandoned.write(&input).unwrap();
        }
        first.write(&input).unwrap();
        let held = manager.snapshot().current_bytes;
        assert!(held > 0, "open writers hold what they have written");
        assert_eq!(manager.snapshot().runs_written, 0);
        let abandoned_path = abandoned.path.clone();
        drop(abandoned);
        assert!(!abandoned_path.exists());
        assert!(manager.snapshot().current_bytes < held);
        let first = first.finish().unwrap();
        let second = second.finish().unwrap();
        assert_eq!(first.read().unwrap().len(), 4);
        assert_eq!(second.read().unwrap().len(), 3);
        let snapshot = manager.snapshot();
        assert_eq!(snapshot.runs_written, 2);
        assert_eq!(snapshot.current_bytes, first.bytes() + second.bytes());
        assert_eq!(snapshot.bytes_written, first.bytes() + second.bytes());
        assert!(snapshot.write_us > 0);
        let error = second
            .reader()
            .map(|_| ())
            .and_then(|()| {
                let mut writer = manager.begin_run(&input.schema())?;
                let other = RecordBatch::try_new(
                    Arc::new(Schema::new(vec![Field::new("id", DataType::Int64, false)])),
                    vec![Arc::new(Int64Array::from(vec![1]))],
                )
                .unwrap();
                writer.write(&other)
            })
            .unwrap_err();
        assert!(error.to_string().contains("does not match the run schema"));
        drop((first, second));
        assert_eq!(manager.snapshot().current_bytes, 0);
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn rejects_a_run_that_exceeds_the_shared_limit_without_leaking_bytes() {
        let root = test_root("limit");
        let manager = SpillManager::new(&root, 16).unwrap();
        let input = batch();

        let error = manager.write_run(&input.schema(), &[input]).unwrap_err();
        assert!(error.to_string().contains("spill limit exceeded"));
        assert_eq!(manager.snapshot().current_bytes, 0);
        assert!(
            fs::read_dir(&manager.inner.directory)
                .unwrap()
                .next()
                .is_none()
        );
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn concurrent_spill_admission_is_bounded_and_releases_capacity() {
        // Exercise the same atomic reservation used by Arrow IPC writers. Two
        // 32-byte writers may be admitted, but a racing third writer must be
        // rejected until an admitted writer releases its reservation.
        let root = test_root("concurrent-admission");
        let manager = SpillManager::new(&root, 64).unwrap();
        let mut workers = Vec::new();
        for _ in 0..8 {
            let manager = manager.clone();
            workers.push(std::thread::spawn(move || {
                if manager.inner.reserve(32).is_ok() {
                    Some(manager)
                } else {
                    None
                }
            }));
        }
        let admitted = workers
            .into_iter()
            .filter_map(|worker| worker.join().unwrap())
            .collect::<Vec<_>>();
        assert_eq!(admitted.len(), 2);
        assert_eq!(manager.snapshot().current_bytes, 64);
        for admitted in admitted {
            admitted.inner.release(32);
        }
        assert_eq!(manager.snapshot().current_bytes, 0);
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn reports_corrupt_ipc_without_releasing_the_live_run() {
        let root = test_root("corrupt");
        let manager = SpillManager::new(&root, SPILL_LIMIT).unwrap();
        let input = batch();
        let run = manager.write_run(&input.schema(), &[input]).unwrap();
        fs::write(run.path(), b"not-arrow-ipc").unwrap();

        assert!(run.read().is_err());
        assert_eq!(manager.snapshot().current_bytes, run.bytes());
        drop(run);
        assert_eq!(manager.snapshot().current_bytes, 0);
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn streaming_reader_yields_batches_incrementally() {
        let root = test_root("stream");
        let manager = SpillManager::new(&root, SPILL_LIMIT).unwrap();
        let input = batch();
        let run = manager
            .write_run(&input.schema(), &[input.clone(), input.clone()])
            .unwrap();
        let mut reader = run.reader().unwrap();

        assert_eq!(reader.next().unwrap().unwrap(), input);
        assert!(reader.next().is_some());
        assert!(reader.next().is_none());
        drop(reader);
        drop(run);
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn rejects_mismatched_batch_schema_and_cleans_partial_file() {
        let root = test_root("schema");
        let manager = SpillManager::new(&root, SPILL_LIMIT).unwrap();
        let input = batch();
        let wrong_schema = Arc::new(Schema::new(vec![Field::new(
            "value",
            DataType::Int64,
            false,
        )]));

        assert!(manager.write_run(&wrong_schema, &[input]).is_err());
        assert_eq!(manager.snapshot().current_bytes, 0);
        assert!(
            fs::read_dir(&manager.inner.directory)
                .unwrap()
                .next()
                .is_none()
        );
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn manager_directory_lives_until_its_runs_are_dropped() {
        let root = test_root("lifetime");
        let manager = SpillManager::new(&root, SPILL_LIMIT).unwrap();
        let directory = manager.inner.directory.clone();
        let input = batch();
        let run = manager.write_run(&input.schema(), &[input]).unwrap();
        drop(manager);

        assert!(directory.exists());
        drop(run);
        assert!(!directory.exists());
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn rejects_zero_byte_limit() {
        assert!(SpillManager::new(test_root("zero"), 0).is_err());
    }
}
