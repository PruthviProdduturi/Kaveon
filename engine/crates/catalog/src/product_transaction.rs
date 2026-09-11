//! Explicit transaction sessions over the durable product-catalog head.
//!
//! A session pins the head observed by `begin`, validates the complete staged
//! write set in memory, exposes that preview for read-your-writes, and submits
//! exactly one conditional publication on commit. Dropping or rolling back a
//! session publishes nothing.

use sha2::{Digest, Sha256};
use std::fmt;

use crate::{
    product_commit::{CommitOutcome, ProductCatalogCommit, ProductDocuments},
    product_manifest::{CatalogChange, CatalogSnapshot, PrepareChange, SnapshotRef},
};

/// Isolation currently provided by a product transaction.
///
/// `Snapshot` means the transaction reads one pinned catalog snapshot and
/// publishes only if that snapshot is still the current head. This is an
/// optimistic catalog-level guarantee, not general row-level MVCC.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TransactionIsolation {
    Snapshot,
}

/// Stable metadata for observability and conflict diagnostics.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TransactionMetadata {
    pub operation_id: String,
    pub base_snapshot: SnapshotRef,
    pub isolation: TransactionIsolation,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TransactionError {
    CannotReadHead,
    InvalidWriteSet(String),
}

impl fmt::Display for TransactionError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::CannotReadHead => formatter.write_str("cannot read the current catalog head"),
            Self::InvalidWriteSet(message) => write!(formatter, "invalid transaction: {message}"),
        }
    }
}

impl std::error::Error for TransactionError {}

/// One optimistic transaction. `commit` and `rollback` consume the session so
/// a caller cannot accidentally reuse it after completion.
pub struct ProductTransaction {
    catalog: ProductCatalogCommit,
    base: CatalogSnapshot,
    request: PrepareChange,
    documents: ProductDocuments,
    preview: Option<CatalogSnapshot>,
}

impl ProductTransaction {
    pub async fn begin(
        catalog: ProductCatalogCommit,
        snapshot_id: impl Into<String>,
        operation_id: impl Into<String>,
        request_digest: impl Into<String>,
    ) -> Result<Self, TransactionError> {
        let base = catalog
            .read_current()
            .await
            .map_err(|_| TransactionError::CannotReadHead)?;
        Ok(Self {
            catalog,
            request: PrepareChange {
                base: base.reference(),
                snapshot_id: snapshot_id.into(),
                operation_id: operation_id.into(),
                request_digest: request_digest.into(),
                changes: Vec::new(),
            },
            base,
            documents: ProductDocuments::new(),
            preview: None,
        })
    }

    /// Stages a write and validates the entire transaction. Failed staging is
    /// atomic: the previous staged state remains available.
    pub fn stage(&mut self, change: CatalogChange) -> Result<(), TransactionError> {
        let mut candidate = self.request.clone();
        candidate.changes.push(change);
        let preview = self
            .base
            .prepare(candidate.clone())
            .map_err(|error| TransactionError::InvalidWriteSet(error.to_string()))?;
        self.request = candidate;
        self.preview = Some(preview);
        Ok(())
    }

    /// Adds immutable bytes needed by staged product-record writes. Exact path
    /// and digest coverage is checked by the durable commit boundary.
    pub fn stage_document(&mut self, path: impl Into<String>, bytes: Vec<u8>) {
        self.documents.insert(path.into(), bytes);
    }

    #[must_use]
    pub fn base_snapshot(&self) -> &CatalogSnapshot {
        &self.base
    }

    /// Returns the identity and pinned snapshot used by this session.
    ///
    /// The operation ID is also the durable idempotency identity at commit;
    /// callers must not reuse it for a different request digest.
    #[must_use]
    pub fn metadata(&self) -> TransactionMetadata {
        TransactionMetadata {
            operation_id: self.request.operation_id.clone(),
            base_snapshot: self.request.base.clone(),
            isolation: TransactionIsolation::Snapshot,
        }
    }

    /// Returns the transaction-local snapshot, including all staged writes.
    #[must_use]
    pub fn snapshot(&self) -> &CatalogSnapshot {
        self.preview.as_ref().unwrap_or(&self.base)
    }

