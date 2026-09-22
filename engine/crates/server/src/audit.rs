//! The audit ledger: an append-only record of who did what on the
//! coordinator — statements submitted, finished, failed, cancelled or
//! refused; catalog mutations; settings changes; authentication failures.
//!
//! Records are newline-delimited JSON in segments under the audit
//! directory (`KAVEON_AUDIT_DIR`, default `<state dir>/audit`), rotated
//! by size and retained by age. Writing is an in-memory enqueue on the
//! caller's path; a dedicated thread appends each batch and syncs it once,
//! so a clean shutdown loses nothing and a crash loses at most the batch
//! in flight. Reads flush first, then scan the segments in order.

use crate::security::{AuthSource, Identity, Role};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::VecDeque;
use std::io::{BufRead, BufWriter, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicI64, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, mpsc};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

pub const KIND_STATEMENT_SUBMITTED: &str = "statement.submitted";
pub const KIND_STATEMENT_FINISHED: &str = "statement.finished";
pub const KIND_STATEMENT_FAILED: &str = "statement.failed";
pub const KIND_STATEMENT_CANCELED: &str = "statement.canceled";
pub const KIND_STATEMENT_REJECTED: &str = "statement.rejected";
pub const KIND_CATALOG_CREATE: &str = "catalog.create";
pub const KIND_CATALOG_UPDATE: &str = "catalog.update";
pub const KIND_CATALOG_DELETE: &str = "catalog.delete";
pub const KIND_SETTINGS_RESOURCE_GROUPS: &str = "settings.resource_groups";
pub const KIND_SETTINGS_CACHE_CLEARED: &str = "settings.cache_cleared";
pub const KIND_AUTH_UNAUTHORIZED: &str = "auth.unauthorized";
pub const KIND_AUTH_FORBIDDEN: &str = "auth.forbidden";

/// Every kind the ledger writes, for the API's validation.
pub const KINDS: &[&str] = &[
    KIND_STATEMENT_SUBMITTED,
    KIND_STATEMENT_FINISHED,
    KIND_STATEMENT_FAILED,
    KIND_STATEMENT_CANCELED,
    KIND_STATEMENT_REJECTED,
    KIND_CATALOG_CREATE,
    KIND_CATALOG_UPDATE,
    KIND_CATALOG_DELETE,
    KIND_SETTINGS_RESOURCE_GROUPS,
    KIND_SETTINGS_CACHE_CLEARED,
    KIND_AUTH_UNAUTHORIZED,
    KIND_AUTH_FORBIDDEN,
];

/// How much of a statement's or an error's text a record keeps.
pub const TEXT_PREFIX_CHARS: usize = 200;
pub const DEFAULT_RETENTION_DAYS: u64 = 90;
pub const DEFAULT_SEGMENT_BYTES: u64 = 64 * 1024 * 1024;
const SEGMENT_PREFIX: &str = "audit-";
const SEGMENT_SUFFIX: &str = ".jsonl";
const CATALOG_CURSOR_FILE: &str = "catalog.cursor";
/// Records written per batch before the batch is synced.
const BATCH_RECORDS: usize = 4_096;
pub const MAX_PAGE: usize = 1_000;
pub const DEFAULT_PAGE: usize = 200;

