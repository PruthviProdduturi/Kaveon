//! Immutable, owner-scoped result pages with process/query disk quotas and TTL.
//!
//! A paged statement's pages are served while the statement still runs: the
//! writer is registered in the store when it is created (`begin`), every page
//! it flushes is readable as soon as its file is on disk, and `publish` marks
//! the entry complete. A writer that is dropped without `publish`, or an
//! in-progress entry that is removed, leaves a tombstone for the TTL so that a
//! client already following the pages sees `410 Gone` rather than a `404` it
//! cannot tell from an expired result.
use crate::security::Identity;
use axum::http::StatusCode;
use serde_json::Value;
use std::{
    collections::HashMap,
    fs,
    path::PathBuf,
    sync::{
        Arc, Mutex,
        atomic::{AtomicU8, AtomicU64, AtomicUsize, Ordering},
    },
    time::{Duration, Instant},
};

const PAGE_ROWS: usize = 1_000;
const PAGE_BYTES: usize = 4 * 1024 * 1024;
/// The defaults of `KAVEON_RESULT_QUERY_DISK_LIMIT_BYTES` and
/// `KAVEON_RESULT_DISK_LIMIT_BYTES`.
pub const DEFAULT_QUERY_BYTES: u64 = 256 * 1024 * 1024;
pub const DEFAULT_PROCESS_BYTES: u64 = 1024 * 1024 * 1024;
const TTL: Duration = Duration::from_secs(900);
const RETAINED_RESULTS: usize = 100;

const IN_PROGRESS: u8 = 0;
const COMPLETE: u8 = 1;
const ABORTED: u8 = 2;

pub struct ResultStore {
    results: Mutex<HashMap<String, Entry>>,
    used: Arc<AtomicU64>,
    /// One result's disk, and the process's across results.
    query_bytes: u64,
    process_bytes: u64,
}

impl Default for ResultStore {
    fn default() -> Self {
        Self::with_limits(DEFAULT_QUERY_BYTES, DEFAULT_PROCESS_BYTES)
    }
}
struct Entry {
    owner: String,
    expires: Instant,
    pages: Arc<Pages>,
}
/// The on-disk pages of one result, shared by the store's entry and the
/// writer that produces them. Readers observe `pages` with `Acquire` after
/// the writer stored it with `Release` behind the page file's write, so a
/// page count never names a file that is not fully on disk.
struct Pages {
    directory: PathBuf,
    used: Arc<AtomicU64>,
    query_bytes: u64,
    process_bytes: u64,
    bytes: AtomicU64,
    pages: AtomicUsize,
    rows: AtomicUsize,
    state: AtomicU8,
}
pub struct ResultWriter {
    shared: Arc<Pages>,
    page: Vec<Vec<Value>>,
    page_bytes: usize,
    published: bool,
    pub rows: usize,
}
/// One page lookup that did not fail.
#[derive(Debug, PartialEq)]
pub enum ResultPage {
    /// `200`: the page's rows, with `next_uri`, `row_count` and `complete`.
    Ready(Value),
    /// `202`: the writer is still running and this is the next page it has
    /// not flushed yet; the body carries `id`, `row_count` and `complete`.
    Pending(Value),
}
impl ResultStore {
    /// A store whose results may take `query_bytes` each and
    /// `process_bytes` together (`KAVEON_RESULT_QUERY_DISK_LIMIT_BYTES`,
    /// `KAVEON_RESULT_DISK_LIMIT_BYTES`).
    pub fn with_limits(query_bytes: u64, process_bytes: u64) -> Self {
        Self {
            results: Mutex::new(HashMap::new()),
            used: Arc::new(AtomicU64::new(0)),
            query_bytes,
            process_bytes,
        }
    }

