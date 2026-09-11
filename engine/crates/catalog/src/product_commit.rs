//! Durable publication of immutable product catalog snapshots.
//!
//! Snapshot objects are written before the conditional head update. Orphaned
//! immutable snapshots are therefore possible after a conflict or uncertain
//! network outcome and must never be treated as committed. Object reads are
//! bounded only after the storage layer returns bytes; a streaming read limit
//! belongs in the storage layer.
//! Deduplication uses immutable, head-referenced SHA-256 operation shards.
//! Each deterministic hash shard is bounded to 1,024 operation IDs; a full
//! shard fails closed with `Indeterminate` until it is compacted into a new
//! index format. This is a safe operational limit, not unbounded deduplication.

use std::{collections::BTreeMap, sync::Arc};

use kaveon_storage::{AdlsConditionalCommit, CommitErrorKind, ObjectVersion};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::{
    product_manifest::{
        CatalogChange, CatalogSnapshot, ImmutableFileRef, PrepareChange, SnapshotRef,
    },
    product_metrics::{TransactionMetrics, TransactionOutcome},
};

const MAX_HEAD_BYTES: usize = 64 * 1024;
const MAX_SNAPSHOT_BYTES: usize = 8 * 1024 * 1024;
const MAX_INDEX_SHARD_ENTRIES: usize = 1_024;
const MAX_PRODUCT_DOCUMENT_BYTES: usize = 8 * 1024 * 1024;

pub type ProductDocuments = BTreeMap<String, Vec<u8>>;

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
#[allow(clippy::large_enum_variant)]
pub enum OperationResolution {
    Committed(CatalogSnapshot),
    Conflict,
    /// The retained chain reached a verified genesis snapshot without this ID.
    NotCommitted,
    /// The operation was not found within the requested immutable history.
    /// This does not prove that it was never committed.
    Unresolved,
}

/// Durable mutation journal data retained in the immutable operation index.
/// `Prepared` means the record was written before head CAS; callers must
/// resolve the current head to distinguish a committed operation from an
/// orphaned candidate after an ambiguous response.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MutationJournalOutcome {
    Prepared,
    Committed,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MutationJournalRecord {
    pub transaction_id: String,
    pub base_snapshot: Option<SnapshotRef>,
    pub request_digest: String,
    pub changes: Vec<CatalogChange>,
    pub outcome: MutationJournalOutcome,
    pub committed_snapshot: SnapshotRef,
}

struct Head {
    snapshot: CatalogSnapshot,
    version: ObjectVersion,
    operation_index: Option<BTreeMap<String, ImmutableIndexRef>>,
}