/// One ledger line. Fields absent for a kind are omitted from the JSON.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct AuditRecord {
    /// The ledger's own sequence, increasing across segments and restarts.
    #[serde(default)]
    pub seq: u64,
    /// When the record was enqueued, Unix milliseconds.
    #[serde(default)]
    pub ts_ms: u64,
    pub kind: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub principal: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub role: Option<Role>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub auth: Option<AuthSource>,
    /// `METHOD /path`, for authentication failures.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub route: Option<String>,
    /// The query record this line is about; the record store keeps the
    /// rest (plan, stages, telemetry).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub query_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub client: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub client_tags: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub catalog: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub schema: Option<String>,
    /// SHA-256 of the statement text as submitted (after `SET SESSION`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub statement_sha256: Option<String>,
    /// The first 200 characters of the statement.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub statement: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resource_group: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub admission_wait_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub elapsed_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rows: Option<u64>,
    /// Compressed bytes the statement's scans read, across every task.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bytes_scanned: Option<u64>,
    /// Where the statement ran: `distributed`, `coordinator`, `cache`,
    /// `context`. Absent when it never ran.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mode: Option<String>,
    /// When the statement was charged against its resource group's
    /// statement quota — the moment it reached the row path. Absent when
    /// no quota applied to it, or it was answered without running. The
    /// quota is rebuilt from this field at start.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub quota_charged_at_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error_code: Option<String>,
    /// The first 200 characters of the error.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    /// Catalog mutations: `catalog`, `schema` or `table`, its id, and the
    /// revision before and after (absent before a create, after a delete).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub object_type: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub object_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub revision_before: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub revision_after: Option<u64>,
    /// What the kind needs beyond the fields above.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub details: Option<Value>,
}

impl AuditRecord {
    pub fn new(kind: &str) -> Self {
        Self {
            kind: kind.to_owned(),
            ..Self::default()
        }
    }

    pub fn by(mut self, identity: &Identity) -> Self {
        self.principal = Some(identity.principal.clone());
        self.role = Some(identity.role);
        self
    }

    /// A settings change by `identity`.
    pub fn settings(identity: &Identity, kind: &str, details: Value) -> Self {
        Self {
            details: Some(details),
            ..Self::new(kind).by(identity)
        }
    }

    /// An authentication failure on `route`; `principal` when the request
    /// named one.
    pub fn auth_failure(status: u16, principal: Option<&str>, route: String) -> Self {
        Self {
            principal: principal.map(str::to_owned),
            route: Some(route),
            error_code: Some(
                if status == 403 {
                    "FORBIDDEN"
                } else {
                    "UNAUTHORIZED"
                }
                .into(),
            ),
            ..Self::new(if status == 403 {
                KIND_AUTH_FORBIDDEN
            } else {
                KIND_AUTH_UNAUTHORIZED
            })
        }
    }

    /// A catalog store event as a ledger line: revisions are consecutive,
    /// so the revision before an update is the one below.
    pub fn catalog(event: &kaveon_catalog::AuditEvent) -> Self {
        let revision = event.revision.value();
        let (kind, before, after) = match event.action.as_str() {
            "create" => (KIND_CATALOG_CREATE, None, Some(revision)),
            "delete" => (KIND_CATALOG_DELETE, Some(revision), None),
            _ => (
                KIND_CATALOG_UPDATE,
                Some(revision.saturating_sub(1)),
                Some(revision),
            ),
        };
        Self {
            ts_ms: event.occurred_at_unix_ms,
            principal: Some(event.actor.clone()),
            object_type: Some(event.object_type.clone()),
            object_id: Some(event.object_id.clone()),
            revision_before: before,
            revision_after: after,
            details: (!event.details.is_empty()).then(|| {
                Value::Object(
                    event
                        .details
                        .iter()
                        .map(|(key, value)| (key.clone(), Value::from(value.as_str())))
                        .collect(),
                )
            }),
            ..Self::new(kind)
        }
    }
}

/// The first `TEXT_PREFIX_CHARS` characters of `text`.
pub fn text_prefix(text: &str) -> String {
    text.chars().take(TEXT_PREFIX_CHARS).collect()
}

pub fn sha256_hex(text: &str) -> String {
    use sha2::{Digest, Sha256};
    let digest = Sha256::digest(text.as_bytes());
    let mut hex = String::with_capacity(64);
    for byte in digest {
        use std::fmt::Write as _;
        let _ = write!(hex, "{byte:02x}");
    }
    hex
}

pub fn unix_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

/// What `GET /v1/audit` filters by.
#[derive(Clone, Debug, Default)]
pub struct AuditFilter {
    pub since_ms: Option<u64>,
    pub until_ms: Option<u64>,
    pub principal: Option<String>,
    /// Exact kinds, or families (`statement`, `catalog`, `settings`,
    /// `auth`); empty matches every kind.
    pub kinds: Vec<String>,
    pub query_id: Option<String>,
    /// Records with `seq` above this.
    pub after_seq: Option<u64>,
}