    #[must_use]
    pub fn staged_change_count(&self) -> usize {
        self.request.changes.len()
    }

    /// Rebinds the durable idempotency digest to the final ordered write set
    /// and trusted caller context before publication.
    pub fn bind_request_digest(&mut self, context: &[u8]) -> Result<(), TransactionError> {
        let changes = serde_json::to_vec(&self.request.changes)
            .map_err(|error| TransactionError::InvalidWriteSet(error.to_string()))?;
        let mut digest = Sha256::new();
        digest.update(b"kaveon-product-transaction-v1");
        digest.update((context.len() as u64).to_be_bytes());
        digest.update(context);
        digest.update((changes.len() as u64).to_be_bytes());
        digest.update(changes);
        self.request.request_digest = format!("{:x}", digest.finalize());
        if !self.request.changes.is_empty() {
            self.preview = Some(
                self.base
                    .prepare(self.request.clone())
                    .map_err(|error| TransactionError::InvalidWriteSet(error.to_string()))?,
            );
        }
        Ok(())
    }

    pub async fn commit(self) -> Result<CommitOutcome, TransactionError> {
        if self.request.changes.is_empty() {
            return Err(TransactionError::InvalidWriteSet(
                "at least one change is required".to_owned(),
            ));
        }
        Ok(self
            .catalog
            .commit_with_documents(self.request, self.documents)
            .await)
    }

