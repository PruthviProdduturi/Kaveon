//! Durable publication of immutable product catalog snapshots.
//!
//! Snapshot objects are written before the conditional head update. Orphaned
//! immutable snapshots are therefore possible after a conflict or uncertain
//! network outcome and must never be treated as committed. Object reads are
//! bounded only after the storage layer returns bytes; a streaming read limit
//! belongs in the storage layer.
//! Deduplication is intentionally limited to the retained parent chain. When
//! that chain exceeds the lookup budget, commits stop with `Indeterminate`
//! until a durable operation index is introduced.

use std::sync::Arc;

use kaveon_storage::{AdlsConditionalCommit, CommitErrorKind, ObjectVersion};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::{
    product_manifest::{CatalogSnapshot, PrepareChange, SnapshotRef},
    product_metrics::{TransactionMetrics, TransactionOutcome},
};

const MAX_HEAD_BYTES: usize = 64 * 1024;
const MAX_SNAPSHOT_BYTES: usize = 8 * 1024 * 1024;
const DEFAULT_HISTORY_HOPS: usize = 64;

#[derive(Clone)]
pub struct ProductCatalogCommit {
    storage: AdlsConditionalCommit,
    prefix: String,
    metrics: Arc<TransactionMetrics>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CommitOutcome {
    Committed(CatalogSnapshot),
    Replayed(CatalogSnapshot),
    Conflict,
    Rejected,
    /// A write may have reached storage. Resolve the operation before retrying.
    Indeterminate,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OperationResolution {
    Committed(CatalogSnapshot),
    Conflict,
    /// The retained chain reached a verified genesis snapshot without this ID.
    NotCommitted,
    /// The operation was not found within the requested immutable history.
    /// This does not prove that it was never committed.
    Unresolved,
}

struct Head {
    snapshot: CatalogSnapshot,
    version: ObjectVersion,
}

#[derive(Debug, Serialize, Deserialize)]
struct HeadRecord {
    reference: SnapshotRef,
    snapshot_sha256: String,
}

impl ProductCatalogCommit {
    pub fn new(
        storage: AdlsConditionalCommit,
        prefix: impl Into<String>,
        metrics: Arc<TransactionMetrics>,
    ) -> Result<Self, String> {
        let prefix = prefix.into().trim_matches('/').to_owned();
        if prefix.is_empty()
            || prefix
                .split('/')
                .any(|part| part.is_empty() || part == "." || part == "..")
        {
            return Err("product catalog prefix must be normalized and relative".to_owned());
        }
        Ok(Self {
            storage,
            prefix,
            metrics,
        })
    }

    /// Creates both immutable genesis objects. Existing objects are never overwritten.
    pub async fn initialize(&self, genesis: CatalogSnapshot) -> CommitOutcome {
        let attempt = self.metrics.begin();
        if genesis.generation != 0 || genesis.parent.is_some() || genesis.validate().is_err() {
            attempt.finish(TransactionOutcome::Rejected);
            return CommitOutcome::Rejected;
        }
        let snapshot = match encode(&genesis) {
            Ok(bytes) => bytes,
            Err(()) => {
                attempt.finish(TransactionOutcome::Rejected);
                return CommitOutcome::Rejected;
            }
        };
        let snapshot_sha256 = digest(&snapshot);
        match self
            .storage
            .create_immutable(&self.snapshot_path(&genesis.snapshot_id), snapshot)
            .await
        {
            Ok(_) => {}
            Err(error) => return finish_storage(attempt, error.kind),
        }
        let head = match encode(&HeadRecord {
            reference: genesis.reference(),
            snapshot_sha256,
        }) {
            Ok(bytes) => bytes,
            Err(()) => {
                attempt.finish(TransactionOutcome::Rejected);
                return CommitOutcome::Rejected;
            }
        };
        match self.storage.create_immutable(&self.head_path(), head).await {
            Ok(_) => {
                attempt.finish(TransactionOutcome::Committed);
                CommitOutcome::Committed(genesis)
            }
            Err(error) => finish_storage(attempt, error.kind),
        }
    }

    /// Publishes the prepared immutable snapshot, then conditionally advances the head.
    pub async fn commit(&self, request: PrepareChange) -> CommitOutcome {
        let attempt = self.metrics.begin();
        let head = match self.read_head().await {
            Ok(head) => head,
            Err(kind) => return finish_storage(attempt, kind),
        };
        let stale_base = request.base != head.snapshot.reference();
        match self
            .resolve_from(
                head.snapshot.clone(),
                &request.operation_id,
                &request.request_digest,
                DEFAULT_HISTORY_HOPS,
            )
            .await
        {
            Ok(OperationResolution::Committed(snapshot)) => {
                attempt.finish(TransactionOutcome::Replayed);
                return CommitOutcome::Replayed(snapshot);
            }
            Ok(OperationResolution::Conflict) => {
                attempt.finish(TransactionOutcome::Conflict);
                return CommitOutcome::Conflict;
            }
            Ok(OperationResolution::NotCommitted) if stale_base => {
                attempt.finish(TransactionOutcome::Conflict);
                return CommitOutcome::Conflict;
            }
            Ok(OperationResolution::NotCommitted) => {}
            Ok(OperationResolution::Unresolved) => {
                attempt.finish(TransactionOutcome::Indeterminate);
                return CommitOutcome::Indeterminate;
            }
            Err(kind) => return finish_storage(attempt, kind),
        }
        let next = match head.snapshot.prepare(request) {
            Ok(snapshot) => snapshot,
            Err(_) => {
                attempt.finish(TransactionOutcome::Rejected);
                return CommitOutcome::Rejected;
            }
        };
        let bytes = match encode(&next) {
            Ok(bytes) => bytes,
            Err(()) => {
                attempt.finish(TransactionOutcome::Rejected);
                return CommitOutcome::Rejected;
            }
        };
        let snapshot_sha256 = digest(&bytes);
        match self.publish_snapshot(&next.snapshot_id, bytes).await {
            Ok(_) => {}
            Err(kind) => return finish_storage(attempt, kind),
        }
        let head_bytes = match encode(&HeadRecord {
            reference: next.reference(),
            snapshot_sha256,
        }) {
            Ok(bytes) => bytes,
            Err(()) => {
                attempt.finish(TransactionOutcome::Rejected);
                return CommitOutcome::Rejected;
            }
        };
        match self
            .storage
            .compare_and_swap(&self.head_path(), &head.version, head_bytes)
            .await
        {
            Ok(_) => {
                attempt.finish(TransactionOutcome::Committed);
                CommitOutcome::Committed(next)
            }
            Err(error) => finish_storage(attempt, error.kind),
        }
    }

    pub async fn read_current(&self) -> Result<CatalogSnapshot, CommitErrorKind> {
        self.read_head().await.map(|head| head.snapshot)
    }

    /// Searches a bounded parent chain. Missing history and an exhausted budget
    /// are deliberately `Unresolved`, never proof that an operation is absent.
    pub async fn resolve_operation(
        &self,
        operation_id: &str,
        request_digest: &str,
        max_hops: usize,
    ) -> Result<OperationResolution, CommitErrorKind> {
        let head = self.read_head().await?;
        self.resolve_from(head.snapshot, operation_id, request_digest, max_hops)
            .await
    }

    async fn resolve_from(
        &self,
        mut snapshot: CatalogSnapshot,
        operation_id: &str,
        request_digest: &str,
        max_hops: usize,
    ) -> Result<OperationResolution, CommitErrorKind> {
        for hop in 0..=max_hops {
            if snapshot.operation_id == operation_id {
                return Ok(if snapshot.request_digest == request_digest {
                    OperationResolution::Committed(snapshot)
                } else {
                    OperationResolution::Conflict
                });
            }
            let Some(parent) = snapshot.parent else {
                return Ok(OperationResolution::NotCommitted);
            };
            if hop == max_hops {
                return Ok(OperationResolution::Unresolved);
            }
            snapshot = match self.read_snapshot(&parent).await {
                Ok(snapshot) => snapshot,
                Err(CommitErrorKind::Missing) => return Ok(OperationResolution::Unresolved),
                Err(kind) => return Err(kind),
            };
        }
        Ok(OperationResolution::Unresolved)
    }

    async fn read_head(&self) -> Result<Head, CommitErrorKind> {
        let object = self
            .storage
            .read_bounded(&self.head_path(), MAX_HEAD_BYTES)
            .await
            .map_err(|error| error.kind)?;
        let record: HeadRecord =
            decode(&object.bytes, MAX_HEAD_BYTES).ok_or(CommitErrorKind::Invalid)?;
        if !valid_digest(&record.snapshot_sha256) {
            return Err(CommitErrorKind::Invalid);
        }
        let (snapshot, snapshot_bytes) = self.read_snapshot_with_bytes(&record.reference).await?;
        if digest(&snapshot_bytes) != record.snapshot_sha256 {
            return Err(CommitErrorKind::Invalid);
        }
        Ok(Head {
            snapshot,
            version: object.version,
        })
    }

    async fn read_snapshot(
        &self,
        reference: &SnapshotRef,
    ) -> Result<CatalogSnapshot, CommitErrorKind> {
        self.read_snapshot_with_bytes(reference)
            .await
            .map(|(snapshot, _)| snapshot)
    }

    async fn read_snapshot_with_bytes(
        &self,
        reference: &SnapshotRef,
    ) -> Result<(CatalogSnapshot, Vec<u8>), CommitErrorKind> {
        if !valid_snapshot_id(&reference.snapshot_id) {
            return Err(CommitErrorKind::Invalid);
        }
        let object = self
            .storage
            .read_bounded(
                &self.snapshot_path(&reference.snapshot_id),
                MAX_SNAPSHOT_BYTES,
            )
            .await
            .map_err(|error| error.kind)?;
        let snapshot: CatalogSnapshot =
            decode(&object.bytes, MAX_SNAPSHOT_BYTES).ok_or(CommitErrorKind::Invalid)?;
        if snapshot.reference() != *reference || snapshot.validate().is_err() {
            return Err(CommitErrorKind::Invalid);
        }
        Ok((snapshot, object.bytes))
    }

    async fn publish_snapshot(
        &self,
        snapshot_id: &str,
        bytes: Vec<u8>,
    ) -> Result<(), CommitErrorKind> {
        let path = self.snapshot_path(snapshot_id);
        match self.storage.create_immutable(&path, bytes.clone()).await {
            Ok(_) => Ok(()),
            Err(error) if error.kind == CommitErrorKind::Conflict => {
                let existing = self
                    .storage
                    .read_bounded(&path, MAX_SNAPSHOT_BYTES)
                    .await
                    .map_err(|read| read.kind)?;
                if existing.bytes == bytes {
                    Ok(())
                } else {
                    Err(CommitErrorKind::Conflict)
                }
            }
            Err(error) => Err(error.kind),
        }
    }

    fn head_path(&self) -> String {
        format!("{}/head.json", self.prefix)
    }
    fn snapshot_path(&self, snapshot_id: &str) -> String {
        format!("{}/snapshots/{snapshot_id}.json", self.prefix)
    }
}

fn encode<T: Serialize>(value: &T) -> Result<Vec<u8>, ()> {
    let bytes = serde_json::to_vec(value).map_err(|_| ())?;
    if bytes.len() > MAX_SNAPSHOT_BYTES {
        return Err(());
    }
    Ok(bytes)
}

fn decode<T: for<'a> Deserialize<'a>>(bytes: &[u8], limit: usize) -> Option<T> {
    (bytes.len() <= limit)
        .then(|| serde_json::from_slice(bytes).ok())
        .flatten()
}

fn digest(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

fn valid_digest(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn valid_snapshot_id(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && !value.contains(['/', '\\', ':'])
        && value != "."
        && value != ".."
        && !value.chars().any(char::is_control)
}

fn finish_storage(
    attempt: crate::product_metrics::TransactionAttempt<'_>,
    kind: CommitErrorKind,
) -> CommitOutcome {
    let (outcome, metric) = match kind {
        CommitErrorKind::Conflict => (CommitOutcome::Conflict, TransactionOutcome::Conflict),
        CommitErrorKind::Retryable
        | CommitErrorKind::Other
        | CommitErrorKind::Unsupported
        | CommitErrorKind::LimitExceeded => (
            CommitOutcome::Indeterminate,
            TransactionOutcome::Indeterminate,
        ),
        _ => (CommitOutcome::Rejected, TransactionOutcome::Rejected),
    };
    attempt.finish(metric);
    outcome
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::product_manifest::{CatalogChange, ImmutableFileRef, TableManifestRef};
    use object_store::memory::InMemory;

    const DIGEST: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";

    fn catalog() -> ProductCatalogCommit {
        catalog_with(AdlsConditionalCommit::new(Arc::new(InMemory::new())))
    }
    fn catalog_with(storage: AdlsConditionalCommit) -> ProductCatalogCommit {
        ProductCatalogCommit::new(storage, "product", Arc::new(TransactionMetrics::default()))
            .unwrap()
    }
    fn table(name: &str) -> TableManifestRef {
        TableManifestRef {
            manifest: ImmutableFileRef {
                path: format!("tables/{name}/manifest.json"),
                sha256: DIGEST.into(),
            },
            parquet_files: vec![ImmutableFileRef {
                path: format!("tables/{name}/part.parquet"),
                sha256: DIGEST.into(),
            }],
        }
    }
    fn request(base: SnapshotRef, id: &str, table_name: &str) -> PrepareChange {
        PrepareChange {
            base,
            snapshot_id: format!("snapshot-{id}"),
            operation_id: id.into(),
            request_digest: DIGEST.into(),
            changes: vec![CatalogChange::Put {
                table: table_name.into(),
                reference: table(table_name),
            }],
        }
    }

    #[test]
    fn oversized_publication_cannot_create_an_unreadable_snapshot() {
        assert!(encode(&"x".repeat(MAX_SNAPSHOT_BYTES)).is_err());
    }

    #[tokio::test]
    async fn concurrent_same_base_has_one_winner_and_reopens() {
        let storage = AdlsConditionalCommit::new(Arc::new(InMemory::new()));
        let catalog = catalog_with(storage.clone());
        let genesis = CatalogSnapshot::empty("snapshot-genesis").unwrap();
        assert!(matches!(
            catalog.initialize(genesis.clone()).await,
            CommitOutcome::Committed(_)
        ));
        let (left, right) = tokio::join!(
            catalog.commit(request(genesis.reference(), "op-left", "bronze.left")),
            catalog.commit(request(genesis.reference(), "op-right", "bronze.right"))
        );
        assert_eq!(
            [left, right]
                .iter()
                .filter(|value| matches!(value, CommitOutcome::Committed(_)))
                .count(),
            1
        );
        let reopened = catalog_with(storage).read_current().await.unwrap();
        assert_eq!(reopened.generation, 1);
        assert_eq!(reopened.tables.len(), 1);
    }

    #[tokio::test]
    async fn replay_resolves_from_history_and_digest_conflict_is_not_replayed() {
        let catalog = catalog();
        let genesis = CatalogSnapshot::empty("snapshot-genesis").unwrap();
        catalog.initialize(genesis.clone()).await;
        let first = request(genesis.reference(), "op-1", "bronze.orders");
        assert!(matches!(
            catalog.commit(first.clone()).await,
            CommitOutcome::Committed(_)
        ));
        assert!(matches!(
            catalog.commit(first).await,
            CommitOutcome::Replayed(_)
        ));
        assert!(matches!(
            catalog.resolve_operation("op-1", &"b".repeat(64), 10).await,
            Ok(OperationResolution::Conflict)
        ));
    }

    #[tokio::test]
    async fn failed_snapshot_create_before_cas_preserves_head() {
        let catalog = catalog();
        let genesis = CatalogSnapshot::empty("snapshot-genesis").unwrap();
        catalog.initialize(genesis.clone()).await;
        let first = request(genesis.reference(), "op-1", "bronze.orders");
        assert!(matches!(
            catalog.commit(first).await,
            CommitOutcome::Committed(_)
        ));
        let before = catalog.read_current().await.unwrap();
        let collision = PrepareChange {
            base: before.reference(),
            snapshot_id: "snapshot-op-1".into(),
            operation_id: "op-collision".into(),
            request_digest: "b".repeat(64),
            changes: vec![CatalogChange::Put {
                table: "bronze.other".into(),
                reference: table("other"),
            }],
        };
        assert!(matches!(
            catalog.commit(collision).await,
            CommitOutcome::Conflict
        ));
        assert_eq!(catalog.read_current().await.unwrap(), before);
    }

    #[tokio::test]
    async fn exhausted_history_refuses_to_reuse_an_old_operation() {
        let catalog = catalog();
        let genesis = CatalogSnapshot::empty("snapshot-genesis").unwrap();
        catalog.initialize(genesis.clone()).await;
        let mut current = genesis;
        for index in 0..=65 {
            let operation = format!("op-{index}");
            current = current
                .prepare(request(current.reference(), &operation, "bronze.orders"))
                .unwrap();
            let bytes = encode(&current).unwrap();
            catalog
                .storage
                .create_immutable(&catalog.snapshot_path(&current.snapshot_id), bytes.clone())
                .await
                .unwrap();
        }
        let old_head = catalog
            .storage
            .read_bounded(&catalog.head_path(), MAX_HEAD_BYTES)
            .await
            .unwrap();
        let head = encode(&HeadRecord {
            reference: current.reference(),
            snapshot_sha256: digest(&encode(&current).unwrap()),
        })
        .unwrap();
        catalog
            .storage
            .compare_and_swap(&catalog.head_path(), &old_head.version, head)
            .await
            .unwrap();
        let before = catalog.read_current().await.unwrap();
        let reused = request(before.reference(), "op-0", "bronze.orders");
        assert!(matches!(
            catalog.commit(reused).await,
            CommitOutcome::Indeterminate
        ));
        assert_eq!(catalog.read_current().await.unwrap(), before);
    }
}