impl AuditFilter {
    fn matches(&self, record: &AuditRecord) -> bool {
        self.after_seq.is_none_or(|after| record.seq > after)
            && self.since_ms.is_none_or(|since| record.ts_ms >= since)
            && self.until_ms.is_none_or(|until| record.ts_ms <= until)
            && self
                .principal
                .as_deref()
                .is_none_or(|principal| record.principal.as_deref() == Some(principal))
            && self
                .query_id
                .as_deref()
                .is_none_or(|id| record.query_id.as_deref() == Some(id))
            && (self.kinds.is_empty()
                || self.kinds.iter().any(|kind| {
                    record.kind == *kind
                        || record
                            .kind
                            .strip_prefix(kind.as_str())
                            .is_some_and(|rest| rest.starts_with('.'))
                }))
    }
}

/// One page of matching records, oldest first, and where the next page
/// starts.
#[derive(Debug, Default, Serialize)]
pub struct AuditPage {
    pub records: Vec<AuditRecord>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub next_cursor: Option<u64>,
}

enum Command {
    Record(Box<AuditRecord>),
    Flush(mpsc::SyncSender<()>),
}

struct Inner {
    dir: PathBuf,
    segment_bytes: u64,
    retention: Duration,
    next_seq: AtomicU64,
    /// Where the catalog store's audit events were last read up to.
    catalog_cursor: AtomicI64,
    sender: Mutex<Option<mpsc::Sender<Command>>>,
    writer: Mutex<Option<std::thread::JoinHandle<()>>>,
}

/// The ledger handle. Cloning shares the ledger; a disabled ledger
/// (workers, tests without a directory) accepts and drops every record.
#[derive(Clone)]
pub struct AuditLedger {
    inner: Option<Arc<Inner>>,
}

impl AuditLedger {
    pub fn disabled() -> Self {
        Self { inner: None }
    }

    /// Opens the ledger under `dir`, continuing the sequence from the last
    /// record on disk and applying the retention at once.
    pub fn open(dir: &Path, segment_bytes: u64, retention: Duration) -> std::io::Result<Self> {
        std::fs::create_dir_all(dir)?;
        let last_seq = last_sequence(dir)?;
        let catalog_cursor = std::fs::read_to_string(dir.join(CATALOG_CURSOR_FILE))
            .ok()
            .and_then(|text| text.trim().parse::<i64>().ok())
            .unwrap_or(0);
        let (sender, receiver) = mpsc::channel();
        let inner = Arc::new(Inner {
            dir: dir.to_path_buf(),
            segment_bytes: segment_bytes.max(1),
            retention,
            next_seq: AtomicU64::new(last_seq + 1),
            catalog_cursor: AtomicI64::new(catalog_cursor),
            sender: Mutex::new(Some(sender)),
            writer: Mutex::new(None),
        });
        let mut writer = SegmentWriter::open(&inner.dir, inner.segment_bytes)?;
        let thread = std::thread::Builder::new()
            .name("kaveon-audit".into())
            .spawn(move || write_loop(&receiver, &mut writer))?;
        *inner.writer.lock().unwrap_or_else(|p| p.into_inner()) = Some(thread);
        let ledger = Self { inner: Some(inner) };
        ledger.enforce_retention();
        Ok(ledger)
    }

    pub fn is_enabled(&self) -> bool {
        self.inner.is_some()
    }

    pub fn directory(&self) -> Option<&Path> {
        self.inner.as_ref().map(|inner| inner.dir.as_path())
    }

    /// Enqueues one record: the sequence and the time are assigned here,
    /// the write happens on the ledger's thread.
    pub fn record(&self, mut record: AuditRecord) {
        let Some(inner) = &self.inner else {
            return;
        };
        record.seq = inner.next_seq.fetch_add(1, Ordering::AcqRel);
        if record.ts_ms == 0 {
            record.ts_ms = unix_ms();
        }
        if let Some(sender) = inner
            .sender
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .as_ref()
        {
            // A closed receiver means the ledger has shut down: the process
            // is stopping and the record has nowhere durable to go.
            let _ = sender.send(Command::Record(Box::new(record)));
        }
    }

