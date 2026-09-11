//! Coordinator-local exchange spools survive worker failure, with no HA claim.
use crate::exchange::{ExchangeChunk, ExchangeIdentity, ExchangeLimits};
use axum::body::Body;
use std::{
    collections::{BTreeMap, HashMap},
    fs,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    time::{Duration, Instant, SystemTime},
};

const QUERY_LIMIT: u64 = 2 * 1024 * 1024 * 1024;
const TTL: Duration = Duration::from_secs(900);
#[derive(Default)]
struct QuotaState {
    total: u64,
    queries: HashMap<String, u64>,
}
struct Quota {
    state: Mutex<QuotaState>,
    limit: u64,
}
struct Directory(PathBuf);
impl Drop for Directory {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
struct FileChunk {
    path: PathBuf,
    bytes: u64,
    query: String,
    quota: Arc<Quota>,
    _directory: Arc<Directory>,
}
impl Drop for FileChunk {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
        if let Ok(mut state) = self.quota.state.lock() {
            state.total -= self.bytes;
            if let Some(bytes) = state.queries.get_mut(&self.query) {
                *bytes -= self.bytes;
                if *bytes == 0 {
                    state.queries.remove(&self.query);
                }
            }
        }
    }
}
struct Entry {
    count: usize,
    chunks: BTreeMap<usize, Arc<FileChunk>>,
    expires: Instant,
}
#[derive(Default)]
struct StoreState {
    entries: HashMap<ExchangeIdentity, Entry>,
    finished: HashMap<String, Instant>,
}
pub struct DiskExchangeStore {
    directory: Arc<Directory>,
    quota: Arc<Quota>,
    state: Mutex<StoreState>,
}
impl DiskExchangeStore {
    pub fn new(root: &Path, limit: u64) -> Result<Self, String> {
        if limit == 0 {
            return Err("exchange disk limit must be positive".into());
        }
        fs::create_dir_all(root).map_err(|error| error.to_string())?;
        reconcile_stale_directories(root, SystemTime::now() - TTL)?;
        let directory = root.join(format!("kaveon-exchange-{}", uuid::Uuid::new_v4()));
        let builder = fs::DirBuilder::new();
        #[cfg(unix)]
        let mut builder = builder;
        #[cfg(unix)]
        {
            use std::os::unix::fs::DirBuilderExt;
            builder.mode(0o700);
        }
        builder
            .create(&directory)
            .map_err(|error| error.to_string())?;
        Ok(Self {
            directory: Arc::new(Directory(directory)),
            quota: Arc::new(Quota {
                state: Mutex::new(QuotaState::default()),
                limit,
            }),
            state: Mutex::new(StoreState::default()),
        })
    }
    pub fn insert(&self, chunk: ExchangeChunk) -> Result<(), String> {
        let encoded = chunk
            .encode(ExchangeLimits::default())
            .map_err(|error| error.to_string())?;
        let mut state = self
            .state
            .lock()
            .map_err(|_| "exchange store unavailable")?;
        if state
            .finished
            .contains_key(&chunk.identity.task_id.query_id)
        {
            return Err("query exchange lifecycle is already finished".into());
        }
        if let Some(entry) = state.entries.get(&chunk.identity) {
            if entry.count != chunk.chunk_count {
                return Err("conflicting exchange chunk count".into());
            }
            if let Some(existing) = entry.chunks.get(&chunk.chunk_index) {
                return if fs::read(&existing.path).map_err(|error| error.to_string())? == encoded {
                    Ok(())
                } else {
                    Err("conflicting exchange chunk".into())
                };
            }
        } else if state.entries.len() >= 1024 {
            return Err("exchange count quota exceeded".into());
        }
        let bytes = encoded.len() as u64;
        let query = chunk.identity.task_id.query_id.clone();
        {
            let mut quota = self
                .quota
                .state
                .lock()
                .map_err(|_| "exchange quota unavailable")?;
            let query_bytes = quota.queries.get(&query).copied().unwrap_or_default();
            if quota.total.saturating_add(bytes) > self.quota.limit
                || query_bytes.saturating_add(bytes) > QUERY_LIMIT
            {
                return Err("exchange disk quota exceeded".into());
            }
            quota.total += bytes;
            quota.queries.insert(query.clone(), query_bytes + bytes);
        }
        let file = Arc::new(FileChunk {
            path: self
                .directory
                .0
                .join(format!("{}.chunk", uuid::Uuid::new_v4())),
            bytes,
            query,
            quota: self.quota.clone(),
            _directory: self.directory.clone(),
        });
        fs::write(&file.path, encoded).map_err(|error| error.to_string())?;
        let entry = state
            .entries
            .entry(chunk.identity)
            .or_insert_with(|| Entry {
                count: chunk.chunk_count,
                chunks: BTreeMap::new(),
                expires: Instant::now() + TTL,
            });
        entry.expires = Instant::now() + TTL;
        entry.chunks.insert(chunk.chunk_index, file);
        Ok(())
    }
    pub fn body(&self, identity: &ExchangeIdentity) -> Result<Option<Body>, String> {
        let chunks = {
            let mut state = self
                .state
                .lock()
                .map_err(|_| "exchange store unavailable")?;
            let Some(entry) = state.entries.get_mut(identity) else {
                return Ok(None);
            };
            if entry.chunks.len() != entry.count || entry.chunks.keys().copied().ne(0..entry.count)
            {
                return Err("incomplete exchange chunk set".into());
            }
            entry.expires = Instant::now() + TTL;
            entry.chunks.values().cloned().collect::<Vec<_>>()
        };
        let stream = futures::stream::unfold(chunks.into_iter(), |mut chunks| async move {
            let file = chunks.next()?;
            let result = tokio::task::spawn_blocking(move || {
                let encoded = fs::read(&file.path)?;
                let chunk = ExchangeChunk::decode(&encoded, ExchangeLimits::default())
                    .map_err(|error| std::io::Error::other(error.to_string()))?;
                Ok::<_, std::io::Error>(chunk.payload)
            })
            .await
            .unwrap_or_else(|error| Err(std::io::Error::other(error.to_string())));
            Some((result, chunks))
        });
        Ok(Some(Body::from_stream(stream)))
    }
    pub fn remove(&self, identity: &ExchangeIdentity) -> Result<bool, String> {
        Ok(self
            .state
            .lock()
            .map_err(|_| "exchange store unavailable")?
            .entries
            .remove(identity)
            .is_some())
    }
    pub fn finish_query(&self, query: &str) {
        if let Ok(mut state) = self.state.lock() {
            state
                .entries
                .retain(|identity, _| identity.task_id.query_id != query);
            state
                .finished
                .retain(|_, expires| *expires > Instant::now());
            if state.finished.len() < 10000 {
                state.finished.insert(query.into(), Instant::now() + TTL);
            }
        }
    }
    pub fn cleanup(&self) {
        if let Ok(mut state) = self.state.lock() {
            state
                .entries
                .retain(|_, entry| entry.expires > Instant::now());
            state
                .finished
                .retain(|_, expires| *expires > Instant::now());
        }
    }
}