#[derive(Debug, Serialize, Deserialize)]
struct HeadRecord {
    reference: SnapshotRef,
    snapshot_sha256: String,
    #[serde(default)]
    operation_index: Option<BTreeMap<String, ImmutableIndexRef>>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
struct ImmutableIndexRef {
    path: String,
    sha256: String,
}
#[derive(Debug, Serialize, Deserialize)]
struct OperationRecord {
    request_digest: String,
    snapshot: SnapshotRef,
    snapshot_sha256: String,
    result: Option<ImmutableIndexRef>,
    #[serde(default)]
    transaction_id: String,
    #[serde(default)]
    base_snapshot: Option<SnapshotRef>,
    #[serde(default)]
    changes: Vec<CatalogChange>,
    #[serde(default)]
    outcome: JournalOutcome,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
enum JournalOutcome {
    #[default]
    Prepared,
    Committed,
}
#[derive(Debug, Serialize, Deserialize, Default)]
struct OperationShard {
    entries: BTreeMap<String, OperationRecord>,
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
        let head = match encode_head(&HeadRecord {
            reference: genesis.reference(),
            snapshot_sha256,
            operation_index: Some(BTreeMap::new()),
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
        self.commit_with_documents(request, BTreeMap::new()).await
    }

    /// Publishes create-only product documents before advancing the snapshot head.
    /// Only documents referenced by product creates or updates are accepted.
    pub async fn commit_with_documents(
        &self,
        request: PrepareChange,
        documents: ProductDocuments,
    ) -> CommitOutcome {
        let attempt = self.metrics.begin();
        let head = match self.read_head().await {
            Ok(head) => head,
            Err(_) => return finish_indeterminate(attempt),
        };
        let Some(index) = &head.operation_index else {
            attempt.finish(TransactionOutcome::Indeterminate);
            return CommitOutcome::Indeterminate;
        };
        let shard_key = digest(request.operation_id.as_bytes())[..2].to_owned();
        let mut shard = match self.read_shard(index.get(&shard_key)).await {
            Ok(value) => value,
            Err(_) => return finish_indeterminate(attempt),
        };
        if let Some(existing) = shard.entries.get(&request.operation_id) {
            if existing.request_digest != request.request_digest {
                attempt.finish(TransactionOutcome::Conflict);
                return CommitOutcome::Conflict;
            }
            match self.read_snapshot_with_bytes(&existing.snapshot).await {
                Ok((snapshot, bytes)) if digest(&bytes) == existing.snapshot_sha256 => {
                    attempt.finish(TransactionOutcome::Replayed);
                    return CommitOutcome::Replayed(snapshot);
                }
                Ok(_) => return finish_indeterminate(attempt),
                Err(_) => return finish_indeterminate(attempt),
            }
        }
        if request.base != head.snapshot.reference() {
            attempt.finish(TransactionOutcome::Conflict);
            return CommitOutcome::Conflict;
        }
        let next = match head.snapshot.prepare(request.clone()) {
            Ok(snapshot) => snapshot,
            Err(_) => {
                attempt.finish(TransactionOutcome::Rejected);
                return CommitOutcome::Rejected;
            }
        };
        let required_documents = match required_documents(&request) {
            Ok(required) => required,
            Err(()) => {
                attempt.finish(TransactionOutcome::Rejected);
                return CommitOutcome::Rejected;
            }
        };
        if documents.len() != required_documents.len()
            || documents
                .keys()
                .any(|path| !required_documents.contains_key(path))
        {
            attempt.finish(TransactionOutcome::Rejected);
            return CommitOutcome::Rejected;
        }
        for (path, reference) in &required_documents {
            let Some(document) = documents.get(path) else {
                attempt.finish(TransactionOutcome::Rejected);
                return CommitOutcome::Rejected;
            };
            if document.len() > MAX_PRODUCT_DOCUMENT_BYTES || digest(document) != reference.sha256 {
                attempt.finish(TransactionOutcome::Rejected);
                return CommitOutcome::Rejected;
            }
        }
        for (path, document) in documents {
            match self
                .publish_document(&required_documents[&path], document)
                .await
            {
                Ok(()) => {}
                Err(kind) => return finish_storage(attempt, kind),
            }
        }
        let bytes = match encode(&next) {
            Ok(bytes) => bytes,
            Err(()) => {
                attempt.finish(TransactionOutcome::Rejected);
                return CommitOutcome::Rejected;
            }
        };
        let snapshot_sha256 = digest(&bytes);
        if shard.entries.len() >= MAX_INDEX_SHARD_ENTRIES {
            attempt.finish(TransactionOutcome::Indeterminate);
            return CommitOutcome::Indeterminate;
        }
        shard.entries.insert(
            request.operation_id.clone(),
            OperationRecord {
                request_digest: request.request_digest.clone(),
                snapshot: next.reference(),
                snapshot_sha256: snapshot_sha256.clone(),
                result: None,
                transaction_id: request.operation_id.clone(),
                base_snapshot: Some(request.base.clone()),
                changes: request.changes.clone(),
                outcome: JournalOutcome::Prepared,
            },
        );
        let mut next_index = index.clone();
        let shard_ref = match self.publish_shard(&shard).await {
            Ok(value) => value,
            Err(kind) => return finish_storage(attempt, kind),
        };
        next_index.insert(shard_key, shard_ref);
        match self.publish_snapshot(&next.snapshot_id, bytes).await {
            Ok(_) => {}
            Err(kind) => return finish_storage(attempt, kind),
        }
        let head_bytes = match encode_head(&HeadRecord {
            reference: next.reference(),
            snapshot_sha256,
            operation_index: Some(next_index),
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

    /// Reads and verifies one immutable product document on demand. Snapshot
    /// head reads deliberately do not scan all product documents.
    pub async fn fetch_product_document(
        &self,
        kind: crate::product_manifest::ProductRecordKind,
        id: &str,
    ) -> Result<Option<Vec<u8>>, CommitErrorKind> {
        let head = self.read_head().await?;
        self.fetch_product_document_at(&head.snapshot, kind, id)
            .await
    }

    /// Reads one document referenced by an already pinned snapshot. This keeps
    /// authorization metadata and returned bytes on the same catalog generation.
    pub async fn fetch_product_document_at(
        &self,
        snapshot: &CatalogSnapshot,
        kind: crate::product_manifest::ProductRecordKind,
        id: &str,
    ) -> Result<Option<Vec<u8>>, CommitErrorKind> {
        let record = snapshot
            .product_record(kind, id)
            .map_err(|_| CommitErrorKind::Invalid)?;
        let Some(record) = record else {
            return Ok(None);
        };
        let object = self
            .storage
            .read_bounded(
                &format!("{}/{}", self.prefix, record.document.path),
                MAX_PRODUCT_DOCUMENT_BYTES,
            )
            .await
            .map_err(|error| error.kind)?;
        if digest(&object.bytes) != record.document.sha256 {
            return Err(CommitErrorKind::Invalid);
        }
        Ok(Some(object.bytes))
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

    /// Resolves the retained immutable mutation journal entry against the
    /// current head. A journal entry is committed only when its snapshot is
    /// head-reachable; otherwise it remains `Prepared` and may be an orphan
    /// from an ambiguous or conflicting CAS attempt.
    pub async fn resolve_mutation_journal(
        &self,
        transaction_id: &str,
        request_digest: &str,
    ) -> Result<Option<MutationJournalRecord>, CommitErrorKind> {
        let head = self.read_head().await?;
        let Some(index) = &head.operation_index else {
            return Ok(None);
        };
        let shard_key = digest(transaction_id.as_bytes())[..2].to_owned();
        let Some(reference) = index.get(&shard_key) else {
            return Ok(None);
        };
        let shard = self.read_shard(Some(reference)).await?;
        let Some(record) = shard.entries.get(transaction_id) else {
            return Ok(None);
        };
        if record.request_digest != request_digest {
            return Err(CommitErrorKind::Conflict);
        }
        let outcome = if head.snapshot.reference() == record.snapshot {
            MutationJournalOutcome::Committed
        } else {
            MutationJournalOutcome::Prepared
        };
        Ok(Some(MutationJournalRecord {
            transaction_id: if record.transaction_id.is_empty() {
                transaction_id.to_owned()
            } else {
                record.transaction_id.clone()
            },
            base_snapshot: record.base_snapshot.clone(),
            request_digest: record.request_digest.clone(),
            changes: record.changes.clone(),
            outcome,
            committed_snapshot: record.snapshot.clone(),
        }))
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
            operation_index: record.operation_index,
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

    async fn publish_document(
        &self,
        reference: &ImmutableFileRef,
        bytes: Vec<u8>,
    ) -> Result<(), CommitErrorKind> {
        let path = format!("{}/{}", self.prefix, reference.path);
        match self.storage.create_immutable(&path, bytes.clone()).await {
            Ok(_) => Ok(()),
            Err(error) if error.kind == CommitErrorKind::Conflict => {
                let existing = self
                    .storage
                    .read_bounded(&path, MAX_PRODUCT_DOCUMENT_BYTES)
                    .await
                    .map_err(|read| read.kind)?;
                if existing.bytes == bytes && digest(&existing.bytes) == reference.sha256 {
                    Ok(())
                } else {
                    Err(CommitErrorKind::Conflict)
                }
            }
            Err(error) => Err(error.kind),
        }
    }

    async fn read_shard(
        &self,
        reference: Option<&ImmutableIndexRef>,
    ) -> Result<OperationShard, CommitErrorKind> {
        let Some(reference) = reference else {
            return Ok(OperationShard::default());
        };
        if !valid_digest(&reference.sha256) {
            return Err(CommitErrorKind::Invalid);
        }
        let bytes = self
            .storage
            .read_bounded(
                &format!("{}/{}", self.prefix, reference.path),
                MAX_HEAD_BYTES,
            )
            .await
            .map_err(|error| error.kind)?
            .bytes;
        if digest(&bytes) != reference.sha256 {
            return Err(CommitErrorKind::Invalid);
        }
        let shard: OperationShard =
            decode(&bytes, MAX_HEAD_BYTES).ok_or(CommitErrorKind::Invalid)?;
        if shard.entries.len() > MAX_INDEX_SHARD_ENTRIES {
            return Err(CommitErrorKind::Invalid);
        }
        Ok(shard)
    }
    async fn publish_shard(
        &self,
        shard: &OperationShard,
    ) -> Result<ImmutableIndexRef, CommitErrorKind> {
        let bytes = encode(shard).map_err(|_| CommitErrorKind::Invalid)?;
        let sha256 = digest(&bytes);
        let path = format!("{}/operation-index/{sha256}.json", self.prefix);
        match self.storage.create_immutable(&path, bytes.clone()).await {
            Ok(_) => {}
            Err(error) if error.kind == CommitErrorKind::Conflict => {
                let old = self
                    .storage
                    .read_bounded(&path, MAX_HEAD_BYTES)
                    .await
                    .map_err(|error| error.kind)?;
                if old.bytes != bytes {
                    return Err(CommitErrorKind::Conflict);
                }
            }
            Err(error) => return Err(error.kind),
        }
        Ok(ImmutableIndexRef {
            path: format!("operation-index/{sha256}.json"),
            sha256,
        })
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

fn required_documents(request: &PrepareChange) -> Result<BTreeMap<String, ImmutableFileRef>, ()> {
    let mut required = BTreeMap::new();
    for change in &request.changes {
        let record = match change {
            CatalogChange::CreateProduct { record }
            | CatalogChange::UpdateProduct { record, .. } => record,
            CatalogChange::PutStatistics { statistics, .. } => {
                match required.insert(
                    statistics.document.path.clone(),
                    statistics.document.clone(),
                ) {
                    Some(previous) if previous.sha256 != statistics.document.sha256 => {
                        return Err(());
                    }
                    _ => {}
                }
                continue;
            }
            _ => continue,
        };
        match required.insert(record.document.path.clone(), record.document.clone()) {
            Some(previous) if previous.sha256 != record.document.sha256 => return Err(()),
            _ => {}
        }
    }
    Ok(required)
}

fn encode_head(value: &HeadRecord) -> Result<Vec<u8>, ()> {
    let bytes = serde_json::to_vec(value).map_err(|_| ())?;
    (bytes.len() <= MAX_HEAD_BYTES).then_some(bytes).ok_or(())
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

fn finish_indeterminate(attempt: crate::product_metrics::TransactionAttempt<'_>) -> CommitOutcome {
    attempt.finish(TransactionOutcome::Indeterminate);
    CommitOutcome::Indeterminate
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
    use crate::product_manifest::{
        CatalogChange, ImmutableFileRef, ProductRecordKind, ProductRecordRef,
        ProductRecordReference, TableManifestRef,
    };
    use object_store::{ObjectStore, memory::InMemory, path::Path};

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

    fn control(path: &str) -> ImmutableFileRef {
        let object_path = format!("control/{path}.json");
        ImmutableFileRef {
            sha256: digest(&document_bytes(&object_path)),
            path: object_path,
        }
    }
    fn document_bytes(path: &str) -> Vec<u8> {
        format!("payload:{path}").into_bytes()
    }
    fn documents_for(request: &PrepareChange) -> ProductDocuments {
        required_documents(request)
            .unwrap()
            .into_keys()
            .map(|path| {
                let bytes = document_bytes(&path);
                (path, bytes)
            })
            .collect()
    }
    fn dashboard(id: &str, revision: u64, name: &str) -> ProductRecordRef {
        ProductRecordRef {
            kind: ProductRecordKind::Dashboard,
            id: id.into(),
            revision,
            document: control(&format!("{id}-{revision}")),
            unique_values: BTreeMap::from([("owner_name".into(), name.into())]),
            references: Default::default(),
        }
    }

    fn related_record(
        kind: ProductRecordKind,
        id: &str,
        reference: Option<ProductRecordReference>,
    ) -> ProductRecordRef {
        ProductRecordRef {
            kind,
            id: id.into(),
            revision: 1,
            document: control(id),
            unique_values: BTreeMap::new(),
            references: reference.into_iter().collect(),
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
    async fn committed_control_records_reopen_with_their_table_generation() {
        let storage = AdlsConditionalCommit::new(Arc::new(InMemory::new()));
        let catalog = catalog_with(storage.clone());
        let genesis = CatalogSnapshot::empty("snapshot-genesis").unwrap();
        catalog.initialize(genesis.clone()).await;
        let request = PrepareChange {
            base: genesis.reference(),
            snapshot_id: "snapshot-product-state".into(),
            operation_id: "op-product-state".into(),
            request_digest: DIGEST.into(),
            changes: vec![
                CatalogChange::Put {
                    table: "kaveon.system_events".into(),
                    reference: table("system_events"),
                },
                CatalogChange::PutControl {
                    key: "dashboards.executive-overview".into(),
                    reference: control("dashboard-1"),
                },
            ],
        };

        assert!(matches!(
            catalog.commit(request).await,
            CommitOutcome::Committed(_)
        ));
        let reopened = catalog_with(storage).read_current().await.unwrap();
        assert_eq!(reopened.generation, 1);
        assert!(reopened.tables.contains_key("kaveon.system_events"));
        assert!(
            reopened
                .control_records
                .contains_key("dashboards.executive-overview")
        );
    }

    #[tokio::test]
    async fn statistics_document_is_required_and_reopens_with_exact_source() {
        let storage = AdlsConditionalCommit::new(Arc::new(InMemory::new()));
        let catalog = catalog_with(storage.clone());
        let genesis = CatalogSnapshot::empty("snapshot-genesis").unwrap();
        catalog.initialize(genesis.clone()).await;
        let source = table("orders");
        let table_request = PrepareChange {
            base: genesis.reference(),
            snapshot_id: "snapshot-table".into(),
            operation_id: "put-table".into(),
            request_digest: DIGEST.into(),
            changes: vec![CatalogChange::Put {
                table: "sales.orders".into(),
                reference: source.clone(),
            }],
        };
        let table_snapshot = match catalog.commit(table_request).await {
            CommitOutcome::Committed(value) => value,
            other => panic!("unexpected: {other:?}"),
        };
        let bytes = br#"{"row_count":42}"#.to_vec();
        let path = "statistics/sales-orders.json".to_owned();
        let request = PrepareChange {
            base: table_snapshot.reference(),
            snapshot_id: "snapshot-analyzed".into(),
            operation_id: "analyze-table".into(),
            request_digest: DIGEST.into(),
            changes: vec![
                CatalogChange::PutRuntimeTableSource {
                    table: "sales.orders".into(),
                    source: crate::product_manifest::RuntimeTableSourceRef {
                        catalog_snapshot_sha256: DIGEST.into(),
                        source_identity_sha256: DIGEST.into(),
                    },
                },
                CatalogChange::PutStatistics {
                    table: "sales.orders".into(),
                    statistics: crate::product_manifest::TableStatisticsRef {
                        catalog_snapshot_sha256: DIGEST.into(),
                        source_identity_sha256: DIGEST.into(),
                        document: ImmutableFileRef {
                            path: path.clone(),
                            sha256: digest(&bytes),
                        },
                        row_count: 42,
                    },
                },
            ],
        };
        assert_eq!(
            catalog.commit(request.clone()).await,
            CommitOutcome::Rejected
        );
        assert!(matches!(
            catalog
                .commit_with_documents(request, BTreeMap::from([(path, bytes)]))
                .await,
            CommitOutcome::Committed(_)
        ));
        let reopened = catalog_with(storage).read_current().await.unwrap();
        assert_eq!(reopened.table_statistics["sales.orders"].row_count, 42);
    }

    #[tokio::test]
    async fn concurrent_product_updates_have_one_durable_winner() {
        let storage = AdlsConditionalCommit::new(Arc::new(InMemory::new()));
        let catalog = catalog_with(storage.clone());
        let genesis = CatalogSnapshot::empty("snapshot-genesis").unwrap();
        catalog.initialize(genesis.clone()).await;
        let create_request = PrepareChange {
            base: genesis.reference(),
            snapshot_id: "snapshot-created".into(),
            operation_id: "create-dashboard".into(),
            request_digest: DIGEST.into(),
            changes: vec![CatalogChange::CreateProduct {
                record: dashboard("dash-1", 1, "alice/home"),
            }],
        };
        let created = match catalog
            .commit_with_documents(create_request.clone(), documents_for(&create_request))
            .await
        {
            CommitOutcome::Committed(snapshot) => snapshot,
            other => panic!("unexpected create outcome: {other:?}"),
        };
        let update = |operation: &str, name: &str| PrepareChange {
            base: created.reference(),
            snapshot_id: format!("snapshot-{operation}"),
            operation_id: operation.into(),
            request_digest: DIGEST.into(),
            changes: vec![CatalogChange::UpdateProduct {
                expected_revision: 1,
                record: dashboard("dash-1", 2, name),
            }],
        };

        let left_request = update("left", "alice/left");
        let right_request = update("right", "alice/right");
        let (left, right) = tokio::join!(
            catalog.commit_with_documents(left_request.clone(), documents_for(&left_request)),
            catalog.commit_with_documents(right_request.clone(), documents_for(&right_request))
        );
        assert_eq!(
            [left, right]
                .iter()
                .filter(|outcome| matches!(outcome, CommitOutcome::Committed(_)))
                .count(),
            1
        );
        let reopened = catalog_with(storage).read_current().await.unwrap();
        assert_eq!(reopened.generation, 2);
        assert_eq!(reopened.product_records["dashboard/dash-1"].revision, 2);
        assert!(matches!(
            reopened.product_records["dashboard/dash-1"].unique_values["owner_name"].as_str(),
            "alice/left" | "alice/right"
        ));
    }

    #[tokio::test]
    async fn related_product_records_survive_commit_reopen() {
        let storage = AdlsConditionalCommit::new(Arc::new(InMemory::new()));
        let catalog = catalog_with(storage.clone());
        let genesis = CatalogSnapshot::empty("snapshot-genesis").unwrap();
        catalog.initialize(genesis.clone()).await;
        let request = PrepareChange {
            base: genesis.reference(),
            snapshot_id: "snapshot-related".into(),
            operation_id: "create-related".into(),
            request_digest: DIGEST.into(),
            changes: vec![
                CatalogChange::CreateProduct {
                    record: related_record(ProductRecordKind::Dataset, "dataset-1", None),
                },
                CatalogChange::CreateProduct {
                    record: related_record(
                        ProductRecordKind::Chart,
                        "chart-1",
                        Some(ProductRecordReference {
                            kind: ProductRecordKind::Dataset,
                            id: "dataset-1".into(),
                        }),
                    ),
                },
            ],
        };
        assert!(matches!(
            catalog
                .commit_with_documents(request.clone(), documents_for(&request))
                .await,
            CommitOutcome::Committed(_)
        ));

        let reopened = catalog_with(storage).read_current().await.unwrap();
        reopened.validate().unwrap();
        assert!(
            reopened.product_records["chart/chart-1"]
                .references
                .contains(&ProductRecordReference {
                    kind: ProductRecordKind::Dataset,
                    id: "dataset-1".into(),
                })
        );
    }

    #[tokio::test]
    async fn product_documents_are_required_digest_checked_and_idempotent() {
        let storage = AdlsConditionalCommit::new(Arc::new(InMemory::new()));
        let catalog = catalog_with(storage.clone());
        let genesis = CatalogSnapshot::empty("snapshot-genesis").unwrap();
        catalog.initialize(genesis.clone()).await;
        let request = PrepareChange {
            base: genesis.reference(),
            snapshot_id: "snapshot-document".into(),
            operation_id: "create-document".into(),
            request_digest: DIGEST.into(),
            changes: vec![CatalogChange::CreateProduct {
                record: dashboard("dash-document", 1, "alice/document"),
            }],
        };

        assert_eq!(
            catalog.commit(request.clone()).await,
            CommitOutcome::Rejected
        );
        assert_eq!(catalog.read_current().await.unwrap(), genesis);

        let mut mismatched = documents_for(&request);
        *mismatched.values_mut().next().unwrap() = b"mismatched".to_vec();
        assert_eq!(
            catalog
                .commit_with_documents(request.clone(), mismatched)
                .await,
            CommitOutcome::Rejected
        );
        assert_eq!(catalog.read_current().await.unwrap(), genesis);

        let committed = catalog
            .commit_with_documents(request.clone(), documents_for(&request))
            .await;
        assert!(matches!(committed, CommitOutcome::Committed(_)));
        assert!(matches!(
            catalog.commit(request).await,
            CommitOutcome::Replayed(_)
        ));
        let reopened = catalog_with(storage).read_current().await.unwrap();
        assert!(
            reopened
                .product_records
                .contains_key("dashboard/dash-document")
        );
    }

    #[tokio::test]
    async fn conflicting_immutable_document_prevents_snapshot_publication() {
        let storage = AdlsConditionalCommit::new(Arc::new(InMemory::new()));
        let catalog = catalog_with(storage.clone());
        let genesis = CatalogSnapshot::empty("snapshot-genesis").unwrap();
        catalog.initialize(genesis.clone()).await;
        let request = PrepareChange {
            base: genesis.reference(),
            snapshot_id: "snapshot-conflicting-document".into(),
            operation_id: "conflicting-document".into(),
            request_digest: DIGEST.into(),
            changes: vec![CatalogChange::CreateProduct {
                record: dashboard("dash-conflict", 1, "alice/conflict"),
            }],
        };
        let path = required_documents(&request)
            .unwrap()
            .into_keys()
            .next()
            .unwrap();
        storage
            .create_immutable(&format!("product/{path}"), b"wrong-existing-bytes".to_vec())
            .await
            .unwrap();

        assert_eq!(
            catalog
                .commit_with_documents(request.clone(), documents_for(&request))
                .await,
            CommitOutcome::Conflict
        );
        assert_eq!(catalog.read_current().await.unwrap(), genesis);
    }

    #[tokio::test]
    async fn head_read_is_constant_and_document_fetch_detects_corruption() {
        let raw_store = Arc::new(InMemory::new());
        let storage = AdlsConditionalCommit::new(raw_store.clone());
        let catalog = catalog_with(storage);
        let genesis = CatalogSnapshot::empty("snapshot-genesis").unwrap();
        catalog.initialize(genesis.clone()).await;
        let request = PrepareChange {
            base: genesis.reference(),
            snapshot_id: "snapshot-corruption".into(),
            operation_id: "create-corruption-fixture".into(),
            request_digest: DIGEST.into(),
            changes: vec![CatalogChange::CreateProduct {
                record: dashboard("dash-corrupt", 1, "alice/corrupt"),
            }],
        };
        catalog
            .commit_with_documents(request.clone(), documents_for(&request))
            .await;
        let document_path = required_documents(&request)
            .unwrap()
            .into_keys()
            .next()
            .unwrap();
        raw_store
            .put(
                &Path::from(format!("product/{document_path}")),
                b"corrupted-after-publication".to_vec().into(),
            )
            .await
            .unwrap();

        // Reading the two-object head/snapshot path does not fan out over documents.
        assert_eq!(catalog.read_current().await.unwrap().generation, 1);
        assert_eq!(
            catalog
                .fetch_product_document(ProductRecordKind::Dashboard, "dash-corrupt")
                .await
                .unwrap_err(),
            CommitErrorKind::Invalid
        );
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
        let replayed = match catalog.commit(first).await {
            CommitOutcome::Replayed(snapshot) => snapshot,
            other => panic!("expected replay, got {other:?}"),
        };
        assert_eq!(replayed.snapshot_id, "snapshot-op-1");
        assert_eq!(replayed.generation, 1);
        assert_eq!(catalog.read_current().await.unwrap(), replayed);
        assert!(matches!(
            catalog.resolve_operation("op-1", &"b".repeat(64), 10).await,
            Ok(OperationResolution::Conflict)
        ));
    }

    #[tokio::test]
    async fn mutation_journal_replays_after_catalog_reopen() {
        let catalog = catalog();
        let genesis = CatalogSnapshot::empty("snapshot-genesis").unwrap();
        catalog.initialize(genesis.clone()).await;
        let request = request(genesis.reference(), "journal-op", "bronze.orders");
        let committed = match catalog.commit(request.clone()).await {
            CommitOutcome::Committed(snapshot) => snapshot,
            other => panic!("expected commit, got {other:?}"),
        };
        let reopened = ProductCatalogCommit::new(
            catalog.storage.clone(),
            "product",
            Arc::new(TransactionMetrics::default()),
        )
        .unwrap();
        let journal = reopened
            .resolve_mutation_journal("journal-op", DIGEST)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(journal.transaction_id, "journal-op");
        assert_eq!(journal.base_snapshot, Some(genesis.reference()));
        assert_eq!(journal.changes, request.changes);
        assert_eq!(journal.committed_snapshot, committed.reference());
        assert_eq!(journal.outcome, MutationJournalOutcome::Committed);
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
            operation_index: None,
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

    #[tokio::test]
    async fn indexed_oldest_operation_replays_after_more_than_one_hundred_commits() {
        let catalog = catalog();
        let genesis = CatalogSnapshot::empty("snapshot-genesis").unwrap();
        catalog.initialize(genesis.clone()).await;
        let first = request(genesis.reference(), "op-0", "bronze.orders");
        let mut current = match catalog.commit(first.clone()).await {
            CommitOutcome::Committed(value) => value,
            _ => panic!("first"),
        };
        for number in 1..=110 {
            current = match catalog
                .commit(request(
                    current.reference(),
                    &format!("op-{number}"),
                    "bronze.orders",
                ))
                .await
            {
                CommitOutcome::Committed(value) => value,
                _ => panic!("commit"),
            };
        }
        let replayed = match catalog.commit(first).await {
            CommitOutcome::Replayed(snapshot) => snapshot,
            other => panic!("expected replay, got {other:?}"),
        };
        assert_eq!(replayed.snapshot_id, "snapshot-op-0");
        assert_eq!(replayed.generation, 1);
        assert_eq!(catalog.read_current().await.unwrap(), current);
    }

    #[tokio::test]
    async fn missing_index_shard_refuses_to_advance_the_head() {
        let catalog = catalog();
        let genesis = CatalogSnapshot::empty("snapshot-genesis").unwrap();
        catalog.initialize(genesis.clone()).await;
        let first = request(genesis.reference(), "op-1", "bronze.orders");
        let current = match catalog.commit(first).await {
            CommitOutcome::Committed(value) => value,
            _ => panic!("first"),
        };
        let object = catalog
            .storage
            .read_bounded(&catalog.head_path(), MAX_HEAD_BYTES)
            .await
            .unwrap();
        let mut head: HeadRecord = decode(&object.bytes, MAX_HEAD_BYTES).unwrap();
        let key = digest(b"op-1")[..2].to_owned();
        head.operation_index
            .as_mut()
            .unwrap()
            .get_mut(&key)
            .unwrap()
            .path = "operation-index/missing.json".into();
        catalog
            .storage
            .compare_and_swap(
                &catalog.head_path(),
                &object.version,
                encode_head(&head).unwrap(),
            )
            .await
            .unwrap();
        assert!(matches!(
            catalog
                .commit(request(current.reference(), "op-1", "bronze.orders"))
                .await,
            CommitOutcome::Indeterminate
        ));
        assert!(matches!(
            catalog.resolve_mutation_journal("op-1", DIGEST).await,
            Err(CommitErrorKind::Missing)
        ));
        assert_eq!(catalog.read_current().await.unwrap(), current);
    }
}