    /// Blocks until everything enqueued so far is on disk and synced.
    pub fn flush(&self) {
        let Some(inner) = &self.inner else {
            return;
        };
        let (done, wait) = mpsc::sync_channel(1);
        let sent = inner
            .sender
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .as_ref()
            .is_some_and(|sender| sender.send(Command::Flush(done)).is_ok());
        if sent {
            let _ = wait.recv();
        }
    }

    /// Writes what is enqueued and stops the writer. Called on a clean
    /// shutdown; records enqueued afterwards are dropped.
    pub fn shutdown(&self) {
        let Some(inner) = &self.inner else {
            return;
        };
        inner
            .sender
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .take();
        if let Some(thread) = inner
            .writer
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .take()
        {
            let _ = thread.join();
        }
    }

    /// Removes every segment whose records are all older than the
    /// retention; the open segment stays.
    pub fn enforce_retention(&self) {
        let Some(inner) = &self.inner else {
            return;
        };
        let Ok(segments) = list_segments(&inner.dir) else {
            return;
        };
        let cutoff = unix_ms().saturating_sub(inner.retention.as_millis() as u64);
        for pair in segments.windows(2) {
            // Every record of `pair[0]` precedes the first of `pair[1]`.
            if pair[1].first_ts_ms < cutoff {
                let _ = std::fs::remove_file(&pair[0].path);
            }
        }
    }

    /// Reads the catalog store's audit events past the ledger's cursor
    /// and appends them; the cursor is written after the records are
    /// enqueued so a restart re-reads at most the last drained batch.
    pub fn drain_catalog(&self, store: &kaveon_catalog::CatalogStore) {
        let Some(inner) = &self.inner else {
            return;
        };
        let mut after = inner.catalog_cursor.load(Ordering::Acquire);
        loop {
            let Ok(events) = store.audit_events(Some(after), 512) else {
                return;
            };
            if events.is_empty() {
                break;
            }
            for event in &events {
                self.record(AuditRecord::catalog(event));
                after = after.max(event.id);
            }
        }
        if after != inner.catalog_cursor.swap(after, Ordering::AcqRel) {
            let _ = std::fs::write(inner.dir.join(CATALOG_CURSOR_FILE), after.to_string());
        }
    }

    /// Positions the catalog cursor at the store's newest event without
    /// emitting history: the ledger starts at its own start.
    pub fn skip_catalog_history(&self, store: &kaveon_catalog::CatalogStore) {
        let Some(inner) = &self.inner else {
            return;
        };
        if inner.dir.join(CATALOG_CURSOR_FILE).exists() {
            return;
        }
        let mut after = 0;
        loop {
            let Ok(events) = store.audit_events(Some(after), 512) else {
                return;
            };
            match events.last() {
                Some(last) => after = last.id,
                None => break,
            }
        }
        inner.catalog_cursor.store(after, Ordering::Release);
        let _ = std::fs::write(inner.dir.join(CATALOG_CURSOR_FILE), after.to_string());
    }

    /// One page of records matching `filter`, oldest first: at most
    /// `limit`, and `next_cursor` when more follow. Flushes first, so a
    /// record enqueued before the call is visible.
    pub fn query(&self, filter: &AuditFilter, limit: usize) -> std::io::Result<AuditPage> {
        let Some(inner) = &self.inner else {
            return Ok(AuditPage::default());
        };
        self.flush();
        let limit = limit.clamp(1, MAX_PAGE);
        let segments = list_segments(&inner.dir)?;
        let mut page = AuditPage::default();
        for (index, segment) in segments.iter().enumerate() {
            // A segment whose successor starts before `since` holds nothing
            // in range.
            if let Some(since) = filter.since_ms
                && segments
                    .get(index + 1)
                    .is_some_and(|next| next.first_ts_ms < since)
            {
                continue;
            }
            if filter
                .until_ms
                .is_some_and(|until| segment.first_ts_ms > until)
            {
                break;
            }
            let file = std::fs::File::open(&segment.path)?;
            for line in std::io::BufReader::new(file).lines() {
                let line = line?;
                let Ok(record) = serde_json::from_str::<AuditRecord>(&line) else {
                    continue;
                };
                if !filter.matches(&record) {
                    continue;
                }
                if page.records.len() == limit {
                    page.next_cursor = page.records.last().map(|last| last.seq);
                    return Ok(page);
                }
                page.records.push(record);
            }
        }
        Ok(page)
    }
}