/// Remove only exchange directories from a previous process that have been
/// untouched for a complete exchange TTL. The age guard avoids deleting a
/// live coordinator's spool when tests or operators intentionally share a
/// parent directory. Result spools and unrelated files use different names and
/// are never traversed.
fn reconcile_stale_directories(root: &Path, stale_before: SystemTime) -> Result<(), String> {
    for item in fs::read_dir(root).map_err(|error| error.to_string())? {
        let item = item.map_err(|error| error.to_string())?;
        let file_type = item.file_type().map_err(|error| error.to_string())?;
        if !file_type.is_dir() {
            continue;
        }
        let name = item.file_name();
        let Some(identifier) = name
            .to_str()
            .and_then(|name| name.strip_prefix("kaveon-exchange-"))
        else {
            continue;
        };
        if uuid::Uuid::parse_str(identifier).is_err() {
            continue;
        }
        let mut modified = item
            .metadata()
            .and_then(|metadata| metadata.modified())
            .map_err(|error| error.to_string())?;
        // Directory mtime behavior differs across supported filesystems. Chunk
        // files are immutable, so their newest mtime is the authoritative last
        // write without decoding or trusting their contents.
        for child in fs::read_dir(item.path()).map_err(|error| error.to_string())? {
            let child = child.map_err(|error| error.to_string())?;
            let child_modified = child
                .metadata()
                .and_then(|metadata| metadata.modified())
                .map_err(|error| error.to_string())?;
            modified = modified.max(child_modified);
        }
        if modified <= stale_before {
            // A second coordinator may intentionally share the parent in local
            // development, and Windows denies removal while it owns an open
            // chunk. Treat that as evidence the directory is still live; a
            // later startup can retry it.
            let _ = fs::remove_dir_all(item.path());
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::exchange::ExchangeIdentity;
    use kaveon_core::{ExchangeId, StageId, TaskId};
    fn chunk() -> ExchangeChunk {
        ExchangeChunk {
            identity: ExchangeIdentity {
                exchange_id: ExchangeId("exchange".into()),
                task_id: TaskId {
                    query_id: "query".into(),
                    stage_id: StageId(0),
                    partition: 0,
                    attempt: 0,
                },
                output_partition: 0,
            },
            chunk_index: 0,
            chunk_count: 1,
            payload: vec![1, 2, 3].into(),
        }
    }
    #[tokio::test]
    async fn retained_download_survives_query_cleanup_and_releases_disk() {
        use futures::StreamExt;
        let store = DiskExchangeStore::new(&std::env::temp_dir(), 1024).unwrap();
        let chunk = chunk();
        store.insert(chunk.clone()).unwrap();
        store.insert(chunk.clone()).unwrap();
        let mut body = store
            .body(&chunk.identity)
            .unwrap()
            .unwrap()
            .into_data_stream();
        store.finish_query("query");
        assert!(store.quota.state.lock().unwrap().total > 0);
        assert!(store.insert(chunk.clone()).is_err());
        assert_eq!(body.next().await.unwrap().unwrap(), chunk.payload);
        drop(body);
        assert_eq!(store.quota.state.lock().unwrap().total, 0);
        assert!(store.body(&chunk.identity).unwrap().is_none());
    }
    #[test]
    fn disk_quota_failure_does_not_leave_files_or_reservations() {
        let store = DiskExchangeStore::new(&std::env::temp_dir(), 1).unwrap();
        assert!(store.insert(chunk()).is_err());
        assert_eq!(store.quota.state.lock().unwrap().total, 0);
        assert_eq!(fs::read_dir(&store.directory.0).unwrap().count(), 0);
    }

    #[test]
    fn restart_removes_only_stale_exchange_directories() {
        let root = std::env::temp_dir().join(format!("kaveon-reconcile-{}", uuid::Uuid::new_v4()));
        fs::create_dir(&root).unwrap();
        let stale = root.join(format!("kaveon-exchange-{}", uuid::Uuid::new_v4()));
        let live = root.join(format!("kaveon-exchange-{}", uuid::Uuid::new_v4()));
        let retained_result = root.join(format!("kaveon-result-{}", uuid::Uuid::new_v4()));
        let unrelated = root.join("operator-owned");
        let malformed = root.join("kaveon-exchange-not-a-uuid");
        for directory in [&stale, &live, &retained_result, &unrelated, &malformed] {
            fs::create_dir(directory).unwrap();
            fs::write(directory.join("payload"), b"retained").unwrap();
        }

        let between = SystemTime::now();
        // Some supported filesystems expose directory mtimes at one-second
        // precision, so cross a full tick before making the live directory new.
        std::thread::sleep(Duration::from_millis(1_100));
        fs::write(live.join("new-chunk"), b"live").unwrap();
        reconcile_stale_directories(&root, between).unwrap();

        assert!(!stale.exists());
        assert!(live.exists());
        assert_eq!(
            fs::read(retained_result.join("payload")).unwrap(),
            b"retained"
        );
        assert_eq!(fs::read(unrelated.join("payload")).unwrap(), b"retained");
        assert_eq!(fs::read(malformed.join("payload")).unwrap(), b"retained");
        fs::remove_dir_all(root).unwrap();
    }
}