    /// Removes a result. A complete result is forgotten outright; an
    /// in-progress one becomes a tombstone (`410 Gone` for the TTL) and its
    /// disk is released now, so a still-running writer fails on its next
    /// page rather than writing into a directory nobody will read.
    pub fn remove(&self, id: &str) {
        if let Ok(mut entries) = self.results.lock()
            && let Some(entry) = entries.get(id)
        {
            match entry.pages.state.load(Ordering::Acquire) {
                IN_PROGRESS => entry.pages.abort(),
                COMPLETE => {
                    entries.remove(id);
                }
                _ => {}
            }
        }
    }
    /// Whether a live (in-progress or complete) result is registered.
    pub fn contains(&self, id: &str) -> bool {
        self.results.lock().is_ok_and(|entries| {
            entries
                .get(id)
                .is_some_and(|entry| entry.pages.state.load(Ordering::Acquire) != ABORTED)
        })
    }
    /// Registers an in-progress result for `owner` and returns its writer.
    /// Pages the writer flushes are readable through `page` immediately.
    pub fn begin(&self, id: &str, owner: &str) -> std::io::Result<ResultWriter> {
        self.cleanup();
        let mut results = self
            .results
            .lock()
            .map_err(|_| std::io::Error::other("result store unavailable"))?;
        let live = results
            .values()
            .filter(|entry| entry.pages.state.load(Ordering::Acquire) != ABORTED)
            .count();
        if live >= RETAINED_RESULTS {
            return Err(std::io::Error::other(
                "result retention limit reached; retry after expiration",
            ));
        }
        let directory =
            std::env::temp_dir().join(format!("kaveon-result-{}", uuid::Uuid::new_v4()));
        let builder = fs::DirBuilder::new();
        #[cfg(unix)]
        let mut builder = builder;
        #[cfg(unix)]
        {
            use std::os::unix::fs::DirBuilderExt;
            builder.mode(0o700);
        }
        builder.create(&directory)?;
        let shared = Arc::new(Pages {
            directory,
            used: self.used.clone(),
            query_bytes: self.query_bytes,
            process_bytes: self.process_bytes,
            bytes: AtomicU64::new(0),
            pages: AtomicUsize::new(0),
            rows: AtomicUsize::new(0),
            state: AtomicU8::new(IN_PROGRESS),
        });
        results.insert(
            id.into(),
            Entry {
                owner: owner.into(),
                expires: Instant::now() + TTL,
                pages: Arc::clone(&shared),
            },
        );
        Ok(ResultWriter {
            shared,
            page: Vec::new(),
            page_bytes: 0,
            published: false,
            rows: 0,
        })
    }
    /// Flushes the writer's tail page and marks the result complete. Fails
    /// when the result was removed while it was being written.
    pub fn publish(&self, id: &str, mut writer: ResultWriter) -> std::io::Result<()> {
        let mut results = self
            .results
            .lock()
            .map_err(|_| std::io::Error::other("result store unavailable"))?;
        // The tail page lands under the lock `page` holds: a reader never
        // sees the final page count beside an in-progress state, which would
        // hand out a `next_uri` that answers 404 once the state catches up.
        writer.flush()?;
        let entry = results
            .get_mut(id)
            .filter(|entry| Arc::ptr_eq(&entry.pages, &writer.shared))
            .ok_or_else(|| std::io::Error::other("result was removed before it completed"))?;
        if entry
            .pages
            .state
            .compare_exchange(IN_PROGRESS, COMPLETE, Ordering::AcqRel, Ordering::Acquire)
            .is_err()
        {
            return Err(std::io::Error::other(
                "result was removed before it completed",
            ));
        }
        entry.expires = Instant::now() + TTL;
        writer.published = true;
        Ok(())
    }
    pub fn page(
        &self,
        id: &str,
        index: usize,
        identity: &Identity,
    ) -> Result<ResultPage, StatusCode> {
        self.cleanup();
        let entries = self
            .results
            .lock()
            .map_err(|_| StatusCode::SERVICE_UNAVAILABLE)?;
        let entry = entries
            .get(id)
            .filter(|entry| identity.can_view(Some(&entry.owner)))
            .ok_or(StatusCode::NOT_FOUND)?;
        let pages = &entry.pages;
        // Under the lock the state and the count agree: `publish` flushes the
        // tail page and marks the entry complete while holding it.
        let state = pages.state.load(Ordering::Acquire);
        if state == ABORTED {
            return Err(StatusCode::GONE);
        }
        let complete = state == COMPLETE;
        let flushed = pages.pages.load(Ordering::Acquire);
        let rows = pages.rows.load(Ordering::Acquire);
        if index >= flushed {
            if !complete && index == flushed {
                return Ok(ResultPage::Pending(
                    serde_json::json!({"id": id, "row_count": rows, "complete": false}),
                ));
            }
            return Err(StatusCode::NOT_FOUND);
        }
        let data: Value = serde_json::from_reader(
            fs::File::open(pages.directory.join(format!("{index}.json")))
                .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?,
        )
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
        let next = (index + 1 < flushed || !complete)
            .then(|| format!("/v1/query/{id}/results/{}", index + 1));
        Ok(ResultPage::Ready(serde_json::json!({
            "id": id,
            "data": data,
            "next_uri": next,
            "row_count": rows,
            "complete": complete,
        })))
    }
    pub fn cleanup(&self) {
        if let Ok(mut entries) = self.results.lock() {
            entries.retain(|_, entry| entry.expires > Instant::now());
        }
    }
}
impl Pages {
    /// Marks the result gone and releases its disk; a later `publish` fails.
    fn abort(&self) {
        self.state.store(ABORTED, Ordering::Release);
        self.release();
    }
    /// Removes the page files and returns their bytes to the process quota.
    /// Idempotent, and repeated on the writer's drop after a removal so a
    /// page the writer was still putting down does not outlive its result.
    fn release(&self) {
        let _ = fs::remove_dir_all(&self.directory);
        let bytes = self.bytes.swap(0, Ordering::AcqRel);
        self.used.fetch_sub(bytes, Ordering::AcqRel);
    }
}
impl Drop for Pages {
    fn drop(&mut self) {
        self.release();
    }
}
impl ResultWriter {
    pub fn push(&mut self, row: Vec<Value>) -> std::io::Result<()> {
        let bytes = serde_json::to_vec(&row)?.len();
        if bytes > PAGE_BYTES {
            return Err(std::io::Error::other(
                "single result row exceeds 4 MiB page limit",
            ));
        }
        if !self.page.is_empty()
            && (self.page.len() >= PAGE_ROWS
                || self.page_bytes + bytes + self.page.len() + 2 > PAGE_BYTES)
        {
            self.flush()?;
        }
        self.page_bytes += bytes;
        self.rows += 1;
        self.page.push(row);
        Ok(())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        let shared = &self.shared;
        let flushed = shared.pages.load(Ordering::Acquire);
        if self.page.is_empty() && flushed > 0 {
            return Ok(());
        }
        if shared.state.load(Ordering::Acquire) == ABORTED {
            return Err(std::io::Error::other(
                "result was removed before it completed",
            ));
        }
        let encoded = serde_json::to_vec(&self.page)?;
        let bytes = encoded.len() as u64;
        if shared.bytes.load(Ordering::Acquire) + bytes > shared.query_bytes {
            return Err(std::io::Error::other(format!(
                "query result disk quota exceeded ({}); raise KAVEON_RESULT_QUERY_DISK_LIMIT_BYTES on the coordinator, or narrow the result",
                mebibytes(shared.query_bytes)
            )));
        }
        shared
            .used
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |used| {
                used.checked_add(bytes)
                    .filter(|total| *total <= shared.process_bytes)
            })
            .map_err(|_| {
                std::io::Error::other(format!(
                    "process result disk quota exceeded ({}); raise KAVEON_RESULT_DISK_LIMIT_BYTES on the coordinator, or wait for results to expire",
                    mebibytes(shared.process_bytes)
                ))
            })?;
        shared.bytes.fetch_add(bytes, Ordering::AcqRel);
        fs::write(shared.directory.join(format!("{flushed}.json")), encoded)?;
        // Rows before pages: a reader that sees the new count sees at least
        // the rows it holds.
        shared.rows.fetch_add(self.page.len(), Ordering::AcqRel);
        shared.pages.store(flushed + 1, Ordering::Release);
        self.page.clear();
        self.page_bytes = 0;
        Ok(())
    }
}
impl Drop for ResultWriter {
    fn drop(&mut self) {
        if !self.published {
            self.shared.abort();
        }
    }
}