struct Segment {
    path: PathBuf,
    first_ts_ms: u64,
}

/// The segments in order of their first record.
fn list_segments(dir: &Path) -> std::io::Result<Vec<Segment>> {
    let mut segments = Vec::new();
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let name = entry.file_name();
        let Some(name) = name.to_str() else {
            continue;
        };
        let Some(stem) = name
            .strip_prefix(SEGMENT_PREFIX)
            .and_then(|rest| rest.strip_suffix(SEGMENT_SUFFIX))
        else {
            continue;
        };
        let Some((ts, _seq)) = stem.split_once('-') else {
            continue;
        };
        let Ok(first_ts_ms) = ts.parse::<u64>() else {
            continue;
        };
        segments.push(Segment {
            path: entry.path(),
            first_ts_ms,
        });
    }
    segments.sort_by(|a, b| a.path.cmp(&b.path));
    Ok(segments)
}

/// The `seq` of the last record on disk, zero for an empty ledger.
fn last_sequence(dir: &Path) -> std::io::Result<u64> {
    let segments = list_segments(dir)?;
    for segment in segments.iter().rev() {
        let file = std::fs::File::open(&segment.path)?;
        let mut last = None;
        for line in std::io::BufReader::new(file).lines() {
            let line = line?;
            if let Ok(record) = serde_json::from_str::<AuditRecord>(&line) {
                last = Some(record.seq);
            }
        }
        if let Some(seq) = last {
            return Ok(seq);
        }
    }
    Ok(0)
}

struct SegmentWriter {
    dir: PathBuf,
    segment_bytes: u64,
    file: Option<BufWriter<std::fs::File>>,
    written: u64,
}

impl SegmentWriter {
    /// Continues the newest segment when it has room, else starts fresh
    /// at the first record.
    fn open(dir: &Path, segment_bytes: u64) -> std::io::Result<Self> {
        let mut writer = Self {
            dir: dir.to_path_buf(),
            segment_bytes,
            file: None,
            written: 0,
        };
        if let Some(last) = list_segments(dir)?.pop() {
            let size = std::fs::metadata(&last.path)?.len();
            if size < segment_bytes {
                let file = std::fs::OpenOptions::new().append(true).open(&last.path)?;
                writer.file = Some(BufWriter::new(file));
                writer.written = size;
            }
        }
        Ok(writer)
    }

    fn write(&mut self, record: &AuditRecord) -> std::io::Result<()> {
        if self.file.is_none() || self.written >= self.segment_bytes {
            self.rotate(record)?;
        }
        let file = self.file.as_mut().expect("a segment is open");
        let line = serde_json::to_vec(record)?;
        file.write_all(&line)?;
        file.write_all(b"\n")?;
        self.written += line.len() as u64 + 1;
        Ok(())
    }

    fn rotate(&mut self, first: &AuditRecord) -> std::io::Result<()> {
        self.sync()?;
        let path = self.dir.join(format!(
            "{SEGMENT_PREFIX}{:013}-{:012}{SEGMENT_SUFFIX}",
            first.ts_ms, first.seq
        ));
        let file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)?;
        self.file = Some(BufWriter::new(file));
        self.written = 0;
        Ok(())
    }

    fn sync(&mut self) -> std::io::Result<()> {
        if let Some(file) = self.file.as_mut() {
            file.flush()?;
            file.get_ref().sync_data()?;
        }
        Ok(())
    }
}