    /// Ends the session without storage writes and returns its pinned base.
    #[must_use]
    pub fn rollback(self) -> CatalogSnapshot {
        self.base
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use kaveon_storage::AdlsConditionalCommit;
    use object_store::memory::InMemory;

    use super::*;
    use crate::{
        product_manifest::{ImmutableFileRef, TableManifestRef},
        product_metrics::TransactionMetrics,
    };

    const DIGEST: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";

    fn table(name: &str) -> TableManifestRef {
        TableManifestRef {
            manifest: ImmutableFileRef {
                path: format!("manifests/{name}.json"),
                sha256: DIGEST.into(),
            },
            parquet_files: Vec::new(),
        }
    }

    async fn catalog() -> ProductCatalogCommit {
        let storage = AdlsConditionalCommit::new(Arc::new(InMemory::new()));
        let catalog =
            ProductCatalogCommit::new(storage, "catalog", Arc::new(TransactionMetrics::default()))
                .unwrap();
        assert!(matches!(
            catalog
                .initialize(CatalogSnapshot::empty("snapshot-genesis").unwrap())
                .await,
            CommitOutcome::Committed(_)
        ));
        catalog
    }

    #[tokio::test]
    async fn staged_writes_are_visible_together_and_commit_atomically() {
        let catalog = catalog().await;
        let mut transaction =
            ProductTransaction::begin(catalog.clone(), "snapshot-1", "operation-1", DIGEST)
                .await
                .unwrap();
        transaction
            .stage(CatalogChange::Put {
                table: "app.users".into(),
                reference: table("users"),
            })
            .unwrap();
        transaction
            .stage(CatalogChange::Put {
                table: "app.orders".into(),
                reference: table("orders"),
            })
            .unwrap();

        assert_eq!(transaction.base_snapshot().tables.len(), 0);
        assert_eq!(transaction.snapshot().tables.len(), 2);
        assert_eq!(transaction.staged_change_count(), 2);
        assert!(matches!(
            transaction.commit().await.unwrap(),
            CommitOutcome::Committed(_)
        ));
        assert_eq!(catalog.read_current().await.unwrap().tables.len(), 2);
    }

    #[tokio::test]
    async fn metadata_pins_identity_and_base_snapshot() {
        let catalog = catalog().await;
        let transaction = ProductTransaction::begin(catalog, "snapshot-1", "operation-1", DIGEST)
            .await
            .unwrap();
        assert_eq!(
            transaction.metadata(),
            TransactionMetadata {
                operation_id: "operation-1".into(),
                base_snapshot: SnapshotRef {
                    generation: 0,
                    snapshot_id: "snapshot-genesis".into(),
                },
                isolation: TransactionIsolation::Snapshot,
            }
        );
    }

    #[tokio::test]
    async fn rollback_does_not_publish_staged_writes() {
        let catalog = catalog().await;
        let mut transaction =
            ProductTransaction::begin(catalog.clone(), "snapshot-1", "operation-1", DIGEST)
                .await
                .unwrap();
        transaction
            .stage(CatalogChange::Put {
                table: "app.users".into(),
                reference: table("users"),
            })
            .unwrap();
        let base = transaction.rollback();

        assert_eq!(base.generation, 0);
        assert!(catalog.read_current().await.unwrap().tables.is_empty());
    }

    #[tokio::test]
    async fn failed_stage_preserves_the_prior_transaction_view() {
        let catalog = catalog().await;
        let mut transaction =
            ProductTransaction::begin(catalog, "snapshot-1", "operation-1", DIGEST)
                .await
                .unwrap();
        let change = CatalogChange::Put {
            table: "app.users".into(),
            reference: table("users"),
        };
        transaction.stage(change.clone()).unwrap();
        assert!(transaction.stage(change).is_err());
        assert_eq!(transaction.staged_change_count(), 1);
        assert_eq!(transaction.snapshot().tables.len(), 1);
    }

    #[tokio::test]
    async fn concurrent_sessions_from_one_head_have_one_commit_winner() {
        let catalog = catalog().await;
        let mut left = ProductTransaction::begin(catalog.clone(), "snapshot-left", "left", DIGEST)
            .await
            .unwrap();
        let mut right =
            ProductTransaction::begin(catalog.clone(), "snapshot-right", "right", DIGEST)
                .await
                .unwrap();
        left.stage(CatalogChange::Put {
            table: "app.left".into(),
            reference: table("left"),
        })
        .unwrap();
        right
            .stage(CatalogChange::Put {
                table: "app.right".into(),
                reference: table("right"),
            })
            .unwrap();

        let left_metadata = left.metadata();
        let right_metadata = right.metadata();
        assert_eq!(left_metadata.base_snapshot, right_metadata.base_snapshot);
        assert_ne!(left_metadata.operation_id, right_metadata.operation_id);
        assert_eq!(left_metadata.isolation, TransactionIsolation::Snapshot);

        let (left, right) = tokio::join!(left.commit(), right.commit());
        let outcomes = [left.unwrap(), right.unwrap()];
        assert_eq!(
            outcomes
                .iter()
                .filter(|outcome| matches!(outcome, CommitOutcome::Committed(_)))
                .count(),
            1
        );
        assert_eq!(
            outcomes
                .iter()
                .filter(|outcome| matches!(outcome, CommitOutcome::Conflict))
                .count(),
            1
        );
    }

    #[tokio::test]
    async fn empty_transaction_cannot_commit() {
        let catalog = catalog().await;
        let transaction = ProductTransaction::begin(catalog, "snapshot-1", "operation-1", DIGEST)
            .await
            .unwrap();
        assert!(matches!(
            transaction.commit().await,
            Err(TransactionError::InvalidWriteSet(_))
        ));
    }

    #[tokio::test]
    async fn final_digest_binds_ordered_changes_and_principal_context() {
        let catalog = catalog().await;
        let mut alice =
            ProductTransaction::begin(catalog.clone(), "snapshot-a", "operation-a", DIGEST)
                .await
                .unwrap();
        alice
            .stage(CatalogChange::Put {
                table: "app.users".into(),
                reference: table("users"),
            })
            .unwrap();
        alice.bind_request_digest(b"alice").unwrap();
        let mut bob = ProductTransaction::begin(catalog, "snapshot-b", "operation-b", DIGEST)
            .await
            .unwrap();
        bob.stage(CatalogChange::Put {
            table: "app.users".into(),
            reference: table("users"),
        })
        .unwrap();
        bob.bind_request_digest(b"bob").unwrap();

        assert_ne!(alice.snapshot().request_digest, DIGEST);
        assert_ne!(
            alice.snapshot().request_digest,
            bob.snapshot().request_digest
        );
    }
}