/// `256 MiB`, `1.5 GiB`: the quota as people set it.
fn mebibytes(bytes: u64) -> String {
    const GIB: u64 = 1024 * 1024 * 1024;
    const MIB: u64 = 1024 * 1024;
    if bytes >= GIB && bytes.is_multiple_of(GIB / 2) {
        let whole = bytes / GIB;
        let half = (bytes % GIB) / (GIB / 2);
        if half == 0 {
            format!("{whole} GiB")
        } else {
            format!("{whole}.5 GiB")
        }
    } else {
        format!("{} MiB", bytes / MIB)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_result_over_its_query_quota_fails_the_writer_and_names_the_setting() {
        let store = ResultStore::with_limits(64, 1024);
        let mut writer = store.begin("small", "owner").unwrap();
        // The first page is bigger than 64 bytes: the flush that carries
        // it fails, on push or on publish.
        let mut failure = None;
        for index in 0..PAGE_ROWS {
            if let Err(error) = writer.push(vec![Value::from(index)]) {
                failure = Some(error);
                break;
            }
        }
        let error = match failure {
            Some(error) => error,
            None => store.publish("small", writer).unwrap_err(),
        }
        .to_string();
        assert!(
            error.contains("query result disk quota exceeded (0 MiB)")
                && error.contains("KAVEON_RESULT_QUERY_DISK_LIMIT_BYTES"),
            "{error}"
        );
        assert_eq!(mebibytes(256 * 1024 * 1024), "256 MiB");
        assert_eq!(mebibytes(1024 * 1024 * 1024), "1 GiB");
        assert_eq!(mebibytes(3 * 512 * 1024 * 1024), "1.5 GiB");
        assert_eq!(mebibytes(1000 * 1024 * 1024), "1000 MiB");
    }
    use crate::security::Role;

    fn analyst(principal: &str) -> Identity {
        Identity {
            principal: principal.into(),
            display_identity: None,
            role: Role::Analyst,
        }
    }
    fn ready(page: Result<ResultPage, StatusCode>) -> Value {
        match page {
            Ok(ResultPage::Ready(value)) => value,
            other => panic!("expected a ready page, got {other:?}"),
        }
    }

    #[test]
    fn pages_replay_and_isolate_owners() {
        let store = ResultStore::default();
        let mut writer = store.begin("query", "alice").unwrap();
        for i in 0..1002 {
            writer.push(vec![Value::from(i)]).unwrap();
        }
        store.publish("query", writer).unwrap();
        let alice = analyst("alice");
        let bob = analyst("bob");
        assert_eq!(
            store.page("query", 0, &bob).unwrap_err(),
            StatusCode::NOT_FOUND
        );
        let first = ready(store.page("query", 0, &alice));
        assert_eq!(first["data"].as_array().unwrap().len(), 1000);
        assert_eq!(first, ready(store.page("query", 0, &alice)));
        assert_eq!(
            ready(store.page("query", 1, &alice))["data"]
                .as_array()
                .unwrap()
                .len(),
            2
        );
        assert!(store.page("query", 2, &alice).is_err());
    }

    #[test]
    fn pages_are_served_while_the_writer_runs() {
        let store = ResultStore::default();
        let alice = analyst("alice");
        let bob = analyst("bob");
        let mut writer = store.begin("query", "alice").unwrap();
        assert!(store.contains("query"));
        assert_eq!(
            store.page("query", 0, &alice),
            Ok(ResultPage::Pending(
                serde_json::json!({"id": "query", "row_count": 0, "complete": false})
            ))
        );
        for i in 0..1500 {
            writer.push(vec![Value::from(i)]).unwrap();
        }
        let first = ready(store.page("query", 0, &alice));
        assert_eq!(first["data"].as_array().unwrap().len(), 1000);
        assert_eq!(first["complete"], Value::Bool(false));
        assert_eq!(first["row_count"], Value::from(1000));
        assert_eq!(first["next_uri"], "/v1/query/query/results/1");
        assert_eq!(
            store.page("query", 1, &alice),
            Ok(ResultPage::Pending(
                serde_json::json!({"id": "query", "row_count": 1000, "complete": false})
            ))
        );
        assert_eq!(
            store.page("query", 2, &alice).unwrap_err(),
            StatusCode::NOT_FOUND
        );
        // Owner isolation holds before the result is complete.
        assert_eq!(
            store.page("query", 0, &bob).unwrap_err(),
            StatusCode::NOT_FOUND
        );
        assert_eq!(
            store.page("query", 1, &bob).unwrap_err(),
            StatusCode::NOT_FOUND
        );

        store.publish("query", writer).unwrap();
        let first = ready(store.page("query", 0, &alice));
        assert_eq!(first["complete"], Value::Bool(true));
        assert_eq!(first["row_count"], Value::from(1500));
        assert_eq!(first["next_uri"], "/v1/query/query/results/1");
        let last = ready(store.page("query", 1, &alice));
        assert_eq!(last["data"].as_array().unwrap().len(), 500);
        assert_eq!(last["complete"], Value::Bool(true));
        assert_eq!(last["row_count"], Value::from(1500));
        assert_eq!(last["next_uri"], Value::Null);
        assert_eq!(
            store.page("query", 2, &alice).unwrap_err(),
            StatusCode::NOT_FOUND
        );
    }

    #[test]
    fn an_abandoned_writer_leaves_a_tombstone_and_frees_disk() {
        let store = ResultStore::default();
        let alice = analyst("alice");
        let mut writer = store.begin("query", "alice").unwrap();
        for i in 0..1001 {
            writer.push(vec![Value::from(i)]).unwrap();
        }
        let path = writer.shared.directory.clone();
        assert!(path.join("0.json").exists());
        assert!(store.used.load(Ordering::Acquire) > 0);
        drop(writer);
        assert!(!path.exists());
        assert_eq!(store.used.load(Ordering::Acquire), 0);
        assert!(!store.contains("query"));
        assert_eq!(
            store.page("query", 0, &alice).unwrap_err(),
            StatusCode::GONE
        );
        assert_eq!(
            store.page("query", 1, &alice).unwrap_err(),
            StatusCode::GONE
        );
        assert_eq!(
            store.page("query", 0, &analyst("bob")).unwrap_err(),
            StatusCode::NOT_FOUND
        );
        // The tombstone does not count against retention and expires with
        // the TTL like any entry.
        store
            .results
            .lock()
            .unwrap()
            .get_mut("query")
            .unwrap()
            .expires = Instant::now();
        store.cleanup();
        assert_eq!(
            store.page("query", 0, &alice).unwrap_err(),
            StatusCode::NOT_FOUND
        );
    }

    #[test]
    fn removing_an_in_progress_result_frees_disk_and_fails_the_writer() {
        let store = ResultStore::default();
        let alice = analyst("alice");
        let mut writer = store.begin("query", "alice").unwrap();
        for i in 0..1001 {
            writer.push(vec![Value::from(i)]).unwrap();
        }
        let path = writer.shared.directory.clone();
        store.remove("query");
        assert!(!path.exists());
        assert_eq!(store.used.load(Ordering::Acquire), 0);
        assert!(!store.contains("query"));
        assert_eq!(
            store.page("query", 0, &alice).unwrap_err(),
            StatusCode::GONE
        );
        assert!(store.publish("query", writer).is_err());
        assert_eq!(
            store.page("query", 0, &alice).unwrap_err(),
            StatusCode::GONE
        );
        assert_eq!(store.used.load(Ordering::Acquire), 0);

        // A complete result is forgotten outright.
        let writer = store.begin("done", "alice").unwrap();
        store.publish("done", writer).unwrap();
        assert!(store.contains("done"));
        store.remove("done");
        assert!(!store.contains("done"));
        assert_eq!(
            store.page("done", 0, &alice).unwrap_err(),
            StatusCode::NOT_FOUND
        );
    }

    #[test]
    fn expiration_hides_pages_and_releases_disk() {
        let store = ResultStore::default();
        let alice = analyst("alice");
        let mut writer = store.begin("query", "alice").unwrap();
        writer.push(vec![Value::from(1)]).unwrap();
        store.publish("query", writer).unwrap();
        assert!(store.used.load(Ordering::Acquire) > 0);
        let page = ready(store.page("query", 0, &alice));
        assert_eq!(page["data"], serde_json::json!([[1]]));
        assert_eq!(page["next_uri"], Value::Null);
        assert_eq!(page["complete"], Value::Bool(true));
        store
            .results
            .lock()
            .unwrap()
            .get_mut("query")
            .unwrap()
            .expires = Instant::now();
        store.cleanup();
        assert_eq!(store.used.load(Ordering::Acquire), 0);
        assert_eq!(
            store.page("query", 0, &alice).unwrap_err(),
            StatusCode::NOT_FOUND
        );
    }
}