/// The writer thread: one batch per wake-up, synced once.
fn write_loop(receiver: &mpsc::Receiver<Command>, writer: &mut SegmentWriter) {
    let mut pending: VecDeque<Command> = VecDeque::new();
    while let Ok(first) = receiver.recv() {
        pending.push_back(first);
        while pending.len() < BATCH_RECORDS {
            match receiver.try_recv() {
                Ok(command) => pending.push_back(command),
                Err(_) => break,
            }
        }
        let mut flushes = Vec::new();
        for command in pending.drain(..) {
            match command {
                Command::Record(record) => {
                    // A write that fails is retried with the next batch's
                    // sync; the record itself is lost only if the disk is.
                    let _ = writer.write(&record);
                }
                Command::Flush(done) => flushes.push(done),
            }
        }
        let _ = writer.sync();
        for done in flushes {
            let _ = done.send(());
        }
    }
    let _ = writer.sync();
}

/// A time as the API accepts it: Unix milliseconds, `YYYY-MM-DD`, or
/// `YYYY-MM-DDTHH:MM:SS[.fff]Z` (UTC).
pub fn parse_time(text: &str) -> Option<u64> {
    let text = text.trim();
    if let Ok(ms) = text.parse::<u64>() {
        return Some(ms);
    }
    let (date, time) = match text.split_once('T') {
        Some((date, time)) => (date, Some(time.strip_suffix('Z').unwrap_or(time))),
        None => (text, None),
    };
    let mut parts = date.split('-');
    let year: i64 = parts.next()?.parse().ok()?;
    let month: u32 = parts.next()?.parse().ok()?;
    let day: u32 = parts.next()?.parse().ok()?;
    if parts.next().is_some() || !(1..=12).contains(&month) || !(1..=31).contains(&day) {
        return None;
    }
    let days = days_from_civil(year, month, day);
    let mut ms = u64::try_from(days).ok()? * 86_400_000;
    if let Some(time) = time {
        let mut clock = time.split(':');
        let hours: u64 = clock.next()?.parse().ok()?;
        let minutes: u64 = clock.next()?.parse().ok()?;
        let seconds = clock.next()?;
        if clock.next().is_some() || hours > 23 || minutes > 59 {
            return None;
        }
        let (whole, fraction) = seconds.split_once('.').unwrap_or((seconds, ""));
        let whole: u64 = whole.parse().ok()?;
        if whole > 60 {
            return None;
        }
        let millis = if fraction.is_empty() {
            0
        } else {
            let digits: String = fraction.chars().take(3).collect();
            let value: u64 = digits.parse().ok()?;
            value * 10u64.pow(3 - digits.len() as u32)
        };
        ms += hours * 3_600_000 + minutes * 60_000 + whole * 1_000 + millis;
    }
    Some(ms)
}

/// A time as the API reports it: `YYYY-MM-DDTHH:MM:SSZ` (UTC, whole
/// seconds) from Unix milliseconds. The inverse of [`parse_time`] at
/// second precision.
pub fn format_time(ms: u64) -> String {
    let seconds = ms / 1_000;
    let days = i64::try_from(seconds / 86_400).unwrap_or(i64::MAX);
    let (year, month, day) = civil_from_days(days);
    let clock = seconds % 86_400;
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}Z",
        clock / 3_600,
        clock % 3_600 / 60,
        clock % 60
    )
}

/// The proleptic Gregorian date of a day count since 1970-01-01.
fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let days = days + 719_468;
    let era = days.div_euclid(146_097);
    let day_of_era = days.rem_euclid(146_097);
    let year_of_era =
        (day_of_era - day_of_era / 1_460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_index = (5 * day_of_year + 2) / 153;
    let day = (day_of_year - (153 * month_index + 2) / 5 + 1) as u32;
    let month = if month_index < 10 {
        month_index + 3
    } else {
        month_index - 9
    } as u32;
    let year = year_of_era + era * 400 + i64::from(month <= 2);
    (year, month, day)
}

