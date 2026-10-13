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

use kaveon_storage::{
    AdlsConditionalCommit, CommitErrorKind, ImmutableParquetWriter, ObjectVersion,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::{
    product_manifest::{
        CatalogChange, CatalogSnapshot, ImmutableFileRef, PrepareChange, SnapshotRef, TypedTableRef,
    },
    product_metrics::{TransactionMetrics, TransactionOutcome},
};

const MAX_HEAD_BYTES: usize = 64 * 1024;
const MAX_SNAPSHOT_BYTES: usize = 8 * 1024 * 1024;
const MAX_INDEX_SHARD_ENTRIES: usize = 1_024;
/// The byte ceiling for one operation-index shard, honoured identically where a
/// shard is written and where it is read.
///
/// Those two were different: shards were encoded under MAX_SNAPSHOT_BYTES and
/// read under MAX_HEAD_BYTES, so the Engine could publish a shard it could never
/// read back, and the operations hashing to it failed from then on. A single
/// constant used on both sides is what makes that impossible rather than
/// unlikely.
///
/// The entry cap above cannot do this job. An OperationRecord carries the whole
/// change set of its operation, so a count says nothing about size. This is
/// generous enough that eviction is rare in ordinary use and small enough that a
/// shard read stays a single bounded object read.
const MAX_INDEX_SHARD_BYTES: usize = 512 * 1024;
/// A commit that loses the race for the head is rebased and retried this many
/// times before the conflict is handed back.
///
/// Sized from measurement, not taste: with N writers serialising on one head a
/// writer can lose N-1 races before its own turn comes, so a budget equal to
/// the write concurrency is the floor, not the ceiling. At eight writers an
/// eight-attempt budget still surfaced the occasional conflict — roughly one
/// write in fifty — because a loser that backs off can be overtaken again.
/// Sixteen clears that, and the backoff below caps the worst case at about a
/// second and a half of waiting rather than a failed write.
const MAX_COMMIT_REBASE_ATTEMPTS: u32 = 16;
const COMMIT_RETRY_BASE_DELAY_MS: u64 = 4;
const COMMIT_RETRY_JITTER_MS: u64 = 24;
const COMMIT_RETRY_MAX_DELAY_MS: u64 = 400;
/// `validate_snapshot_id` bounds an identifier at 128 bytes; a rebase suffix
/// must not push a caller's ID past it.
const MAX_SNAPSHOT_ID_BYTES: usize = 128;
const MAX_PRODUCT_DOCUMENT_BYTES: usize = 8 * 1024 * 1024;

pub type ProductDocuments = BTreeMap<String, Vec<u8>>;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImmutableTableObjects {
    pub manifest_path: String,
    pub manifest_bytes: Vec<u8>,
    pub parquet_objects: BTreeMap<String, Vec<u8>>,
}

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

/// Result of resolving a transaction after a lost response or process restart.
/// `Prepared` is deliberately distinct from `NotFound`: a prepared journal
/// entry proves that immutable intent reached storage, but not that the head
/// CAS committed it. Callers must not retry it blindly or expose its writes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MutationRecovery {
    Committed(MutationJournalRecord),
    Prepared(MutationJournalRecord),
    NotFound,
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
            Err(error) if error.kind == CommitErrorKind::Conflict => {
                // Startup can race with another coordinator initializing the
                // same store. Reconcile the durable head before reporting a
                // conflict; an identical verified genesis is a successful
                // replay, while a different head must still fail closed.
                if let Ok(current) = self.read_current().await
                    && current.reference() == genesis.reference()
                    && current.operation_id == genesis.operation_id
                    && current.request_digest == genesis.request_digest
                {
                    attempt.finish(TransactionOutcome::Replayed);
                    return CommitOutcome::Replayed(current);
                }
                return finish_storage(attempt, error.kind);
            }
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
            Err(error) if error.kind == CommitErrorKind::Conflict => {
                // The head may have been created by a concurrent initializer
                // after our genesis object write. Verify it instead of
                // turning a safe replay into an indeterminate startup error.
                if let Ok(current) = self.read_current().await
                    && current.reference() == genesis.reference()
                    && current.operation_id == genesis.operation_id
                    && current.request_digest == genesis.request_digest
                {
                    attempt.finish(TransactionOutcome::Replayed);
                    return CommitOutcome::Replayed(current);
                }
                finish_storage(attempt, error.kind)
            }
            Err(error) => finish_storage(attempt, error.kind),
        }
    }

    /// Returns payload-free transaction counters for operational health and
    /// retirement evidence. The counters are process-local and approximate
    /// during a concurrent scrape; they are never used for commit decisions.
    #[must_use]
    pub fn transaction_metrics(&self) -> crate::product_metrics::TransactionMetricsSnapshot {
        self.metrics.snapshot()
    }

    /// Publishes the prepared immutable snapshot, then conditionally advances the head.
    pub async fn commit(&self, request: PrepareChange) -> CommitOutcome {
        self.commit_with_documents(request, BTreeMap::new()).await
    }

    /// Writes one complete immutable table object set, verifies that the
    /// staged table reference names those exact bytes, then publishes the
    /// catalog head through the normal conditional commit. Object writes are
    /// preparatory: a failed head CAS leaves them unreachable and harmless.
    pub async fn commit_table_objects(
        &self,
        request: PrepareChange,
        objects: ImmutableTableObjects,
    ) -> CommitOutcome {
        let writer = ImmutableParquetWriter::with_default_limit(self.storage.clone());
        let manifest = match writer
            .write_manifest(
                &format!("{}/{}", self.prefix, objects.manifest_path),
                objects.manifest_bytes,
            )
            .await
        {
            Ok(reference) => reference,
            Err(error) => return map_data_write_error(error),
        };
        let mut parquet_files = Vec::with_capacity(objects.parquet_objects.len());
        for (path, bytes) in objects.parquet_objects {
            let reference = match writer
                .write(&format!("{}/{}", self.prefix, path), bytes)
                .await
            {
                Ok(reference) => reference,
                Err(error) => return map_data_write_error(error),
            };
            parquet_files.push(ImmutableFileRef {
                path,
                sha256: reference.sha256,
            });
        }
        parquet_files.sort_by(|left, right| left.path.cmp(&right.path));
        let reference = match crate::product_manifest::TableManifestRef::validated(
            ImmutableFileRef {
                path: objects.manifest_path,
                sha256: manifest.sha256,
            },
            parquet_files,
        ) {
            Ok(reference) => reference,
            Err(_) => return CommitOutcome::Rejected,
        };
        let Some(CatalogChange::Put {
            reference: staged, ..
        }) = (request.changes.len() == 1)
            .then(|| request.changes.first())
            .flatten()
        else {
            return CommitOutcome::Rejected;
        };
        if staged != &reference {
            return CommitOutcome::Rejected;
        }
        // The request already carries the validated exact reference. Keeping
        // this method explicit prevents callers from accidentally publishing
        // a manifest that was not prepared by the immutable writer.
        self.commit(request).await
    }

    /// Publishes create-only product documents before advancing the snapshot head.
    /// Only documents referenced by product creates or updates are accepted.
    ///
    /// A commit that loses the race for the head is rebased onto the new head and
    /// retried, up to [`MAX_COMMIT_REBASE_ATTEMPTS`] times. Losing that race says
    /// nothing about this commit's own write set: every row precondition — the
    /// revision on an update or delete, the absence of a key on a create — is
    /// re-checked by `CatalogSnapshot::prepare` against whatever head the retry
    /// lands on. A write set that is still valid commits; one that another writer
    /// has invalidated is rejected on its own merits and never retried.
    ///
    /// The rebase is safe to do here because `request_digest` covers the actor and
    /// the changes only — not `base` — so the operation keeps its identity across
    /// attempts and the deduplication index still recognises a genuine replay.
    /// Each attempt mints its own snapshot ID, because snapshot objects are
    /// create-only and a rebased snapshot has different bytes.
    pub async fn commit_with_documents(
        &self,
        request: PrepareChange,
        documents: ProductDocuments,
    ) -> CommitOutcome {
        let original_snapshot_id = request.snapshot_id.clone();
        let mut request = request;
        for attempt in 0..=MAX_COMMIT_REBASE_ATTEMPTS {
            if attempt > 0 {
                // A fresh snapshot ID per attempt; the previous attempt's object
                // is orphaned, never committed, and must not be overwritten.
                let candidate = format!("{original_snapshot_id}-r{attempt}");
                if candidate.len() > MAX_SNAPSHOT_ID_BYTES {
                    return CommitOutcome::Conflict;
                }
                request.snapshot_id = candidate;
                self.backoff(attempt, &request.operation_id).await;
            }
            let (outcome, retry) = self
                .commit_with_documents_once(request.clone(), documents.clone())
                .await;
            if !retry || attempt == MAX_COMMIT_REBASE_ATTEMPTS {
                return outcome;
            }
        }
        CommitOutcome::Conflict
    }

    /// Sleeps a little before rebasing, so a crowd of writers that collided does
    /// not re-collide in the same order. The delay grows with the attempt and is
    /// spread by the operation ID, which gives each caller its own offset without
    /// a random number generator in the commit path.
    async fn backoff(&self, attempt: u32, operation_id: &str) {
        let step = COMMIT_RETRY_BASE_DELAY_MS.saturating_mul(1u64 << (attempt - 1).min(5));
        let spread = u64::from(digest(operation_id.as_bytes()).as_bytes()[0]);
        let delay = step.saturating_add(spread % COMMIT_RETRY_JITTER_MS.max(1));
        tokio::time::sleep(std::time::Duration::from_millis(
            delay.min(COMMIT_RETRY_MAX_DELAY_MS),
        ))
        .await;
    }

    /// One commit attempt. The second element of the pair is `true` when the
    /// attempt lost a race it can simply run again — nothing was committed and
    /// no precondition of its own failed — and `false` when the outcome is final:
    /// committed, replayed, rejected on its own write set, or indeterminate.
    async fn commit_with_documents_once(
        &self,
        request: PrepareChange,
        documents: ProductDocuments,
    ) -> (CommitOutcome, bool) {
        let attempt = self.metrics.begin();
        let head = match self.read_head().await {
            Ok(head) => head,
            Err(error) => {
                eprintln!(
                    "[product-commit] indeterminate: head read failed ({:?})",
                    error
                );
                return (finish_indeterminate(attempt), false);
            }
        };
        let Some(index) = &head.operation_index else {
            eprintln!(
                "[product-commit] indeterminate: head carries no operation index, so a replay \
cannot be told from a new operation"
            );
            attempt.finish(TransactionOutcome::Indeterminate);
            return (CommitOutcome::Indeterminate, false);
        };
        let shard_key = digest(request.operation_id.as_bytes())[..2].to_owned();
        let mut shard = match self.read_shard(index.get(&shard_key)).await {
            Ok(value) => value,
            Err(error) => {
                eprintln!(
                    "[product-commit] indeterminate: operation-index shard {} read failed ({:?})",
                    shard_key, error
                );
                return (finish_indeterminate(attempt), false);
            }
        };
        if let Some(existing) = shard.entries.get(&request.operation_id) {
            if existing.request_digest != request.request_digest {
                attempt.finish(TransactionOutcome::Conflict);
                return (CommitOutcome::Conflict, false);
            }
            match self.read_snapshot_with_bytes(&existing.snapshot).await {
                Ok((snapshot, bytes)) if digest(&bytes) == existing.snapshot_sha256 => {
                    attempt.finish(TransactionOutcome::Replayed);
                    return (CommitOutcome::Replayed(snapshot), false);
                }
                Ok(_) => return (finish_indeterminate(attempt), false),
                Err(_) => return (finish_indeterminate(attempt), false),
            }
        }
        // Rebase onto the head actually in hand rather than refusing a commit
        // whose base has moved — but only when every change in the write set
        // states its own precondition, so `prepare` can judge it against this
        // head instead of trusting the base. A write set another writer has
        // invalidated is then rejected there, on its own merits.
        let mut request = request;
        if request.base != head.snapshot.reference() {
            if !is_rebasable(&request) {
                attempt.finish(TransactionOutcome::Conflict);
                return (CommitOutcome::Conflict, false);
            }
            request.base = head.snapshot.reference();
        }
        let next = match head.snapshot.prepare(request.clone()) {
            Ok(snapshot) => snapshot,
            Err(error) => {
                // prepare() names the violation — a stale revision, a dangling
                // reference, a duplicate key — and this is the only place that
                // message existed before it was discarded.
                eprintln!(
                    "[product-commit] rejected by prepare: {error}; operation {}",
                    request.operation_id
                );
                attempt.finish(TransactionOutcome::Rejected);
                return (CommitOutcome::Rejected, false);
            }
        };
        let required_documents = match required_documents(&request) {
            Ok(required) => required,
            Err(()) => {
                attempt.finish(TransactionOutcome::Rejected);
                return (CommitOutcome::Rejected, false);
            }
        };
        if documents.len() != required_documents.len()
            || documents
                .keys()
                .any(|path| !required_documents.contains_key(path))
        {
            attempt.finish(TransactionOutcome::Rejected);
            return (CommitOutcome::Rejected, false);
        }
        for (path, reference) in &required_documents {
            let Some(document) = documents.get(path) else {
                attempt.finish(TransactionOutcome::Rejected);
                return (CommitOutcome::Rejected, false);
            };
            if document.len() > MAX_PRODUCT_DOCUMENT_BYTES || digest(document) != reference.sha256 {
                attempt.finish(TransactionOutcome::Rejected);
                return (CommitOutcome::Rejected, false);
            }
        }
        for (path, document) in documents {
            match self
                .publish_document(&required_documents[&path], document)
                .await
            {
                Ok(()) => {}
                Err(kind) => return (finish_storage(attempt, kind), false),
            }
        }
        // Carve the typed tables out before encoding. What gets published and
        // digested is the manifest — references to the tables, not the rows —
        // so the snapshot's size no longer grows with the catalog's contents.
        let (manifest, pending_tables) = match split_typed_tables(&next) {
            Ok(split) => split,
            Err(()) => {
                eprintln!(
                    "[product-commit] rejected: a typed table exceeds MAX_TYPED_TABLE_BYTES ({} bytes); operation {}",
                    MAX_TYPED_TABLE_BYTES, request.operation_id
                );
                attempt.finish(TransactionOutcome::Rejected);
                return (CommitOutcome::Rejected, false);
            }
        };
        // Create-only and content-addressed, published before the head moves,
        // so a snapshot never references a table that is not there. An
        // interrupted commit leaves orphaned table objects, which are
        // unreachable rather than wrong — the same property the product
        // documents already rely on.
        for (path, table_bytes) in &pending_tables {
            match self
                .storage
                .create_immutable(&format!("{}/{}", self.prefix, path), table_bytes.clone())
                .await
            {
                Ok(_) => {}
                // Named by its own digest, so an object already there holds
                // these very bytes and the write is already done.
                Err(error) if error.kind == CommitErrorKind::Conflict => {}
                Err(error) => return (finish_storage(attempt, error.kind), false),
            }
        }
        let bytes = match encode(&manifest) {
            Ok(bytes) => bytes,
            Err(()) => {
                // The size is reported because the bound alone does not say how
                // far over it is, and the difference between "one row too many"
                // and "twice the limit" decides whether pruning or a structural
                // change is the fix.
                eprintln!(
                    "[product-commit] rejected: encoded snapshot exceeds MAX_SNAPSHOT_BYTES ({} bytes); \
operation {}",
                    MAX_SNAPSHOT_BYTES, request.operation_id
                );
                attempt.finish(TransactionOutcome::Rejected);
                return (CommitOutcome::Rejected, false);
            }
        };
        let snapshot_sha256 = digest(&bytes);
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
        // Keep the shard inside the bound its reader enforces. A shard that
        // outgrew that bound became unreadable, and every operation hashing to
        // it then failed permanently — the state this installation was found
        // in, with one shard at 227 KB against a 64 KiB read.
        let before = shard.entries.len();
        trim_shard(&mut shard, &request.operation_id);
        if shard.entries.len() != before {
            eprintln!(
                "[product-commit] operation-index shard {} trimmed {} settled entries to stay within {} bytes",
                shard_key,
                before - shard.entries.len(),
                MAX_INDEX_SHARD_BYTES
            );
        }
        let mut next_index = index.clone();
        let shard_ref = match self.publish_shard(&shard).await {
            Ok(value) => value,
            Err(kind) => return (finish_storage(attempt, kind), false),
        };
        next_index.insert(shard_key, shard_ref);
        match self.publish_snapshot(&next.snapshot_id, bytes).await {
            Ok(_) => {}
            Err(kind) => return (finish_storage(attempt, kind), false),
        }
        let head_bytes = match encode_head(&HeadRecord {
            reference: next.reference(),
            snapshot_sha256,
            operation_index: Some(next_index),
        }) {
            Ok(bytes) => bytes,
            Err(()) => {
                attempt.finish(TransactionOutcome::Rejected);
                return (CommitOutcome::Rejected, false);
            }
        };
        match self
            .storage
            .compare_and_swap(&self.head_path(), &head.version, head_bytes)
            .await
        {
            Ok(_) => {
                attempt.finish(TransactionOutcome::Committed);
                (CommitOutcome::Committed(next), false)
            }
            Err(error) => {
                // The head moved between our read and this write. The staged
                // shard and snapshot stay orphaned and invisible — the old head
                // still names the old operation index — so nothing was committed
                // and the attempt can simply run again against the new head.
                let retry = error.kind == CommitErrorKind::Conflict && is_rebasable(&request);
                (finish_storage(attempt, error.kind), retry)
            }
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

    /// Reads and verifies one immutable file a snapshot references by path
    /// and digest — a statistics document, for one — bounded like a product
    /// document.
    pub async fn fetch_immutable_file(
        &self,
        file: &crate::product_manifest::ImmutableFileRef,
    ) -> Result<Vec<u8>, CommitErrorKind> {
        let object = self
            .storage
            .read_bounded(
                &format!("{}/{}", self.prefix, file.path),
                MAX_PRODUCT_DOCUMENT_BYTES,
            )
            .await
            .map_err(|error| error.kind)?;
        if digest(&object.bytes) != file.sha256 {
            return Err(CommitErrorKind::Invalid);
        }
        Ok(object.bytes)
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
        // The operation may no longer be the current head after later
        // commits. Resolve the retained parent chain instead of comparing only
        // with the current snapshot; otherwise every successful historical
        // transaction would be misreported as merely prepared after restart.
        let outcome = match self
            .resolve_from(
                head.snapshot.clone(),
                transaction_id,
                request_digest,
                MAX_INDEX_SHARD_ENTRIES,
            )
            .await?
        {
            OperationResolution::Committed(_) => MutationJournalOutcome::Committed,
            OperationResolution::Conflict => return Err(CommitErrorKind::Conflict),
            OperationResolution::NotCommitted | OperationResolution::Unresolved => {
                MutationJournalOutcome::Prepared
            }
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

    /// Resolves a transaction using only durable journal/head state. This is
    /// the restart-safe API for callers that lost a commit response; it never
    /// replays writes and never treats an absent entry as proof of non-commit
    /// beyond the retained operation index.
    pub async fn recover_transaction(
        &self,
        transaction_id: &str,
        request_digest: &str,
    ) -> Result<MutationRecovery, CommitErrorKind> {
        match self
            .resolve_mutation_journal(transaction_id, request_digest)
            .await?
        {
            Some(record) => Ok(match record.outcome {
                MutationJournalOutcome::Committed => MutationRecovery::Committed(record),
                MutationJournalOutcome::Prepared => MutationRecovery::Prepared(record),
            }),
            None => Ok(MutationRecovery::NotFound),
        }
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
        // Before validation, because validation reasons about the rows and a
        // manifest on its own carries only references to them.
        let snapshot = self.materialise_typed_tables(snapshot).await?;
        if snapshot.reference() != *reference || snapshot.validate().is_err() {
            return Err(CommitErrorKind::Invalid);
        }
        // The manifest bytes, which are what was published and what the head
        // and the operation index record a digest of.
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

    /// Load the rows for every table a manifest references.
    ///
    /// A snapshot written before typed tables moved out of line carries its
    /// rows inline and references nothing, so it is already materialised and
    /// is returned untouched — those snapshots stay readable, and the first
    /// commit after this change writes the new form.
    async fn materialise_typed_tables(
        &self,
        mut snapshot: CatalogSnapshot,
    ) -> Result<CatalogSnapshot, CommitErrorKind> {
        if snapshot.typed_tables.is_empty() {
            return Ok(snapshot);
        }
        for (table, reference) in snapshot.typed_tables.clone() {
            if !valid_digest(&reference.document.sha256) {
                return Err(CommitErrorKind::Invalid);
            }
            let bytes = self
                .storage
                .read_bounded(
                    &format!("{}/{}", self.prefix, reference.document.path),
                    MAX_TYPED_TABLE_BYTES,
                )
                .await
                .map_err(|error| error.kind)?
                .bytes;
            // The object is named by its own digest, so a mismatch is
            // corruption rather than a stale read, and must not be served.
            if digest(&bytes) != reference.document.sha256 {
                return Err(CommitErrorKind::Invalid);
            }
            let rows = decode(&bytes, MAX_TYPED_TABLE_BYTES).ok_or(CommitErrorKind::Invalid)?;
            snapshot.typed_rows.insert(table, rows);
        }
        Ok(snapshot)
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
                MAX_INDEX_SHARD_BYTES,
            )
            .await
            .map_err(|error| error.kind)?
            .bytes;
        if digest(&bytes) != reference.sha256 {
            return Err(CommitErrorKind::Invalid);
        }
        let shard: OperationShard =
            decode(&bytes, MAX_INDEX_SHARD_BYTES).ok_or(CommitErrorKind::Invalid)?;
        if shard.entries.len() > MAX_INDEX_SHARD_ENTRIES {
            return Err(CommitErrorKind::Invalid);
        }
        Ok(shard)
    }
    async fn publish_shard(
        &self,
        shard: &OperationShard,
    ) -> Result<ImmutableIndexRef, CommitErrorKind> {
        // Encoded under the shard's own bound, so a shard that would be
        // unreadable is never published in the first place. `trim_shard` has
        // already brought it inside the bound; this is the assertion that it
        // did, not a second attempt at it.
        let bytes =
            encode_bounded(shard, MAX_INDEX_SHARD_BYTES).map_err(|_| CommitErrorKind::Invalid)?;
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
    encode_bounded(value, MAX_SNAPSHOT_BYTES)
}

/// Encode under an explicit ceiling. Every object this module publishes has a
/// bound that its reader enforces; passing that bound in is what keeps the two
/// from drifting apart.
fn encode_bounded<T: Serialize>(value: &T, max_bytes: usize) -> Result<Vec<u8>, ()> {
    let bytes = serde_json::to_vec(value).map_err(|_| ())?;
    if bytes.len() > max_bytes {
        return Err(());
    }
    Ok(bytes)
}

/// One typed table's object, read and written under the same ceiling.
///
/// Generous relative to a snapshot, because this holds one table rather than
/// the whole catalog, and a table is read on its own and written only when it
/// changes. The snapshot it was carved out of is now a few kilobytes of
/// references whatever the tables grow to.
const MAX_TYPED_TABLE_BYTES: usize = 64 * 1024 * 1024;

/// Separate a materialised snapshot into the manifest that is published and
/// the typed-table objects it refers to.
///
/// A table whose rows are byte-identical to the parent's keeps the reference it
/// already had and is not republished. That is the whole point: a commit used
/// to rewrite every row in the catalog — saving a favourite republished 5.3 MB
/// of query history — and now writes only the tables it touched.
#[allow(clippy::type_complexity)]
fn split_typed_tables(
    snapshot: &CatalogSnapshot,
) -> Result<(CatalogSnapshot, Vec<(String, Vec<u8>)>), ()> {
    let mut manifest = snapshot.clone();
    manifest.typed_rows = BTreeMap::new();
    manifest.typed_tables = BTreeMap::new();
    let mut pending = Vec::new();
    for (table, rows) in &snapshot.typed_rows {
        let bytes = encode_bounded(rows, MAX_TYPED_TABLE_BYTES)?;
        let sha256 = digest(&bytes);
        let path = format!("typed-tables/{sha256}.json");
        // The parent's reference for this table. Equal digests mean equal
        // bytes, so the object is already published and nothing is written.
        let unchanged = snapshot
            .typed_tables
            .get(table)
            .is_some_and(|reference| reference.document.sha256 == sha256);
        if !unchanged {
            pending.push((path.clone(), bytes));
        }
        manifest.typed_tables.insert(
            table.clone(),
            TypedTableRef {
                document: ImmutableFileRef { path, sha256 },
                rows: rows.len() as u64,
            },
        );
    }
    Ok((manifest, pending))
}

/// Drop the oldest entries until the shard encodes within its bound.
///
/// The index exists to recognise a retry of an operation whose outcome the
/// caller is unsure of, and a retry follows its original by seconds. So the
/// useful content is a recent window, and holding every operation a shard has
/// ever seen buys nothing while growing without limit.
///
/// Oldest is lowest snapshot generation: the operations whose outcome has been
/// settled longest. `keep` is the entry being recorded now and is never
/// evicted — evicting it would discard the very fact the commit is writing.
fn trim_shard(shard: &mut OperationShard, keep: &str) {
    while encode_bounded(shard, MAX_INDEX_SHARD_BYTES).is_err()
        || shard.entries.len() > MAX_INDEX_SHARD_ENTRIES
    {
        let Some(oldest) = shard
            .entries
            .iter()
            .filter(|(id, _)| id.as_str() != keep)
            .min_by_key(|(id, record)| (record.snapshot.generation, (*id).clone()))
            .map(|(id, _)| id.clone())
        else {
            // Only the entry being recorded is left. Nothing further can be
            // given up, and publishing will fail on its own bound rather than
            // this loop spinning.
            return;
        };
        shard.entries.remove(&oldest);
    }
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

/// True when every change in the write set carries the precondition it needs to
/// be judged against a head other than the one the caller read.
///
/// Product records and typed rows do: a create fails if the key already exists,
/// an update or delete fails unless the expected revision still stands. Those
/// can be rebased, because `CatalogSnapshot::prepare` re-checks them in full.
///
/// Table, statistics, runtime-source, control and schema publications do not.
/// They overwrite whatever is present, so the base snapshot the caller read *is*
/// their precondition, and rebasing one would let a stale publication land on
/// top of a newer one. Those keep the strict generation check.
fn is_rebasable(request: &PrepareChange) -> bool {
    request.changes.iter().all(|change| {
        matches!(
            change,
            CatalogChange::CreateProduct { .. }
                | CatalogChange::UpdateProduct { .. }
                | CatalogChange::DeleteProduct { .. }
                | CatalogChange::InsertTypedRow { .. }
                | CatalogChange::UpdateTypedRow { .. }
                | CatalogChange::DeleteTypedRow { .. }
        )
    })
}

fn finish_indeterminate(attempt: crate::product_metrics::TransactionAttempt<'_>) -> CommitOutcome {
    attempt.finish(TransactionOutcome::Indeterminate);
    CommitOutcome::Indeterminate
}

fn map_data_write_error(error: kaveon_storage::DataWriteError) -> CommitOutcome {
    match error {
        kaveon_storage::DataWriteError::Conflict => CommitOutcome::Conflict,
        kaveon_storage::DataWriteError::Invalid(_) => CommitOutcome::Rejected,
        kaveon_storage::DataWriteError::Storage(kind) => match kind {
            CommitErrorKind::Conflict => CommitOutcome::Conflict,
            CommitErrorKind::Invalid
            | CommitErrorKind::LimitExceeded
            | CommitErrorKind::Unsupported => CommitOutcome::Rejected,
            _ => CommitOutcome::Indeterminate,
        },
    }
}

fn finish_storage(
    attempt: crate::product_metrics::TransactionAttempt<'_>,
    kind: CommitErrorKind,
) -> CommitOutcome {
    // The kind is what separates a refusal from an unknown, and it was only
    // ever used to pick an outcome — never reported.
    eprintln!("[product-commit] storage outcome: {kind:?}");
    let (outcome, metric) = match kind {
        CommitErrorKind::Conflict => (CommitOutcome::Conflict, TransactionOutcome::Conflict),
        // A size limit is a decision, not an unknown. The snapshot either fits
        // the bound or it does not, and the answer is the same on every retry,
        // so reporting it as indeterminate tells a caller its commit might have
        // landed when it provably did not — and sends operators looking for a
        // transient fault that is really a structural one.
        CommitErrorKind::LimitExceeded => (CommitOutcome::Rejected, TransactionOutcome::Rejected),
        CommitErrorKind::Retryable | CommitErrorKind::Other | CommitErrorKind::Unsupported => (
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
        ProductRecordReference, TableManifestRef, TypedRow, TypedValue,
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

    #[tokio::test]
    async fn initialize_reconciles_an_existing_genesis_as_replay() {
        let catalog = catalog();
        let genesis = CatalogSnapshot::empty("snapshot-genesis").unwrap();
        assert!(matches!(
            catalog.initialize(genesis.clone()).await,
            CommitOutcome::Committed(_)
        ));

        // A restart after another coordinator completed initialization must
        // reconcile the durable head instead of reporting a startup conflict.
        assert!(matches!(
            catalog.initialize(genesis).await,
            CommitOutcome::Replayed(_)
        ));
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

    fn parquet_bytes(payload: &[u8]) -> Vec<u8> {
        let mut bytes = b"PAR1".to_vec();
        bytes.extend_from_slice(payload);
        bytes.extend_from_slice(b"PAR1");
        bytes
    }

    fn table_request_with_objects(
        base: SnapshotRef,
        id: &str,
        table_name: &str,
        manifest_path: &str,
        parquet_path: &str,
        manifest_bytes: &[u8],
        parquet_bytes: &[u8],
    ) -> (PrepareChange, ImmutableTableObjects) {
        let reference = TableManifestRef::validated(
            ImmutableFileRef {
                path: manifest_path.into(),
                sha256: digest(manifest_bytes),
            },
            vec![ImmutableFileRef {
                path: parquet_path.into(),
                sha256: digest(parquet_bytes),
            }],
        )
        .unwrap();
        (
            PrepareChange {
                base,
                snapshot_id: format!("snapshot-{id}"),
                operation_id: id.into(),
                request_digest: DIGEST.into(),
                changes: vec![CatalogChange::Put {
                    table: table_name.into(),
                    reference,
                }],
            },
            ImmutableTableObjects {
                manifest_path: manifest_path.into(),
                manifest_bytes: manifest_bytes.to_vec(),
                parquet_objects: BTreeMap::from([(parquet_path.into(), parquet_bytes.to_vec())]),
            },
        )
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

    #[tokio::test]
    async fn table_objects_publish_atomically_and_replay_after_retry() {
        let catalog = catalog();
        let genesis = CatalogSnapshot::empty("snapshot-genesis").unwrap();
        catalog.initialize(genesis.clone()).await;
        let parquet = parquet_bytes(b"orders");
        let (request, objects) = table_request_with_objects(
            genesis.reference(),
            "table-1",
            "bronze.orders",
            "tables/orders/manifest.json",
            "tables/orders/part.parquet",
            br#"{"schema":"orders"}"#,
            &parquet,
        );
        let committed = catalog
            .commit_table_objects(request.clone(), objects.clone())
            .await;
        assert!(matches!(committed, CommitOutcome::Committed(_)));
        let current = catalog.read_current().await.unwrap();
        assert_eq!(current.tables["bronze.orders"].parquet_files.len(), 1);
        assert!(matches!(
            catalog.commit_table_objects(request, objects).await,
            CommitOutcome::Replayed(_)
        ));
    }

    #[tokio::test]
    async fn divergent_object_rejection_leaves_table_head_unchanged() {
        let catalog = catalog();
        let genesis = CatalogSnapshot::empty("snapshot-genesis").unwrap();
        catalog.initialize(genesis.clone()).await;
        let parquet = parquet_bytes(b"orders");
        let (request, objects) = table_request_with_objects(
            genesis.reference(),
            "table-1",
            "bronze.orders",
            "tables/orders/manifest.json",
            "tables/orders/part.parquet",
            br#"{"schema":"orders"}"#,
            &parquet,
        );
        assert!(matches!(
            catalog.commit_table_objects(request, objects).await,
            CommitOutcome::Committed(_)
        ));
        let before = catalog.read_current().await.unwrap();
        let (request, objects) = table_request_with_objects(
            before.reference(),
            "table-2",
            "bronze.orders",
            "tables/orders/manifest.json",
            "tables/orders/part.parquet",
            br#"{"schema":"orders"}"#,
            &parquet_bytes(b"different"),
        );
        assert_eq!(
            catalog.commit_table_objects(request, objects).await,
            CommitOutcome::Conflict
        );
        assert_eq!(catalog.read_current().await.unwrap(), before);
    }

    #[tokio::test]
    async fn stale_table_publication_leaves_prepared_objects_unreachable() {
        let catalog = catalog();
        let genesis = CatalogSnapshot::empty("snapshot-genesis").unwrap();
        catalog.initialize(genesis.clone()).await;
        let parquet = parquet_bytes(b"orders");
        let (request, _objects) = table_request_with_objects(
            genesis.reference(),
            "table-1",
            "bronze.orders",
            "tables/orders/manifest.json",
            "tables/orders/part.parquet",
            br#"{"schema":"orders"}"#,
            &parquet,
        );
        let prepared_base = request.base.clone();
        assert!(matches!(
            catalog.commit(request).await,
            CommitOutcome::Committed(_)
        ));
        let before_stale = catalog.read_current().await.unwrap();
        let (stale_request, stale_objects) = table_request_with_objects(
            prepared_base,
            "table-stale",
            "bronze.stale",
            "tables/stale/manifest.json",
            "tables/stale/part.parquet",
            br#"{"schema":"stale"}"#,
            &parquet_bytes(b"stale"),
        );
        assert_eq!(
            catalog
                .commit_table_objects(stale_request, stale_objects)
                .await,
            CommitOutcome::Conflict
        );
        assert_eq!(catalog.read_current().await.unwrap(), before_stale);
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
    async fn concurrent_creates_of_distinct_records_all_commit() {
        // The defect this covers: every one of these writers creates a
        // different dashboard, so no write set contends with any other, yet
        // only one used to commit per snapshot generation and the rest were
        // refused. All of them must now land.
        let storage = AdlsConditionalCommit::new(Arc::new(InMemory::new()));
        let catalog = Arc::new(catalog_with(storage.clone()));
        let genesis = CatalogSnapshot::empty("snapshot-genesis").unwrap();
        catalog.initialize(genesis.clone()).await;

        let base = genesis.reference();
        let create = move |n: usize, base: SnapshotRef| PrepareChange {
            base,
            snapshot_id: format!("snapshot-create-{n}"),
            operation_id: format!("create-{n}"),
            request_digest: DIGEST.into(),
            changes: vec![CatalogChange::CreateProduct {
                record: dashboard(&format!("dash-{n}"), 1, &format!("alice/{n}")),
            }],
        };

        let writers = 8;
        let mut handles = Vec::new();
        for n in 0..writers {
            let catalog = Arc::clone(&catalog);
            let base = base.clone();
            handles.push(tokio::spawn(async move {
                let request = create(n, base);
                catalog
                    .commit_with_documents(request.clone(), documents_for(&request))
                    .await
            }));
        }
        let mut committed = 0;
        for handle in handles {
            match handle.await.unwrap() {
                CommitOutcome::Committed(_) => committed += 1,
                other => panic!("a distinct-record create did not commit: {other:?}"),
            }
        }
        assert_eq!(committed, writers);

        // Every record is durable, and the generations are the serial order the
        // commits actually took — one per committed write, none lost.
        let reopened = catalog_with(storage).read_current().await.unwrap();
        assert_eq!(reopened.generation, writers as u64);
        for n in 0..writers {
            assert_eq!(
                reopened.product_records[&format!("dashboard/dash-{n}")].revision,
                1
            );
        }
    }

    #[tokio::test]
    async fn a_stale_record_revision_is_rejected_and_never_retried() {
        // The retry must not paper over a real conflict: once another writer has
        // advanced the row, a write set carrying the old expected revision has
        // to fail, however many times it is rebased.
        let storage = AdlsConditionalCommit::new(Arc::new(InMemory::new()));
        let catalog = catalog_with(storage.clone());
        let genesis = CatalogSnapshot::empty("snapshot-genesis").unwrap();
        catalog.initialize(genesis.clone()).await;
        let create = PrepareChange {
            base: genesis.reference(),
            snapshot_id: "snapshot-created".into(),
            operation_id: "create-dashboard".into(),
            request_digest: DIGEST.into(),
            changes: vec![CatalogChange::CreateProduct {
                record: dashboard("dash-1", 1, "alice/home"),
            }],
        };
        let created = match catalog
            .commit_with_documents(create.clone(), documents_for(&create))
            .await
        {
            CommitOutcome::Committed(snapshot) => snapshot,
            other => panic!("unexpected create outcome: {other:?}"),
        };

        let first = PrepareChange {
            base: created.reference(),
            snapshot_id: "snapshot-first".into(),
            operation_id: "update-first".into(),
            request_digest: DIGEST.into(),
            changes: vec![CatalogChange::UpdateProduct {
                expected_revision: 1,
                record: dashboard("dash-1", 2, "alice/first"),
            }],
        };
        assert!(matches!(
            catalog
                .commit_with_documents(first.clone(), documents_for(&first))
                .await,
            CommitOutcome::Committed(_)
        ));

        // Still expecting revision 1, which no longer exists.
        let stale = PrepareChange {
            base: created.reference(),
            snapshot_id: "snapshot-stale".into(),
            operation_id: "update-stale".into(),
            request_digest: DIGEST.into(),
            changes: vec![CatalogChange::UpdateProduct {
                expected_revision: 1,
                record: dashboard("dash-1", 2, "alice/stale"),
            }],
        };
        assert_eq!(
            catalog
                .commit_with_documents(stale.clone(), documents_for(&stale))
                .await,
            CommitOutcome::Rejected
        );
        let reopened = catalog_with(storage).read_current().await.unwrap();
        assert_eq!(
            reopened.product_records["dashboard/dash-1"].unique_values["owner_name"].as_str(),
            "alice/first"
        );
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
        let recovery = reopened
            .recover_transaction("journal-op", DIGEST)
            .await
            .unwrap();
        assert!(matches!(recovery, MutationRecovery::Committed(_)));
        assert!(matches!(
            reopened
                .recover_transaction("missing-after-restart", DIGEST)
                .await
                .unwrap(),
            MutationRecovery::NotFound
        ));

        let current = reopened.read_current().await.unwrap();
        let later = super::tests::request(current.reference(), "later-op", "bronze.customers");
        assert!(matches!(
            reopened.commit(later).await,
            CommitOutcome::Committed(_)
        ));
        assert!(matches!(
            reopened
                .recover_transaction("journal-op", DIGEST)
                .await
                .unwrap(),
            MutationRecovery::Committed(_)
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

    /// An operation ID whose digest lands on `shard_key`, so a test can pile
    /// entries into one shard instead of spreading them over 256.
    fn id_on_shard(shard_key: &str, nonce: usize) -> String {
        for attempt in 0.. {
            let candidate = format!("op-{nonce}-{attempt}");
            if digest(candidate.as_bytes())[..2] == *shard_key {
                return candidate;
            }
        }
        unreachable!()
    }

    #[tokio::test]
    async fn a_large_typed_table_does_not_grow_the_snapshot() {
        // The property that stops this recurring on a fresh installation.
        //
        // Typed rows were serialised into the snapshot, and a commit
        // republishes the whole snapshot — so every write cost the size of the
        // entire catalog, and the snapshot had a ceiling it could reach. A
        // production catalog stopped accepting writes of any kind with 5,650
        // rows of query history filling an 8 MiB snapshot, while its datasets,
        // charts and dashboards together came to 74 KB.
        //
        // One table is filled past that old ceiling here. Every commit must
        // still succeed, and the published manifest must stay small, because
        // it carries a reference to the table instead of its rows.
        let catalog = catalog();
        let genesis = CatalogSnapshot::empty("snapshot-genesis").unwrap();
        catalog.initialize(genesis.clone()).await;

        // ~80 KB a row, so 120 rows is near 10 MB — past what a snapshot may
        // encode to, with few enough commits to stay a fast test.
        let payload = "x".repeat(80_000);
        let mut current = genesis;
        for number in 0..120u32 {
            let mut columns = BTreeMap::new();
            columns.insert("payload".to_string(), TypedValue::String(payload.clone()));
            let request = PrepareChange {
                base: current.reference(),
                snapshot_id: format!("snapshot-row-{number}"),
                operation_id: format!("op-row-{number}"),
                request_digest: DIGEST.into(),
                changes: vec![CatalogChange::InsertTypedRow {
                    table: "query_history".into(),
                    row: TypedRow {
                        primary_key: format!("row-{number}"),
                        revision: 1,
                        columns,
                        unique_keys: BTreeMap::new(),
                    },
                }],
            };
            current = match catalog.commit(request).await {
                CommitOutcome::Committed(value) => value,
                outcome => panic!("row {number} did not commit: {outcome:?}"),
            };
        }

        assert_eq!(
            current.typed_rows["query_history"].len(),
            120,
            "every row must be present after materialisation"
        );
        let inline = encode_bounded(&current.typed_rows, usize::MAX)
            .unwrap()
            .len();
        assert!(
            inline > MAX_SNAPSHOT_BYTES,
            "the test must exceed the old ceiling to prove anything; rows were {inline} bytes"
        );

        // The manifest is the thing with a ceiling, and it holds a reference.
        let (manifest, _) = split_typed_tables(&current).unwrap();
        let bytes = encode(&manifest).expect("the manifest must encode well inside its bound");
        assert!(
            bytes.len() < 64 * 1024,
            "a snapshot of references should stay small; was {} bytes",
            bytes.len()
        );
        assert!(
            manifest.typed_rows.is_empty(),
            "rows do not belong in the manifest"
        );
        assert_eq!(manifest.typed_tables["query_history"].rows, 120);

        // A write to one table must not republish another. This is why a
        // commit now costs what it touched rather than what the catalog holds.
        let mut columns = BTreeMap::new();
        columns.insert("note".to_string(), TypedValue::String("small".into()));
        let request = PrepareChange {
            base: current.reference(),
            snapshot_id: "snapshot-other-table".into(),
            operation_id: "op-other-table".into(),
            request_digest: DIGEST.into(),
            changes: vec![CatalogChange::InsertTypedRow {
                table: "favorites".into(),
                row: TypedRow {
                    primary_key: "fav-1".into(),
                    revision: 1,
                    columns,
                    unique_keys: BTreeMap::new(),
                },
            }],
        };
        let after = match catalog.commit(request).await {
            CommitOutcome::Committed(value) => value,
            outcome => panic!("the unrelated write did not commit: {outcome:?}"),
        };
        let (_, pending) = split_typed_tables(&after).unwrap();
        let rewritten: Vec<&String> = pending.iter().map(|(path, _)| path).collect();
        assert_eq!(
            rewritten.len(),
            1,
            "only the table that changed should be written; got {rewritten:?}"
        );
    }

    #[tokio::test]
    async fn a_shard_that_outgrows_its_bound_stays_readable() {
        // The defect this reproduces: shards were encoded under
        // MAX_SNAPSHOT_BYTES (8 MiB) and read under MAX_HEAD_BYTES (64 KiB), so
        // the Engine could publish a shard its own reader refused. Every
        // operation hashing to that shard then failed with Indeterminate,
        // permanently. A production catalog was found in exactly that state,
        // with one shard at 227 KB, and stopped accepting writes entirely.
        //
        // Every operation here is forced onto one shard so the entries
        // accumulate in a single object, which is what makes it outgrow a
        // bound at all. An entry carries its operation's whole change set, so
        // the shard passes 64 KiB well before its 1,024-entry cap — the count
        // bound never could have prevented this.
        let catalog = catalog();
        let genesis = CatalogSnapshot::empty("snapshot-genesis").unwrap();
        catalog.initialize(genesis.clone()).await;

        let mut current = genesis;
        let mut published: Option<ImmutableIndexRef> = None;
        for nonce in 0..260 {
            let id = id_on_shard("b4", nonce);
            current = match catalog
                .commit(request(current.reference(), &id, "bronze.orders"))
                .await
            {
                CommitOutcome::Committed(value) => value,
                outcome => panic!("commit {nonce} did not succeed: {outcome:?}"),
            };
            let head = catalog.read_head().await.expect("head");
            published = head
                .operation_index
                .as_ref()
                .and_then(|index| index.get("b4"))
                .cloned();
        }

        // What was published must be readable. This is the invariant the two
        // mismatched bounds broke, and reading it back is the whole test.
        let reference = published.expect("shard b4 was never published");
        let shard = catalog
            .read_shard(Some(&reference))
            .await
            .expect("a published shard must be readable");
        assert!(
            encode_bounded(&shard, MAX_INDEX_SHARD_BYTES).is_ok(),
            "a readable shard must also be publishable under the same bound"
        );

        // And the catalog still takes writes on that shard afterwards, which it
        // did not before: a wedged shard stayed wedged forever.
        let id = id_on_shard("b4", 9_999);
        assert!(matches!(
            catalog
                .commit(request(current.reference(), &id, "bronze.orders"))
                .await,
            CommitOutcome::Committed(_)
        ));
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
