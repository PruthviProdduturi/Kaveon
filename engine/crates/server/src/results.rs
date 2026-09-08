//! Immutable, owner-scoped result pages with process/query disk quotas and TTL.
use crate::security::Identity;
use axum::http::StatusCode;
use serde_json::Value;
use std::{
    collections::HashMap,
    fs,
    path::PathBuf,
    sync::{
        Arc, Mutex,
        atomic::{AtomicU64, Ordering},
    },
    time::{Duration, Instant},
};

const PAGE_ROWS: usize = 1_000;
const PAGE_BYTES: usize = 4 * 1024 * 1024;
const QUERY_BYTES: u64 = 256 * 1024 * 1024;
const PROCESS_BYTES: u64 = 1024 * 1024 * 1024;
const TTL: Duration = Duration::from_secs(900);

#[derive(Default)]
pub struct ResultStore {
    results: Mutex<HashMap<String, Entry>>,
    used: Arc<AtomicU64>,
}
struct Entry {
    owner: String,
    expires: Instant,
    files: ResultWriter,
}
pub struct ResultWriter {
    directory: PathBuf,
    used: Arc<AtomicU64>,
    bytes: u64,
    page: Vec<Vec<Value>>,
    page_bytes: usize,
    pages: usize,
    pub rows: usize,
}
impl ResultStore {
    pub fn remove(&self, id: &str) {
        if let Ok(mut entries) = self.results.lock() {
            entries.remove(id);
        }
    }
    pub fn contains(&self, id: &str) -> bool {
        self.results
            .lock()
            .is_ok_and(|entries| entries.contains_key(id))
    }
    pub fn writer(&self) -> std::io::Result<ResultWriter> {
        self.cleanup();
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
        Ok(ResultWriter {
            directory,
            used: self.used.clone(),
            bytes: 0,
            page: Vec::new(),
            page_bytes: 0,
            pages: 0,
            rows: 0,
        })
    }
    pub fn publish(&self, id: &str, owner: &str, mut writer: ResultWriter) -> std::io::Result<()> {
        writer.flush()?;
        let mut results = self
            .results
            .lock()
            .map_err(|_| std::io::Error::other("result store unavailable"))?;
        if results.len() >= 100 {
            return Err(std::io::Error::other(
                "result retention limit reached; retry after expiration",
            ));
        }
        results.insert(
            id.into(),
            Entry {
                owner: owner.into(),
                expires: Instant::now() + TTL,
                files: writer,
            },
        );
        Ok(())
    }
    pub fn page(&self, id: &str, index: usize, identity: &Identity) -> Result<Value, StatusCode> {
        self.cleanup();
        let entries = self
            .results
            .lock()
            .map_err(|_| StatusCode::SERVICE_UNAVAILABLE)?;
        let entry = entries
            .get(id)
            .filter(|entry| identity.can_view(Some(&entry.owner)))
            .ok_or(StatusCode::NOT_FOUND)?;
        if index >= entry.files.pages {
            return Err(StatusCode::NOT_FOUND);
        }
        let data: Value = serde_json::from_reader(
            fs::File::open(entry.files.directory.join(format!("{index}.json")))
                .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?,
        )
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
        let next = (index + 1 < entry.files.pages)
            .then(|| format!("/v1/query/{id}/results/{}", index + 1));
        Ok(serde_json::json!({"id":id, "data":data, "next_uri":next, "row_count":entry.files.rows}))
    }
    pub fn cleanup(&self) {
        if let Ok(mut entries) = self.results.lock() {
            entries.retain(|_, entry| entry.expires > Instant::now());
        }
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
        if self.page.is_empty() && self.pages > 0 {
            return Ok(());
        }
        let encoded = serde_json::to_vec(&self.page)?;
        let bytes = encoded.len() as u64;
        if self.bytes + bytes > QUERY_BYTES {
            return Err(std::io::Error::other(
                "query result disk quota exceeded (256 MiB)",
            ));
        }
        self.used
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |used| {
                used.checked_add(bytes)
                    .filter(|total| *total <= PROCESS_BYTES)
            })
            .map_err(|_| std::io::Error::other("process result disk quota exceeded (1 GiB)"))?;
        self.bytes += bytes;
        fs::write(self.directory.join(format!("{}.json", self.pages)), encoded)?;
        self.pages += 1;
        self.page.clear();
        self.page_bytes = 0;
        Ok(())
    }
}
impl Drop for ResultWriter {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.directory);
        self.used.fetch_sub(self.bytes, Ordering::AcqRel);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::security::Role;
    #[test]
    fn pages_replay_and_isolate_owners() {
        let store = ResultStore::default();
        let mut writer = store.writer().unwrap();
        for i in 0..1002 {
            writer.push(vec![Value::from(i)]).unwrap();
        }
        store.publish("query", "alice", writer).unwrap();
        let alice = Identity {
            principal: "alice".into(),
            role: Role::Analyst,
        };
        let bob = Identity {
            principal: "bob".into(),
            role: Role::Analyst,
        };
        assert_eq!(
            store.page("query", 0, &bob).unwrap_err(),
            StatusCode::NOT_FOUND
        );
        let first = store.page("query", 0, &alice).unwrap();
        assert_eq!(first["data"].as_array().unwrap().len(), 1000);
        assert_eq!(first, store.page("query", 0, &alice).unwrap());
        assert_eq!(
            store.page("query", 1, &alice).unwrap()["data"]
                .as_array()
                .unwrap()
                .len(),
            2
        );
        assert!(store.page("query", 2, &alice).is_err());
    }
    #[test]
    fn abandoned_writer_releases_disk_and_expiration_hides_pages() {
        let store = ResultStore::default();
        let mut writer = store.writer().unwrap();
        writer.push(vec![Value::from(1)]).unwrap();
        writer.flush().unwrap();
        let path = writer.directory.clone();
        assert!(store.used.load(Ordering::Acquire) > 0);
        drop(writer);
        assert!(!path.exists());
        assert_eq!(store.used.load(Ordering::Acquire), 0);
        let writer = store.writer().unwrap();
        store.publish("query", "alice", writer).unwrap();
        store
            .results
            .lock()
            .unwrap()
            .get_mut("query")
            .unwrap()
            .expires = Instant::now();
        store.cleanup();
        assert_eq!(store.used.load(Ordering::Acquire), 0);
    }
}