/// Days since 1970-01-01 for a proleptic Gregorian date.
fn days_from_civil(year: i64, month: u32, day: u32) -> i64 {
    let year = if month <= 2 { year - 1 } else { year };
    let era = year.div_euclid(400);
    let year_of_era = year.rem_euclid(400);
    let month_index = i64::from((month + 9) % 12);
    let day_of_year = (153 * month_index + 2) / 5 + i64::from(day) - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    era * 146_097 + day_of_era - 719_468
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temporary_directory() -> PathBuf {
        let path = std::env::temp_dir().join(format!("kaveon-audit-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&path).unwrap();
        path
    }

    fn record(kind: &str, principal: &str) -> AuditRecord {
        AuditRecord {
            principal: Some(principal.into()),
            ..AuditRecord::new(kind)
        }
    }

    #[test]
    fn records_are_sequenced_paged_and_filtered_and_the_sequence_survives_a_reopen() {
        let directory = temporary_directory();
        let ledger = AuditLedger::open(&directory, 1 << 20, Duration::from_secs(86_400)).unwrap();
        for index in 0..7 {
            let principal = if index % 2 == 0 { "alice" } else { "bob" };
            let kind = if index < 5 {
                KIND_STATEMENT_FINISHED
            } else {
                KIND_CATALOG_CREATE
            };
            ledger.record(AuditRecord {
                query_id: Some(format!("q{index}")),
                ..record(kind, principal)
            });
        }
        let page = ledger.query(&AuditFilter::default(), 3).unwrap();
        assert_eq!(
            page.records.iter().map(|r| r.seq).collect::<Vec<_>>(),
            vec![1, 2, 3]
        );
        assert_eq!(page.next_cursor, Some(3));
        let page = ledger
            .query(
                &AuditFilter {
                    after_seq: page.next_cursor,
                    ..AuditFilter::default()
                },
                100,
            )
            .unwrap();
        assert_eq!(page.records.len(), 4);
        assert_eq!(page.next_cursor, None);
        let alice = ledger
            .query(
                &AuditFilter {
                    principal: Some("alice".into()),
                    kinds: vec!["statement".into()],
                    ..AuditFilter::default()
                },
                100,
            )
            .unwrap();
        assert_eq!(
            alice
                .records
                .iter()
                .map(|r| r.query_id.clone().unwrap())
                .collect::<Vec<_>>(),
            vec!["q0", "q2", "q4"]
        );
        let catalog = ledger
            .query(
                &AuditFilter {
                    kinds: vec![KIND_CATALOG_CREATE.into()],
                    ..AuditFilter::default()
                },
                100,
            )
            .unwrap();
        assert_eq!(catalog.records.len(), 2);
        let one = ledger
            .query(
                &AuditFilter {
                    query_id: Some("q3".into()),
                    ..AuditFilter::default()
                },
                100,
            )
            .unwrap();
        assert_eq!(one.records.len(), 1);
        ledger.shutdown();
        // Reopened: the sequence continues, nothing is lost.
        let reopened = AuditLedger::open(&directory, 1 << 20, Duration::from_secs(86_400)).unwrap();
        reopened.record(record(KIND_AUTH_FORBIDDEN, "carol"));
        let all = reopened.query(&AuditFilter::default(), 100).unwrap();
        assert_eq!(all.records.len(), 8);
        assert_eq!(all.records.last().unwrap().seq, 8);
        reopened.shutdown();
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn segments_rotate_by_size_and_old_ones_are_removed_by_the_retention() {
        let directory = temporary_directory();
        // Tiny segments: every record starts a new one.
        let ledger = AuditLedger::open(&directory, 1, Duration::from_millis(50)).unwrap();
        let old = unix_ms() - 10_000;
        for index in 0..3 {
            ledger.record(AuditRecord {
                ts_ms: old + index,
                ..record(KIND_STATEMENT_SUBMITTED, "alice")
            });
        }
        ledger.flush();
        assert_eq!(list_segments(&directory).unwrap().len(), 3);
        ledger.record(record(KIND_STATEMENT_SUBMITTED, "alice"));
        ledger.flush();
        assert_eq!(list_segments(&directory).unwrap().len(), 4);
        // The three old segments each end before a segment that started
        // before the cutoff; the newest stays whatever its age.
        std::thread::sleep(Duration::from_millis(60));
        ledger.enforce_retention();
        let remaining = list_segments(&directory).unwrap();
        assert_eq!(remaining.len(), 1);
        let page = ledger.query(&AuditFilter::default(), 100).unwrap();
        assert_eq!(page.records.len(), 1);
        assert_eq!(page.records[0].seq, 4);
        // A time window skips segments outside it.
        ledger.shutdown();
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn a_time_window_and_a_disabled_ledger_behave() {
        let directory = temporary_directory();
        let ledger = AuditLedger::open(&directory, 1 << 20, Duration::from_secs(86_400)).unwrap();
        for ts in [1_000u64, 2_000, 3_000] {
            ledger.record(AuditRecord {
                ts_ms: ts,
                ..record(KIND_SETTINGS_CACHE_CLEARED, "admin")
            });
        }
        let window = ledger
            .query(
                &AuditFilter {
                    since_ms: Some(1_500),
                    until_ms: Some(2_500),
                    ..AuditFilter::default()
                },
                10,
            )
            .unwrap();
        assert_eq!(window.records.len(), 1);
        assert_eq!(window.records[0].ts_ms, 2_000);
        ledger.shutdown();
        let disabled = AuditLedger::disabled();
        disabled.record(record(KIND_AUTH_UNAUTHORIZED, "nobody"));
        assert!(
            disabled
                .query(&AuditFilter::default(), 10)
                .unwrap()
                .records
                .is_empty()
        );
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn times_parse_as_milliseconds_dates_and_utc_timestamps() {
        assert_eq!(parse_time("1700000000000"), Some(1_700_000_000_000));
        assert_eq!(parse_time("1970-01-02"), Some(86_400_000));
        assert_eq!(parse_time("2026-09-19"), Some(1_789_776_000_000));
        assert_eq!(
            parse_time("2026-09-19T10:20:30.250Z"),
            Some(1_789_776_000_000 + 10 * 3_600_000 + 20 * 60_000 + 30_000 + 250)
        );
        assert_eq!(
            parse_time("2026-09-19T10:20:30Z"),
            Some(1_789_776_000_000 + 37_230_000)
        );
        assert_eq!(parse_time("2026-13-01"), None);
        assert_eq!(parse_time("yesterday"), None);
    }

    #[test]
    fn times_format_as_utc_timestamps_and_round_trip_through_parse() {
        assert_eq!(format_time(0), "1970-01-01T00:00:00Z");
        assert_eq!(format_time(86_400_000 - 1), "1970-01-01T23:59:59Z");
        assert_eq!(
            format_time(1_789_776_000_000 + 10 * 3_600_000 + 20 * 60_000 + 30_250),
            "2026-09-19T10:20:30Z"
        );
        assert_eq!(format_time(951_782_400_000), "2000-02-29T00:00:00Z");
        for text in [
            "2026-09-19T10:20:30Z",
            "2000-02-29T23:59:59Z",
            "1999-12-31T00:00:00Z",
            "2100-03-01T12:00:00Z",
        ] {
            assert_eq!(format_time(parse_time(text).unwrap()), text);
        }
    }

    #[test]
    fn a_catalog_event_becomes_a_line_with_the_revisions_around_it() {
        let event = kaveon_catalog::AuditEvent {
            id: 7,
            occurred_at_unix_ms: 123,
            actor: "alice".into(),
            action: "update".into(),
            object_type: "table".into(),
            object_id: "t1".into(),
            revision: kaveon_core::CatalogRevision::new(4).unwrap(),
            details: Default::default(),
        };
        let line = AuditRecord::catalog(&event);
        assert_eq!(line.kind, KIND_CATALOG_UPDATE);
        assert_eq!(
            (line.revision_before, line.revision_after),
            (Some(3), Some(4))
        );
        assert_eq!(line.principal.as_deref(), Some("alice"));
        assert_eq!(line.ts_ms, 123);
        let deleted = AuditRecord::catalog(&kaveon_catalog::AuditEvent {
            action: "delete".into(),
            ..event
        });
        assert_eq!(
            (deleted.revision_before, deleted.revision_after),
            (Some(4), None)
        );
    }
}
