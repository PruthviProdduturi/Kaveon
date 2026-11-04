//! Authenticated, owner-isolated API sessions for product-catalog transactions.

use std::{
    cmp::Ordering,
    collections::HashMap,
    sync::Arc,
    time::{Duration, Instant},
};

use axum::{
    Extension, Json, Router,
    extract::{Path, Query, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::{get, post},
};
use futures::{StreamExt, stream};
use kaveon_catalog::{
    product_commit::{CommitOutcome, OperationResolution, ProductCatalogCommit},
    product_manifest::{
        CatalogChange, CatalogSnapshot, ImmutableFileRef, ProductRecordKind, ProductRecordRef,
        ProductRecordReference, TypedRow, TypedValue,
    },
    product_transaction::ProductTransaction,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use tokio::sync::Mutex;
use uuid::Uuid;

use crate::{AppState, security::Identity};

const MAX_SESSIONS: usize = 64;
const MAX_SESSIONS_PER_PRINCIPAL: usize = 8;
const MAX_SESSION_DOCUMENT_BYTES: usize = 16 * 1024 * 1024;
const MAX_PRODUCT_INSERT_SELECT_ROWS: usize = 1_000;
const MAX_PRODUCT_READ_CONCURRENCY: usize = 16;
const SESSION_TTL: Duration = Duration::from_secs(5 * 60);
type StagedDocument = (String, Vec<u8>);
type ProductStage = (CatalogChange, Option<StagedDocument>);
type ProductDocument = (
    ImmutableFileRef,
    StagedDocument,
    BTreeSet<ProductRecordReference>,
    BTreeMap<String, String>,
);

pub fn routes() -> Router<Arc<AppState>> {
    Router::new()
        .route("/v1/transaction", post(begin))
        .route("/v1/transaction/{transaction_id}/stage", post(stage))
        .route(
            "/v1/transaction/{transaction_id}/stage-sql-batch",
            post(stage_sql_batch),
        )
        .route("/v1/transaction/{transaction_id}/commit", post(commit))
        .route("/v1/transaction/{transaction_id}/rollback", post(rollback))
        .route("/v1/transaction/{transaction_id}/recovery", get(recovery))
        .route("/v1/transaction/metrics", get(metrics))
        .route("/v1/transaction/sql", post(execute_sql))
        .route("/v1/products/{kind}", get(list_products))
        .route("/v1/product/{kind}/{id}", get(read_product))
        // KaveonDB-owned control-plane rows.  These are deliberately separate
        // from product records: system metadata is administrator-readable and
        // is never owner-filtered through the product API.
        .route("/v1/system/{table}", get(list_system_rows))
        .route("/v1/system/{table}/{id}", get(read_system_row))
        .route("/v1/system/{table}/query", post(query_system_rows))
}

#[derive(Clone)]
pub struct TransactionRegistry {
    catalog: Option<ProductCatalogCommit>,
    sessions: Arc<Mutex<HashMap<String, Session>>>,
    ttl: Duration,
}

struct Session {
    owner: String,
    expires_at: Instant,
    document_bytes: usize,
    transaction: ProductTransaction,
}

#[derive(Debug, PartialEq, Eq)]
enum RegistryError {
    Disabled,
    Capacity,
    Missing,
    ProductMissing,
    SessionForbidden,
    OwnershipForbidden,
    Conflict,
    Invalid(String),
    Corrupt,
    Unavailable,
}

impl TransactionRegistry {
    pub(crate) fn catalog(&self) -> Option<ProductCatalogCommit> {
        self.catalog.clone()
    }

    fn metrics(
        &self,
    ) -> Result<kaveon_catalog::product_metrics::TransactionMetricsSnapshot, RegistryError> {
        self.catalog
            .as_ref()
            .map(ProductCatalogCommit::transaction_metrics)
            .ok_or(RegistryError::Disabled)
    }
    pub fn disabled() -> Self {
        Self {
            catalog: None,
            sessions: Arc::new(Mutex::new(HashMap::new())),
            ttl: SESSION_TTL,
        }
    }

    pub(crate) fn enabled(catalog: ProductCatalogCommit) -> Self {
        Self {
            catalog: Some(catalog),
            sessions: Arc::new(Mutex::new(HashMap::new())),
            ttl: SESSION_TTL,
        }
    }

    async fn begin(&self, owner: &str) -> Result<BeginResponse, RegistryError> {
        let catalog = self.catalog.clone().ok_or(RegistryError::Disabled)?;
        {
            let mut sessions = self.sessions.lock().await;
            expire(&mut sessions);
            if sessions.len() >= MAX_SESSIONS
                || sessions
                    .values()
                    .filter(|session| session.owner == owner)
                    .count()
                    >= MAX_SESSIONS_PER_PRINCIPAL
            {
                return Err(RegistryError::Capacity);
            }
        }
        let transaction_id = Uuid::new_v4().to_string();
        let transaction = ProductTransaction::begin(
            catalog,
            format!("snapshot-{}", transaction_id.replace('-', "")),
            format!("operation-{}", transaction_id.replace('-', "")),
            "0".repeat(64),
        )
        .await
        .map_err(|error| RegistryError::Invalid(error.to_string()))?;
        let base = transaction.base_snapshot().clone();
        let operation_id = transaction.metadata().operation_id;
        let mut sessions = self.sessions.lock().await;
        expire(&mut sessions);
        if sessions.len() >= MAX_SESSIONS
            || sessions
                .values()
                .filter(|session| session.owner == owner)
                .count()
                >= MAX_SESSIONS_PER_PRINCIPAL
        {
            return Err(RegistryError::Capacity);
        }
        sessions.insert(
            transaction_id.clone(),
            Session {
                owner: owner.to_owned(),
                expires_at: Instant::now() + self.ttl,
                document_bytes: 0,
                transaction,
            },
        );
        Ok(BeginResponse {
            transaction_id,
            operation_id,
            base,
        })
    }

    async fn stage(
        &self,
        owner: &str,
        id: &str,
        request: StageRequest,
    ) -> Result<CatalogSnapshot, RegistryError> {
        reject_grant_changes(&request.change)?;
        let mut sessions = self.sessions.lock().await;
        expire(&mut sessions);
        let session = owned_session(&mut sessions, owner, id)?;
        let document_total = if let Some(document) = request.document.as_ref() {
            let total = session
                .document_bytes
                .checked_add(document.bytes.len())
                .ok_or(RegistryError::Capacity)?;
            if total > MAX_SESSION_DOCUMENT_BYTES {
                return Err(RegistryError::Capacity);
            }
            Some(total)
        } else {
            None
        };
        session
            .transaction
            .stage(request.change)
            .map_err(|error| RegistryError::Invalid(error.to_string()))?;
        if let Some(document) = request.document {
            session
                .transaction
                .stage_document(document.path, document.bytes);
            session.document_bytes = document_total.expect("document total was calculated");
        }
        session.expires_at = Instant::now() + self.ttl;
        Ok(session.transaction.snapshot().clone())
    }

    async fn savepoint(&self, owner: &str, id: &str, name: &str) -> Result<(), RegistryError> {
        let mut sessions = self.sessions.lock().await;
        expire(&mut sessions);
        let session = owned_session(&mut sessions, owner, id)?;
        session
            .transaction
            .savepoint(name.to_owned())
            .map_err(|error| RegistryError::Invalid(error.to_string()))?;
        session.expires_at = Instant::now() + self.ttl;
        Ok(())
    }

    async fn rollback_to_savepoint(
        &self,
        owner: &str,
        id: &str,
        name: &str,
    ) -> Result<CatalogSnapshot, RegistryError> {
        let mut sessions = self.sessions.lock().await;
        expire(&mut sessions);
        let session = owned_session(&mut sessions, owner, id)?;
        session
            .transaction
            .rollback_to_savepoint(name)
            .map_err(|error| RegistryError::Invalid(error.to_string()))?;
        session.document_bytes = session.transaction.staged_document_bytes();
        session.expires_at = Instant::now() + self.ttl;
        Ok(session.transaction.snapshot().clone())
    }

    async fn release_savepoint(
        &self,
        owner: &str,
        id: &str,
        name: &str,
    ) -> Result<(), RegistryError> {
        let mut sessions = self.sessions.lock().await;
        expire(&mut sessions);
        let session = owned_session(&mut sessions, owner, id)?;
        session
            .transaction
            .release_savepoint(name)
            .map_err(|error| RegistryError::Invalid(error.to_string()))?;
        session.expires_at = Instant::now() + self.ttl;
        Ok(())
    }

    async fn take(&self, owner: &str, id: &str) -> Result<ProductTransaction, RegistryError> {
        let mut sessions = self.sessions.lock().await;
        expire(&mut sessions);
        match sessions.get(id) {
            None => return Err(RegistryError::Missing),
            Some(session) if session.owner != owner => {
                return Err(RegistryError::SessionForbidden);
            }
            Some(_) => {}
        }
        Ok(sessions
            .remove(id)
            .expect("session was checked")
            .transaction)
    }

    #[allow(dead_code)]
    async fn stage_product_command(
        &self,
        owner: &str,
        id: &str,
        command: kaveon_sql::parser::ProductDmlCommand,
    ) -> Result<CatalogSnapshot, RegistryError> {
        let mut sessions = self.sessions.lock().await;
        expire(&mut sessions);
        let session = owned_session(&mut sessions, owner, id)?;
        let (change, document) = product_change(session.transaction.snapshot(), owner, command)?;
        reject_grant_changes(&change)?;
        let document_total =
            document
                .as_ref()
                .map_or(Ok(session.document_bytes), |(_, bytes)| {
                    session
                        .document_bytes
                        .checked_add(bytes.len())
                        .ok_or(RegistryError::Capacity)
                })?;
        if document_total > MAX_SESSION_DOCUMENT_BYTES {
            return Err(RegistryError::Capacity);
        }
        session
            .transaction
            .stage(change)
            .map_err(|error| RegistryError::Invalid(error.to_string()))?;
        if let Some((path, bytes)) = document {
            session.transaction.stage_document(path, bytes);
            session.document_bytes = document_total;
        }
        session.expires_at = Instant::now() + self.ttl;
        Ok(session.transaction.snapshot().clone())
    }

    async fn stage_product_commands(
        &self,
        owner: &str,
        id: &str,
        commands: Vec<kaveon_sql::parser::ProductDmlCommand>,
        capture_deleted_documents: bool,
    ) -> Result<
        (
            CatalogSnapshot,
            Vec<kaveon_sql::parser::ProductDmlCommand>,
            Vec<Option<String>>,
        ),
        RegistryError,
    > {
        let mut sessions = self.sessions.lock().await;
        expire(&mut sessions);
        let session = owned_session(&mut sessions, owner, id)?;
        // A multi-statement batch is one SQL statement from the caller's
        // perspective.  Preserve the prior staged view so a validation or
        // storage error cannot leave an earlier command in the batch staged
        // while the caller still has a live transaction it could commit.
        let transaction_before = session.transaction.clone();
        let document_bytes_before = session.document_bytes;
        let result = async {
            let mut expanded = Vec::new();
            let catalog = self.catalog.clone();
            let snapshot = session.transaction.snapshot().clone();
            for command in commands {
                if let kaveon_sql::parser::ProductDmlCommand::UpdateFrom {
                    kind,
                    id,
                    expected_revision,
                    source_id,
                    source_kind,
                } = command
                {
                    let source = record_kind(&source_kind)?;
                    let source_record = snapshot
                        .product_record(source, &source_id)
                        .map_err(|error| RegistryError::Invalid(error.to_string()))?
                        .ok_or(RegistryError::ProductMissing)?;
                    require_product_owner(source_record, owner)?;
                    let bytes = if let Some(bytes) = session
                        .transaction
                        .staged_document(&source_record.document.path)
                    {
                        bytes.to_vec()
                    } else {
                        let catalog = catalog.as_ref().ok_or(RegistryError::Disabled)?;
                        catalog
                            .fetch_product_document_at(&snapshot, source, &source_id)
                            .await
                            .map_err(|error| match error {
                                kaveon_storage::CommitErrorKind::Missing
                                | kaveon_storage::CommitErrorKind::Invalid => {
                                    RegistryError::Corrupt
                                }
                                _ => RegistryError::Unavailable,
                            })?
                            .ok_or(RegistryError::Corrupt)?
                    };
                    let document_json =
                        String::from_utf8(bytes).map_err(|_| RegistryError::Corrupt)?;
                    expanded.push(kaveon_sql::parser::ProductDmlCommand::Update {
                        kind,
                        id,
                        expected_revision,
                        document_json,
                    });
                } else if let kaveon_sql::parser::ProductDmlCommand::DeleteFrom {
                    kind,
                    id,
                    expected_revision,
                    source_id,
                    source_kind,
                } = command
                {
                    let source = record_kind(&source_kind)?;
                    let source_record = snapshot
                        .product_record(source, &source_id)
                        .map_err(|error| RegistryError::Invalid(error.to_string()))?
                        .ok_or(RegistryError::ProductMissing)?;
                    require_product_owner(source_record, owner)?;
                    expanded.push(kaveon_sql::parser::ProductDmlCommand::Delete {
                        kind,
                        id,
                        expected_revision,
                    });
                } else if let kaveon_sql::parser::ProductDmlCommand::InsertSelect {
                    kind,
                    source_kind,
                    source_ids,
                    on_conflict_do_nothing,
                    on_conflict_do_update,
                    ..
                } = command
                {
                    let catalog = catalog.as_ref().ok_or(RegistryError::Disabled)?;
                    let source = record_kind(&source_kind)?;
                    let mut cursor = None;
                    loop {
                        let page = snapshot
                            .product_records_page(source, 100, cursor.as_deref())
                            .map_err(|error| RegistryError::Invalid(error.to_string()))?;
                        for record in page.records {
                            if let Some(ids) = &source_ids
                                && !ids.iter().any(|id| id == &record.id)
                            {
                                continue;
                            }
                            if record
                                .unique_values
                                .get("owner_principal")
                                .map(String::as_str)
                                != Some(owner)
                            {
                                return Err(RegistryError::OwnershipForbidden);
                            }
                            if expanded.len() >= MAX_PRODUCT_INSERT_SELECT_ROWS {
                                return Err(RegistryError::Capacity);
                            }
                            let bytes = catalog
                                .fetch_product_document_at(&snapshot, source, &record.id)
                                .await
                                .map_err(|error| match error {
                                    kaveon_storage::CommitErrorKind::Missing
                                    | kaveon_storage::CommitErrorKind::Invalid => {
                                        RegistryError::Corrupt
                                    }
                                    _ => RegistryError::Unavailable,
                                })?
                                .ok_or(RegistryError::Corrupt)?;
                            let document_json =
                                String::from_utf8(bytes).map_err(|_| RegistryError::Corrupt)?;
                            expanded.push(if on_conflict_do_update {
                                kaveon_sql::parser::ProductDmlCommand::Upsert {
                                    kind: kind.clone(),
                                    id: record.id,
                                    document_json,
                                }
                            } else if on_conflict_do_nothing {
                                kaveon_sql::parser::ProductDmlCommand::CreateIgnore {
                                    kind: kind.clone(),
                                    id: record.id,
                                    document_json,
                                }
                            } else {
                                kaveon_sql::parser::ProductDmlCommand::Create {
                                    kind: kind.clone(),
                                    id: record.id,
                                    document_json,
                                }
                            });
                        }
                        cursor = page.next_cursor;
                        if cursor.is_none() {
                            break;
                        }
                    }
                } else if let kaveon_sql::parser::ProductDmlCommand::MergeDeleteNotMatched {
                    kind,
                    source_ids,
                    target_ids,
                } = command
                {
                    let target = record_kind(&kind)?;
                    let mut cursor = None;
                    loop {
                        let page = snapshot
                            .product_records_page(target, 100, cursor.as_deref())
                            .map_err(|error| RegistryError::Invalid(error.to_string()))?;
                        for record in page.records {
                            if source_ids.iter().any(|id| id == &record.id) {
                                continue;
                            }
                            if target_ids
                                .as_ref()
                                .is_some_and(|ids| !ids.iter().any(|id| id == &record.id))
                            {
                                continue;
                            }
                            if record
                                .unique_values
                                .get("owner_principal")
                                .map(String::as_str)
                                != Some(owner)
                            {
                                return Err(RegistryError::OwnershipForbidden);
                            }
                            if expanded.len() >= MAX_PRODUCT_INSERT_SELECT_ROWS {
                                return Err(RegistryError::Capacity);
                            }
                            expanded.push(kaveon_sql::parser::ProductDmlCommand::MergeDelete {
                                kind: kind.clone(),
                                id: record.id,
                            });
                        }
                        cursor = page.next_cursor;
                        if cursor.is_none() {
                            break;
                        }
                    }
                } else if let kaveon_sql::parser::ProductDmlCommand::MergeUpdateNotMatched {
                    kind,
                    source_ids,
                    target_ids,
                    document_json,
                } = command
                {
                    let target = record_kind(&kind)?;
                    let mut cursor = None;
                    loop {
                        let page = snapshot
                            .product_records_page(target, 100, cursor.as_deref())
                            .map_err(|error| RegistryError::Invalid(error.to_string()))?;
                        for record in page.records {
                            if source_ids.iter().any(|id| id == &record.id) {
                                continue;
                            }
                            if target_ids
                                .as_ref()
                                .is_some_and(|ids| !ids.iter().any(|id| id == &record.id))
                            {
                                continue;
                            }
                            if record
                                .unique_values
                                .get("owner_principal")
                                .map(String::as_str)
                                != Some(owner)
                            {
                                return Err(RegistryError::OwnershipForbidden);
                            }
                            if expanded.len() >= MAX_PRODUCT_INSERT_SELECT_ROWS {
                                return Err(RegistryError::Capacity);
                            }
                            expanded.push(kaveon_sql::parser::ProductDmlCommand::MergeUpdate {
                                kind: kind.clone(),
                                id: record.id,
                                document_json: document_json.clone(),
                            });
                        }
                        cursor = page.next_cursor;
                        if cursor.is_none() {
                            break;
                        }
                    }
                } else {
                    expanded.push(command);
                }
            }
            let mut staged_commands = Vec::new();
            let mut deleted_documents = Vec::new();
            for command in expanded {
                let command = match command {
                    kaveon_sql::parser::ProductDmlCommand::CreateIgnore {
                        kind,
                        id: row_id,
                        document_json,
                    } => {
                        if kind == "typed_row" {
                            let (table, row) =
                                typed_row_document(&row_id, &document_json, owner, 1)?;
                            if let Some(existing) = session
                                .transaction
                                .snapshot()
                                .typed_rows
                                .get(&table)
                                .and_then(|rows| rows.get(&row_id))
                            {
                                if existing == &row {
                                    session.transaction.mark_noop();
                                    continue;
                                }
                                return Err(RegistryError::Invalid(
                                    "typed row already exists with different content".into(),
                                ));
                            }
                        } else {
                            let product_kind = record_kind(&kind)?;
                            if session
                                .transaction
                                .snapshot()
                                .product_record(product_kind, &row_id)
                                .map_err(|error| RegistryError::Invalid(error.to_string()))?
                                .is_some()
                            {
                                session.transaction.mark_noop();
                                continue;
                            }
                        }
                        kaveon_sql::parser::ProductDmlCommand::Create {
                            kind,
                            id: row_id,
                            document_json,
                        }
                    }
                    kaveon_sql::parser::ProductDmlCommand::Upsert {
                        kind,
                        id: row_id,
                        document_json,
                    } => {
                        if kind == "typed_row" {
                            match find_typed_row(session.transaction.snapshot(), &row_id, owner) {
                                Ok((_table, current)) => {
                                    kaveon_sql::parser::ProductDmlCommand::Update {
                                        kind,
                                        id: row_id,
                                        expected_revision: current.revision,
                                        document_json,
                                    }
                                }
                                Err(RegistryError::Invalid(message))
                                    if message == "typed row does not exist" =>
                                {
                                    kaveon_sql::parser::ProductDmlCommand::Create {
                                        kind,
                                        id: row_id,
                                        document_json,
                                    }
                                }
                                Err(error) => return Err(error),
                            }
                        } else {
                            let product_kind = record_kind(&kind)?;
                            let current = session
                                .transaction
                                .snapshot()
                                .product_record(product_kind, &row_id)
                                .map_err(|error| RegistryError::Invalid(error.to_string()))?;
                            if let Some(current) = current {
                                require_product_owner(current, owner)?;
                                kaveon_sql::parser::ProductDmlCommand::Update {
                                    kind,
                                    id: row_id,
                                    expected_revision: current.revision,
                                    document_json,
                                }
                            } else {
                                kaveon_sql::parser::ProductDmlCommand::Create {
                                    kind,
                                    id: row_id,
                                    document_json,
                                }
                            }
                        }
                    }
                    kaveon_sql::parser::ProductDmlCommand::MergeUpdateOrCreateFiltered {
                        kind,
                        id: row_id,
                        document_json,
                        matched_ids,
                    } => {
                        let product_kind = record_kind(&kind)?;
                        let current = session
                            .transaction
                            .snapshot()
                            .product_record(product_kind, &row_id)
                            .map_err(|error| RegistryError::Invalid(error.to_string()))?;
                        if let Some(current) = current {
                            require_product_owner(current, owner)?;
                            if !matched_ids.iter().any(|id| id == &row_id) {
                                session.transaction.mark_noop();
                                continue;
                            }
                            kaveon_sql::parser::ProductDmlCommand::Update {
                                kind,
                                id: row_id,
                                expected_revision: current.revision,
                                document_json,
                            }
                        } else {
                            kaveon_sql::parser::ProductDmlCommand::Create {
                                kind,
                                id: row_id,
                                document_json,
                            }
                        }
                    }
                    kaveon_sql::parser::ProductDmlCommand::MergeDeleteOrInsert {
                        kind,
                        id: row_id,
                        document_json,
                    } => {
                        let product_kind = record_kind(&kind)?;
                        let current = session
                            .transaction
                            .snapshot()
                            .product_record(product_kind, &row_id)
                            .map_err(|error| RegistryError::Invalid(error.to_string()))?;
                        if let Some(current) = current {
                            require_product_owner(current, owner)?;
                            kaveon_sql::parser::ProductDmlCommand::Delete {
                                kind,
                                id: row_id,
                                expected_revision: current.revision,
                            }
                        } else {
                            kaveon_sql::parser::ProductDmlCommand::Create {
                                kind,
                                id: row_id,
                                document_json,
                            }
                        }
                    }
                    kaveon_sql::parser::ProductDmlCommand::MergeDeleteOrCreateFiltered {
                        kind,
                        id: row_id,
                        document_json,
                        matched_ids,
                    } => {
                        let product_kind = record_kind(&kind)?;
                        let current = session
                            .transaction
                            .snapshot()
                            .product_record(product_kind, &row_id)
                            .map_err(|error| RegistryError::Invalid(error.to_string()))?;
                        if let Some(current) = current {
                            require_product_owner(current, owner)?;
                            if !matched_ids.iter().any(|id| id == &row_id) {
                                session.transaction.mark_noop();
                                continue;
                            }
                            kaveon_sql::parser::ProductDmlCommand::Delete {
                                kind,
                                id: row_id,
                                expected_revision: current.revision,
                            }
                        } else {
                            kaveon_sql::parser::ProductDmlCommand::Create {
                                kind,
                                id: row_id,
                                document_json,
                            }
                        }
                    }
                    kaveon_sql::parser::ProductDmlCommand::MergeDelete { kind, id: row_id } => {
                        let product_kind = record_kind(&kind)?;
                        let current = session
                            .transaction
                            .snapshot()
                            .product_record(product_kind, &row_id)
                            .map_err(|error| RegistryError::Invalid(error.to_string()))?;
                        if let Some(current) = current {
                            require_product_owner(current, owner)?;
                            kaveon_sql::parser::ProductDmlCommand::Delete {
                                kind,
                                id: row_id,
                                expected_revision: current.revision,
                            }
                        } else {
                            session.transaction.mark_noop();
                            continue;
                        }
                    }
                    kaveon_sql::parser::ProductDmlCommand::MergeUpdate {
                        kind,
                        id: row_id,
                        document_json,
                    } => {
                        let product_kind = record_kind(&kind)?;
                        let current = session
                            .transaction
                            .snapshot()
                            .product_record(product_kind, &row_id)
                            .map_err(|error| RegistryError::Invalid(error.to_string()))?;
                        if let Some(current) = current {
                            require_product_owner(current, owner)?;
                            kaveon_sql::parser::ProductDmlCommand::Update {
                                kind,
                                id: row_id,
                                expected_revision: current.revision,
                                document_json,
                            }
                        } else {
                            session.transaction.mark_noop();
                            continue;
                        }
                    }
                    kaveon_sql::parser::ProductDmlCommand::MergeCreateIfAbsent {
                        kind,
                        id: row_id,
                        document_json,
                    } => {
                        let product_kind = record_kind(&kind)?;
                        if session
                            .transaction
                            .snapshot()
                            .product_record(product_kind, &row_id)
                            .map_err(|error| RegistryError::Invalid(error.to_string()))?
                            .is_some()
                        {
                            session.transaction.mark_noop();
                            continue;
                        }
                        kaveon_sql::parser::ProductDmlCommand::Create {
                            kind,
                            id: row_id,
                            document_json,
                        }
                    }
                    other => other,
                };
                // Migration batches are replayable.  A typed-row create that is
                // already present with the same immutable payload is an idempotent
                // no-op; this avoids turning a resumed batch into 1,000 point-read
                // reconciliations while still rejecting mismatched rows below.
                if let kaveon_sql::parser::ProductDmlCommand::Create {
                    kind,
                    id: row_id,
                    document_json,
                } = &command
                    && kind == "typed_row"
                {
                    let (table, row) = typed_row_document(row_id, document_json, owner, 1)?;
                    if let Some(existing) = session
                        .transaction
                        .snapshot()
                        .typed_rows
                        .get(&table)
                        .and_then(|rows| rows.get(row_id))
                        && existing == &row
                    {
                        continue;
                    }
                }
                let staged_command = command.clone();
                let deleted_document = if capture_deleted_documents {
                    self.capture_deleted_document(&session.transaction, owner, &command)
                        .await?
                } else {
                    None
                };
                let (change, document) =
                    product_change(session.transaction.snapshot(), owner, command)?;
                reject_grant_changes(&change)?;
                let document_total =
                    document
                        .as_ref()
                        .map_or(Ok(session.document_bytes), |(_, bytes)| {
                            session
                                .document_bytes
                                .checked_add(bytes.len())
                                .ok_or(RegistryError::Capacity)
                        })?;
                if document_total > MAX_SESSION_DOCUMENT_BYTES {
                    return Err(RegistryError::Capacity);
                }
                session
                    .transaction
                    .stage(change)
                    .map_err(|error| RegistryError::Invalid(error.to_string()))?;
                staged_commands.push(staged_command);
                deleted_documents.push(deleted_document);
                if let Some((path, bytes)) = document {
                    session.transaction.stage_document(path, bytes);
                    session.document_bytes = document_total;
                }
            }
            session.expires_at = Instant::now() + self.ttl;
            Ok((
                session.transaction.snapshot().clone(),
                staged_commands,
                deleted_documents,
            ))
        }
        .await;
        match result {
            Ok((snapshot, staged_commands, deleted_documents)) => {
                Ok((snapshot, staged_commands, deleted_documents))
            }
            Err(error) => {
                session.transaction = transaction_before;
                session.document_bytes = document_bytes_before;
                Err(error)
            }
        }
    }

    async fn capture_deleted_document(
        &self,
        transaction: &ProductTransaction,
        owner: &str,
        command: &kaveon_sql::parser::ProductDmlCommand,
    ) -> Result<Option<String>, RegistryError> {
        let kaveon_sql::parser::ProductDmlCommand::Delete {
            kind,
            id,
            expected_revision,
        } = command
        else {
            return Ok(None);
        };
        let snapshot = transaction.snapshot();
        if kind == "typed_row" {
            let (table, row) = find_typed_row(snapshot, id, owner)?;
            if row.revision != *expected_revision {
                return Err(RegistryError::Invalid("stale typed row revision".into()));
            }
            let owner_principal = match row.columns.get("owner_principal") {
                Some(TypedValue::String(value)) => value.clone(),
                _ => return Err(RegistryError::Corrupt),
            };
            let document = TypedRowDocument {
                table,
                primary_key: row.primary_key.clone(),
                revision: row.revision,
                columns: row.columns.clone(),
                unique_keys: row.unique_keys.clone(),
                owner_principal,
            };
            return Ok(Some(
                serde_json::to_string(&document).map_err(|_| RegistryError::Corrupt)?,
            ));
        }
        let product_kind = record_kind(kind)?;
        let record = snapshot
            .product_record(product_kind, id)
            .map_err(|error| RegistryError::Invalid(error.to_string()))?
            .ok_or(RegistryError::ProductMissing)?;
        require_product_owner(record, owner)?;
        if record.revision != *expected_revision {
            return Err(RegistryError::Invalid("stale product revision".into()));
        }
        let bytes = if let Some(bytes) = transaction.staged_document(&record.document.path) {
            bytes.to_vec()
        } else {
            let catalog = self.catalog.as_ref().ok_or(RegistryError::Disabled)?;
            catalog
                .fetch_product_document_at(snapshot, product_kind, id)
                .await
                .map_err(|error| match error {
                    kaveon_storage::CommitErrorKind::Missing
                    | kaveon_storage::CommitErrorKind::Invalid => RegistryError::Corrupt,
                    _ => RegistryError::Unavailable,
                })?
                .ok_or(RegistryError::Corrupt)?
        };
        Ok(Some(
            String::from_utf8(bytes).map_err(|_| RegistryError::Corrupt)?,
        ))
    }

    async fn source_copy_documents(
        &self,
        owner: &str,
        id: &str,
        kind: &str,
        source_ids: &[String],
    ) -> Result<HashMap<String, String>, RegistryError> {
        let catalog = self.catalog.clone().ok_or(RegistryError::Disabled)?;
        let mut sessions = self.sessions.lock().await;
        expire(&mut sessions);
        let session = owned_session(&mut sessions, owner, id)?;
        let snapshot = session.transaction.snapshot().clone();
        let product_kind = record_kind(kind)?;
        let mut documents = HashMap::with_capacity(source_ids.len());
        for source_id in source_ids {
            let Some(record) = snapshot
                .product_record(product_kind, source_id)
                .map_err(|error| RegistryError::Invalid(error.to_string()))?
            else {
                continue;
            };
            require_product_owner(record, owner)?;
            let bytes = catalog
                .fetch_product_document_at(&snapshot, product_kind, source_id)
                .await
                .map_err(|error| match error {
                    kaveon_storage::CommitErrorKind::Missing
                    | kaveon_storage::CommitErrorKind::Invalid => RegistryError::Corrupt,
                    _ => RegistryError::Unavailable,
                })?
                .ok_or(RegistryError::Corrupt)?;
            documents.insert(
                source_id.clone(),
                String::from_utf8(bytes).map_err(|_| RegistryError::Corrupt)?,
            );
        }
        Ok(documents)
    }

    async fn conflict_ignore_existing(
        &self,
        owner: &str,
        id: &str,
        commands: &[kaveon_sql::parser::ProductDmlCommand],
    ) -> Result<Vec<bool>, RegistryError> {
        let mut sessions = self.sessions.lock().await;
        expire(&mut sessions);
        let session = owned_session(&mut sessions, owner, id)?;
        let snapshot = session.transaction.snapshot();
        commands
            .iter()
            .map(|command| {
                let kaveon_sql::parser::ProductDmlCommand::CreateIgnore { kind, id, .. } = command
                else {
                    return Ok(false);
                };
                if kind == "typed_row" {
                    return match find_typed_row(snapshot, id, owner) {
                        Ok(_) => Ok(true),
                        Err(RegistryError::Invalid(message))
                            if message == "typed row does not exist" =>
                        {
                            Ok(false)
                        }
                        Err(error) => Err(error),
                    };
                }
                let product_kind = record_kind(kind)?;
                let Some(record) = snapshot
                    .product_record(product_kind, id)
                    .map_err(|error| RegistryError::Invalid(error.to_string()))?
                else {
                    return Ok(false);
                };
                require_product_owner(record, owner)?;
                Ok(true)
            })
            .collect()
    }

    async fn read_product(
        &self,
        identity: &Identity,
        kind: ProductRecordKind,
        id: &str,
    ) -> Result<ProductReadResponse, RegistryError> {
        let catalog = self.catalog.as_ref().ok_or(RegistryError::Disabled)?;
        let snapshot = catalog
            .read_current_shared()
            .await
            .map_err(|error| match error {
                kaveon_storage::CommitErrorKind::Missing
                | kaveon_storage::CommitErrorKind::Invalid => RegistryError::Corrupt,
                _ => RegistryError::Unavailable,
            })?;
        let record = snapshot
            .product_record(kind, id)
            .map_err(|error| RegistryError::Invalid(error.to_string()))?
            .cloned()
            .ok_or(RegistryError::ProductMissing)?;
        let owner = record.unique_values.get("owner_principal");
        if identity.role != crate::security::Role::Admin
            && owner.map(String::as_str) != Some(identity.principal.as_str())
        {
            return Err(RegistryError::OwnershipForbidden);
        }
        let bytes = catalog
            .fetch_product_document_at(&snapshot, kind, id)
            .await
            .map_err(|error| match error {
                kaveon_storage::CommitErrorKind::Missing
                | kaveon_storage::CommitErrorKind::Invalid => RegistryError::Corrupt,
                _ => RegistryError::Unavailable,
            })?
            .ok_or(RegistryError::Corrupt)?;
        let document = serde_json::from_slice(&bytes).map_err(|_| RegistryError::Corrupt)?;
        Ok(ProductReadResponse {
            kind,
            id: record.id,
            revision: record.revision,
            generation: snapshot.generation,
            snapshot_id: snapshot.snapshot_id.clone(),
            document,
        })
    }

    async fn list_products(
        &self,
        identity: &Identity,
        kind: ProductRecordKind,
        limit: usize,
        cursor: Option<&str>,
    ) -> Result<ProductListResponse, RegistryError> {
        let catalog = self.catalog.as_ref().ok_or(RegistryError::Disabled)?;
        let snapshot = catalog
            .read_current_shared()
            .await
            .map_err(|error| match error {
                kaveon_storage::CommitErrorKind::Missing
                | kaveon_storage::CommitErrorKind::Invalid => RegistryError::Corrupt,
                _ => RegistryError::Unavailable,
            })?;
        let mut records = Vec::with_capacity(limit);
        // Authorization filtering must happen before pagination is exposed to
        // callers. Otherwise a page containing another user's records can
        // return fewer rows and advance the cursor past records the caller is
        // allowed to see.
        let mut scan_cursor = cursor.map(str::to_owned);
        let next_cursor = loop {
            let page = snapshot
                .product_records_page(kind, limit, scan_cursor.as_deref())
                .map_err(|error| RegistryError::Invalid(error.to_string()))?;
            let authorized_records = page
                .records
                .into_iter()
                .filter_map(|record| {
                    let owner = record.unique_values.get("owner_principal");
                    if identity.role != crate::security::Role::Admin
                        && owner.map(String::as_str) != Some(identity.principal.as_str())
                    {
                        return None;
                    }
                    Some((record.id, record.revision))
                })
                .collect::<Vec<_>>();
            // Product documents are immutable and independently addressed. A
            // bounded fan-out keeps a page from paying one object-store RTT per
            // row while preserving the source page order in `buffered` output.
            let catalog = (*catalog).clone();
            let page_records = stream::iter(authorized_records)
                .map(|(id, revision)| {
                    let catalog = catalog.clone();
                    let snapshot = snapshot.clone();
                    async move {
                        let bytes = catalog
                            .fetch_product_document_at(&snapshot, kind, &id)
                            .await
                            .map_err(|error| match error {
                                kaveon_storage::CommitErrorKind::Missing
                                | kaveon_storage::CommitErrorKind::Invalid => {
                                    RegistryError::Corrupt
                                }
                                _ => RegistryError::Unavailable,
                            })?
                            .ok_or(RegistryError::Corrupt)?;
                        let document =
                            serde_json::from_slice(&bytes).map_err(|_| RegistryError::Corrupt)?;
                        Ok(ProductReadResponse {
                            kind,
                            id,
                            revision,
                            generation: snapshot.generation,
                            snapshot_id: snapshot.snapshot_id.clone(),
                            document,
                        })
                    }
                })
                .buffered(MAX_PRODUCT_READ_CONCURRENCY)
                .collect::<Vec<Result<_, RegistryError>>>()
                .await;
            for product in page_records {
                records.push(product?);
            }
            let page_next_cursor = page.next_cursor;
            if records.len() >= limit || page_next_cursor.is_none() {
                break page_next_cursor;
            }
            scan_cursor = page_next_cursor;
        };
        Ok(ProductListResponse {
            generation: snapshot.generation,
            snapshot_id: snapshot.snapshot_id.clone(),
            records,
            next_cursor,
        })
    }

    async fn list_system_rows(
        &self,
        table: &str,
        limit: usize,
        cursor: Option<&str>,
    ) -> Result<SystemListResponse, RegistryError> {
        validate_system_table(table)?;
        if !(1..=1000).contains(&limit) {
            return Err(RegistryError::Invalid(
                "system row limit must be between 1 and 1000".into(),
            ));
        }
        let catalog = self.catalog.as_ref().ok_or(RegistryError::Disabled)?;
        let snapshot = catalog
            .read_current_shared()
            .await
            .map_err(|error| match error {
                kaveon_storage::CommitErrorKind::Missing
                | kaveon_storage::CommitErrorKind::Invalid => RegistryError::Corrupt,
                _ => RegistryError::Unavailable,
            })?;
        let rows = snapshot
            .typed_rows
            .get(table)
            .ok_or(RegistryError::ProductMissing)?;
        let start = cursor.unwrap_or("");
        let mut page = Vec::new();
        let mut next_cursor = None;
        for (id, row) in rows
            .iter()
            .filter(|(id, _)| id.as_str() > start)
            .take(limit + 1)
        {
            if page.len() == limit {
                next_cursor = Some(id.clone());
                break;
            }
            page.push(SystemRowResponse {
                table: table.to_owned(),
                id: id.clone(),
                revision: row.revision,
                generation: snapshot.generation,
                snapshot_id: snapshot.snapshot_id.clone(),
                columns: row.columns.clone(),
            });
        }
        Ok(SystemListResponse {
            generation: snapshot.generation,
            snapshot_id: snapshot.snapshot_id.clone(),
            rows: page,
            next_cursor,
        })
    }

    async fn read_system_row(
        &self,
        table: &str,
        id: &str,
    ) -> Result<SystemRowResponse, RegistryError> {
        validate_system_table(table)?;
        let catalog = self.catalog.as_ref().ok_or(RegistryError::Disabled)?;
        let snapshot = catalog
            .read_current_shared()
            .await
            .map_err(|error| match error {
                kaveon_storage::CommitErrorKind::Missing
                | kaveon_storage::CommitErrorKind::Invalid => RegistryError::Corrupt,
                _ => RegistryError::Unavailable,
            })?;
        let row = snapshot
            .typed_rows
            .get(table)
            .and_then(|rows| rows.get(id))
            .ok_or(RegistryError::ProductMissing)?;
        Ok(SystemRowResponse {
            table: table.to_owned(),
            id: id.to_owned(),
            revision: row.revision,
            generation: snapshot.generation,
            snapshot_id: snapshot.snapshot_id.clone(),
            columns: row.columns.clone(),
        })
    }

    async fn query_system_rows(
        &self,
        table: &str,
        request: &SystemQueryRequest,
    ) -> Result<SystemListResponse, RegistryError> {
        validate_system_table(table)?;
        if !(1..=1000).contains(&request.limit) {
            return Err(RegistryError::Invalid(
                "system query limit must be between 1 and 1000".into(),
            ));
        }
        if request.filters.len() > 32 {
            return Err(RegistryError::Invalid(
                "system query filter limit exceeded".into(),
            ));
        }
        for column in request.filters.keys() {
            validate_system_column(column)?;
        }
        if let Some(column) = request.order_by.as_deref() {
            validate_system_column(column)?;
        }
        let catalog = self.catalog.as_ref().ok_or(RegistryError::Disabled)?;
        let snapshot = catalog
            .read_current_shared()
            .await
            .map_err(|error| match error {
                kaveon_storage::CommitErrorKind::Missing
                | kaveon_storage::CommitErrorKind::Invalid => RegistryError::Corrupt,
                _ => RegistryError::Unavailable,
            })?;
        let rows = snapshot
            .typed_rows
            .get(table)
            .ok_or(RegistryError::ProductMissing)?;
        let mut matches = rows
            .iter()
            .filter(|(_, row)| {
                request
                    .filters
                    .iter()
                    .all(|(column, expected)| row.columns.get(column) == Some(expected))
            })
            .collect::<Vec<_>>();
        matches.sort_by(|(left_id, left), (right_id, right)| {
            let ordering = request
                .order_by
                .as_deref()
                .and_then(|column| left.columns.get(column).zip(right.columns.get(column)))
                .map(|(left, right)| typed_value_order(left, right))
                .unwrap_or_else(|| left_id.cmp(right_id));
            let ordering = if request.descending {
                ordering.reverse()
            } else {
                ordering
            };
            ordering.then_with(|| left_id.cmp(right_id))
        });
        let rows = matches
            .into_iter()
            .take(request.limit)
            .map(|(id, row)| SystemRowResponse {
                table: table.to_owned(),
                id: id.clone(),
                revision: row.revision,
                generation: snapshot.generation,
                snapshot_id: snapshot.snapshot_id.clone(),
                columns: row.columns.clone(),
            })
            .collect();
        Ok(SystemListResponse {
            generation: snapshot.generation,
            snapshot_id: snapshot.snapshot_id.clone(),
            rows,
            next_cursor: None,
        })
    }
}

fn validate_system_table(table: &str) -> Result<(), RegistryError> {
    if table.is_empty()
        || table.len() > 128
        || table.starts_with('.')
        || table.ends_with('.')
        || table.contains("..")
        || !table
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_' || byte == b'.')
    {
        return Err(RegistryError::Invalid(
            "system table name is invalid".into(),
        ));
    }
    Ok(())
}

fn validate_system_column(column: &str) -> Result<(), RegistryError> {
    if column.is_empty()
        || column.len() > 128
        || !column
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
    {
        return Err(RegistryError::Invalid(
            "system column name is invalid".into(),
        ));
    }
    Ok(())
}

fn typed_value_order(left: &TypedValue, right: &TypedValue) -> Ordering {
    let rank = |value: &TypedValue| match value {
        TypedValue::Null => 0,
        TypedValue::Boolean(_) => 1,
        TypedValue::Integer(_) => 2,
        TypedValue::String(_) => 3,
        TypedValue::Json(_) => 4,
    };
    match rank(left).cmp(&rank(right)) {
        Ordering::Equal => match (left, right) {
            (TypedValue::Null, TypedValue::Null) => Ordering::Equal,
            (TypedValue::Boolean(left), TypedValue::Boolean(right)) => left.cmp(right),
            (TypedValue::Integer(left), TypedValue::Integer(right)) => left.cmp(right),
            (TypedValue::String(left), TypedValue::String(right)) => left.cmp(right),
            (TypedValue::Json(left), TypedValue::Json(right)) => serde_json::to_vec(left)
                .unwrap_or_default()
                .cmp(&serde_json::to_vec(right).unwrap_or_default()),
            _ => Ordering::Equal,
        },
        ordering => ordering,
    }
}

/// The grants family is written only by the catalog access store, which
/// checks the actor's role and audits; a session — staged as a change or
/// as typed-row DML — cannot record a grant for its owner.
fn reject_grant_changes(change: &CatalogChange) -> Result<(), RegistryError> {
    if crate::catalog_access::touches_grants(change) {
        return Err(RegistryError::Invalid(format!(
            "the '{}' family is managed through the catalog access API",
            crate::catalog_access::GRANTS_TABLE
        )));
    }
    Ok(())
}

fn product_change(
    snapshot: &CatalogSnapshot,
    owner: &str,
    command: kaveon_sql::parser::ProductDmlCommand,
) -> Result<ProductStage, RegistryError> {
    use kaveon_sql::parser::ProductDmlCommand;
    match command {
        ProductDmlCommand::CreateIgnore {
            kind,
            id,
            document_json,
        } => product_change(
            snapshot,
            owner,
            ProductDmlCommand::Create {
                kind,
                id,
                document_json,
            },
        ),
        ProductDmlCommand::Create {
            kind,
            id,
            document_json,
        } => {
            if kind == "typed_row" {
                let (table, row) = typed_row_document(&id, &document_json, owner, 1)?;
                return Ok((CatalogChange::InsertTypedRow { table, row }, None));
            }
            let kind = record_kind(&kind)?;
            let (document, bytes, references, derived_values) =
                product_document(kind, &id, 1, &document_json)?;
            validate_favorite_owner(kind, owner, &derived_values)?;
            validate_product_binding(snapshot, kind, &id, None, &references, &derived_values)?;
            let mut unique_values = BTreeMap::from([("owner_principal".into(), owner.into())]);
            unique_values.extend(derived_values);
            Ok((
                CatalogChange::CreateProduct {
                    record: ProductRecordRef {
                        kind,
                        id,
                        revision: 1,
                        document,
                        unique_values,
                        references,
                    },
                },
                Some(bytes),
            ))
        }
        ProductDmlCommand::Upsert {
            kind,
            id,
            document_json,
        } => {
            if kind == "typed_row" {
                match find_typed_row(snapshot, &id, owner) {
                    Ok((table, current)) => {
                        let expected_revision = current.revision;
                        let next_revision = expected_revision.checked_add(1).ok_or_else(|| {
                            RegistryError::Invalid("typed row revision overflow".into())
                        })?;
                        let (incoming_table, _) =
                            typed_row_document(&id, &document_json, owner, next_revision)?;
                        if incoming_table != table {
                            return Err(RegistryError::Invalid(
                                "typed row upsert cannot change its table".into(),
                            ));
                        }
                        return product_change(
                            snapshot,
                            owner,
                            ProductDmlCommand::Update {
                                kind,
                                id,
                                expected_revision,
                                document_json,
                            },
                        );
                    }
                    Err(RegistryError::Invalid(message))
                        if message == "typed row does not exist" =>
                    {
                        return product_change(
                            snapshot,
                            owner,
                            ProductDmlCommand::Create {
                                kind,
                                id,
                                document_json,
                            },
                        );
                    }
                    Err(error) => return Err(error),
                }
            }
            let product_kind = record_kind(&kind)?;
            let current = snapshot
                .product_record(product_kind, &id)
                .map_err(|error| RegistryError::Invalid(error.to_string()))?;
            if let Some(current) = current {
                require_product_owner(current, owner)?;
                product_change(
                    snapshot,
                    owner,
                    ProductDmlCommand::Update {
                        kind,
                        id,
                        expected_revision: current.revision,
                        document_json,
                    },
                )
            } else {
                product_change(
                    snapshot,
                    owner,
                    ProductDmlCommand::Create {
                        kind,
                        id,
                        document_json,
                    },
                )
            }
        }
        ProductDmlCommand::MergeDeleteOrInsert {
            kind,
            id,
            document_json,
        } => {
            let product_kind = record_kind(&kind)?;
            let current = snapshot
                .product_record(product_kind, &id)
                .map_err(|error| RegistryError::Invalid(error.to_string()))?;
            if let Some(current) = current {
                require_product_owner(current, owner)?;
                product_change(
                    snapshot,
                    owner,
                    ProductDmlCommand::Delete {
                        kind,
                        id,
                        expected_revision: current.revision,
                    },
                )
            } else {
                product_change(
                    snapshot,
                    owner,
                    ProductDmlCommand::Create {
                        kind,
                        id,
                        document_json,
                    },
                )
            }
        }
        ProductDmlCommand::MergeDelete { .. } => Err(RegistryError::Invalid(
            "MERGE matched-delete must be resolved before staging".into(),
        )),
        ProductDmlCommand::MergeUpdate { .. } => Err(RegistryError::Invalid(
            "MERGE matched-update must be resolved before staging".into(),
        )),
        ProductDmlCommand::MergeUpdateOrCreateFiltered { .. } => Err(RegistryError::Invalid(
            "MERGE filtered update-or-insert must be resolved before staging".into(),
        )),
        ProductDmlCommand::MergeDeleteOrCreateFiltered { .. } => Err(RegistryError::Invalid(
            "MERGE filtered delete-or-insert must be resolved before staging".into(),
        )),
        ProductDmlCommand::MergeCreateIfAbsent { .. } => Err(RegistryError::Invalid(
            "MERGE unmatched-insert must be resolved before staging".into(),
        )),
        ProductDmlCommand::MergeDeleteNotMatched { .. } => Err(RegistryError::Invalid(
            "MERGE not-matched-by-source deletes must be resolved before staging".into(),
        )),
        ProductDmlCommand::MergeUpdateNotMatched { .. } => Err(RegistryError::Invalid(
            "MERGE not-matched-by-source updates must be resolved before staging".into(),
        )),
        ProductDmlCommand::Update {
            kind,
            id,
            expected_revision,
            document_json,
        } => {
            if kind == "typed_row" {
                let revision = expected_revision
                    .checked_add(1)
                    .ok_or_else(|| RegistryError::Invalid("typed row revision overflow".into()))?;
                let (table, row) = typed_row_document(&id, &document_json, owner, revision)?;
                let current = snapshot
                    .typed_rows
                    .get(&table)
                    .and_then(|rows| rows.get(&id))
                    .ok_or_else(|| RegistryError::Invalid("typed row does not exist".into()))?;
                require_typed_row_owner(current, owner)?;
                return Ok((
                    CatalogChange::UpdateTypedRow {
                        table,
                        expected_revision,
                        row,
                    },
                    None,
                ));
            }
            let kind = record_kind(&kind)?;
            let current = snapshot
                .product_record(kind, &id)
                .map_err(|error| RegistryError::Invalid(error.to_string()))?
                .ok_or_else(|| RegistryError::Invalid("product record does not exist".into()))?;
            require_product_owner(current, owner)?;
            let revision = expected_revision
                .checked_add(1)
                .ok_or_else(|| RegistryError::Invalid("product revision overflow".into()))?;
            let (document, bytes, references, derived_values) =
                product_document(kind, &id, revision, &document_json)?;
            validate_favorite_owner(kind, owner, &derived_values)?;
            validate_product_binding(
                snapshot,
                kind,
                &id,
                Some(current),
                &references,
                &derived_values,
            )?;
            let mut unique_values = BTreeMap::from([("owner_principal".into(), owner.into())]);
            unique_values.extend(derived_values);
            Ok((
                CatalogChange::UpdateProduct {
                    expected_revision,
                    record: ProductRecordRef {
                        document,
                        revision,
                        references,
                        unique_values,
                        ..current.clone()
                    },
                },
                Some(bytes),
            ))
        }
        ProductDmlCommand::Delete {
            kind,
            id,
            expected_revision,
        } => {
            if kind == "typed_row" {
                let (table, current) = find_typed_row(snapshot, &id, owner)?;
                if current.revision != expected_revision {
                    return Err(RegistryError::Invalid("stale typed row revision".into()));
                }
                return Ok((
                    CatalogChange::DeleteTypedRow {
                        table,
                        primary_key: id,
                        expected_revision,
                    },
                    None,
                ));
            }
            let kind = record_kind(&kind)?;
            let current = snapshot
                .product_record(kind, &id)
                .map_err(|error| RegistryError::Invalid(error.to_string()))?
                .ok_or_else(|| RegistryError::Invalid("product record does not exist".into()))?;
            require_product_owner(current, owner)?;
            Ok((
                CatalogChange::DeleteProduct {
                    kind,
                    id,
                    expected_revision,
                },
                None,
            ))
        }
        ProductDmlCommand::InsertSelect { .. } => Err(RegistryError::Invalid(
            "product INSERT SELECT must be expanded before staging".into(),
        )),
        ProductDmlCommand::UpdateFrom { .. } => Err(RegistryError::Invalid(
            "product UPDATE FROM must be expanded before staging".into(),
        )),
        ProductDmlCommand::DeleteFrom { .. } => Err(RegistryError::Invalid(
            "product DELETE USING must be expanded before staging".into(),
        )),
    }
}

#[derive(Debug, Serialize, Deserialize)]
struct TypedRowDocument {
    table: String,
    primary_key: String,
    revision: u64,
    columns: BTreeMap<String, TypedValue>,
    #[serde(default)]
    unique_keys: BTreeMap<String, String>,
    owner_principal: String,
}

fn typed_row_document(
    id: &str,
    document_json: &str,
    owner: &str,
    expected_revision: u64,
) -> Result<(String, TypedRow), RegistryError> {
    let document: TypedRowDocument = serde_json::from_str(document_json)
        .map_err(|_| RegistryError::Invalid("typed row document is invalid JSON".into()))?;
    if document.primary_key != id || document.revision != expected_revision {
        return Err(RegistryError::Invalid(
            "typed row identity or revision does not match the SQL mutation".into(),
        ));
    }
    if document.owner_principal != owner {
        return Err(RegistryError::OwnershipForbidden);
    }
    let mut columns = document.columns;
    columns.insert(
        "owner_principal".into(),
        TypedValue::String(document.owner_principal),
    );
    Ok((
        document.table,
        TypedRow {
            primary_key: document.primary_key,
            revision: document.revision,
            columns,
            unique_keys: document.unique_keys,
        },
    ))
}

fn require_typed_row_owner(row: &TypedRow, owner: &str) -> Result<(), RegistryError> {
    match row.columns.get("owner_principal") {
        Some(TypedValue::String(value)) if value == owner => Ok(()),
        _ => Err(RegistryError::OwnershipForbidden),
    }
}

fn find_typed_row<'a>(
    snapshot: &'a CatalogSnapshot,
    id: &str,
    owner: &str,
) -> Result<(String, &'a TypedRow), RegistryError> {
    let mut found = None;
    for (table, rows) in &snapshot.typed_rows {
        if let Some(row) = rows.get(id) {
            require_typed_row_owner(row, owner)?;
            if found.is_some() {
                return Err(RegistryError::Invalid(
                    "typed row primary key is ambiguous across tables".into(),
                ));
            }
            found = Some((table.clone(), row));
        }
    }
    found.ok_or_else(|| RegistryError::Invalid("typed row does not exist".into()))
}

fn validate_favorite_owner(
    kind: ProductRecordKind,
    owner: &str,
    values: &BTreeMap<String, String>,
) -> Result<(), RegistryError> {
    if (kind == ProductRecordKind::Favorite
        && !values["favorite_owner_target"].starts_with(&format!("{owner}:")))
        || (kind == ProductRecordKind::UserRecent
            && !values["recent_owner_item"].starts_with(&format!("{owner}:")))
        || (kind == ProductRecordKind::QueryHistory && values["query_owner"] != owner)
        || (kind == ProductRecordKind::Activity && values["activity_actor"] != owner)
        || (kind == ProductRecordKind::ChatSession && values["chat_owner"] != owner)
        || (kind == ProductRecordKind::ChatMessage && values["chat_owner"] != owner)
    {
        return Err(RegistryError::OwnershipForbidden);
    }
    Ok(())
}

fn require_product_owner(record: &ProductRecordRef, owner: &str) -> Result<(), RegistryError> {
    if record
        .unique_values
        .get("owner_principal")
        .map(String::as_str)
        == Some(owner)
    {
        Ok(())
    } else {
        Err(RegistryError::OwnershipForbidden)
    }
}

fn record_kind(kind: &str) -> Result<ProductRecordKind, RegistryError> {
    match kind {
        "dataset" => Ok(ProductRecordKind::Dataset),
        "chart" => Ok(ProductRecordKind::Chart),
        "dashboard" => Ok(ProductRecordKind::Dashboard),
        "saved_query" => Ok(ProductRecordKind::SavedQuery),
        "user_theme" => Ok(ProductRecordKind::UserTheme),
        "dlm_definition" => Ok(ProductRecordKind::DlmDefinition),
        "dlm_run" => Ok(ProductRecordKind::DlmRun),
        "favorite" => Ok(ProductRecordKind::Favorite),
        "source" => Ok(ProductRecordKind::Source),
        "user_recent" => Ok(ProductRecordKind::UserRecent),
        "query_history" => Ok(ProductRecordKind::QueryHistory),
        "activity" => Ok(ProductRecordKind::Activity),
        "chat_session" => Ok(ProductRecordKind::ChatSession),
        "chat_message" => Ok(ProductRecordKind::ChatMessage),
        _ => Err(RegistryError::Invalid(
            "unsupported product record kind".into(),
        )),
    }
}

fn product_document(
    kind: ProductRecordKind,
    id: &str,
    revision: u64,
    document_json: &str,
) -> Result<ProductDocument, RegistryError> {
    let value: serde_json::Value = serde_json::from_str(document_json)
        .map_err(|_| RegistryError::Invalid("document_json must be valid JSON".into()))?;
    if !value.is_object() {
        return Err(RegistryError::Invalid(
            "document_json must be a JSON object".into(),
        ));
    }
    let mut derived_values = BTreeMap::new();
    let references = if matches!(
        kind,
        ProductRecordKind::ChatSession | ProductRecordKind::ChatMessage
    ) {
        let object = value.as_object().expect("object checked above");
        let owner = object
            .get("user_email")
            .and_then(serde_json::Value::as_str)
            .filter(|v| !v.is_empty())
            .ok_or_else(|| RegistryError::Invalid("chat owner is invalid".into()))?;
        if object.get("id").and_then(serde_json::Value::as_str) != Some(id) {
            return Err(RegistryError::Invalid("chat record ID is invalid".into()));
        }
        derived_values.insert("chat_owner".into(), owner.into());
        if kind == ProductRecordKind::ChatSession {
            const ALLOWED: [&str; 5] = ["id", "user_email", "title", "created_at", "updated_at"];
            if object.keys().any(|k| !ALLOWED.contains(&k.as_str()))
                || object
                    .get("title")
                    .and_then(serde_json::Value::as_str)
                    .is_none()
            {
                return Err(RegistryError::Invalid(
                    "chat session document is invalid".into(),
                ));
            }
            BTreeSet::new()
        } else {
            const ALLOWED: [&str; 10] = [
                "id",
                "session_id",
                "user_email",
                "role",
                "content",
                "sql_query",
                "chart_type",
                "data",
                "route",
                "created_at",
            ];
            if object.keys().any(|k| !ALLOWED.contains(&k.as_str()))
                || !matches!(
                    object.get("role").and_then(serde_json::Value::as_str),
                    Some("user" | "assistant")
                )
                || object
                    .get("content")
                    .and_then(serde_json::Value::as_str)
                    .is_none()
            {
                return Err(RegistryError::Invalid(
                    "chat message document is invalid".into(),
                ));
            }
            let session = object
                .get("session_id")
                .and_then(serde_json::Value::as_str)
                .filter(|v| !v.is_empty())
                .ok_or_else(|| {
                    RegistryError::Invalid("chat session reference is invalid".into())
                })?;
            BTreeSet::from([ProductRecordReference {
                kind: ProductRecordKind::ChatSession,
                id: session.into(),
            }])
        }
    } else if kind == ProductRecordKind::Activity {
        let object = value.as_object().expect("object checked above");
        const ALLOWED: [&str; 8] = [
            "id",
            "action",
            "object_type",
            "object_id",
            "object_name",
            "timestamp",
            "user_email",
            "details",
        ];
        if object.keys().any(|key| !ALLOWED.contains(&key.as_str()))
            || object.get("id").and_then(serde_json::Value::as_str) != Some(id)
        {
            return Err(RegistryError::Invalid(
                "activity document schema is invalid".into(),
            ));
        }
        let actor = object
            .get("user_email")
            .and_then(serde_json::Value::as_str)
            .filter(|v| !v.is_empty())
            .ok_or_else(|| RegistryError::Invalid("activity actor is invalid".into()))?;
        for field in [
            "action",
            "object_type",
            "object_id",
            "object_name",
            "timestamp",
        ] {
            if object
                .get(field)
                .and_then(serde_json::Value::as_str)
                .is_none_or(|v| v.is_empty())
            {
                return Err(RegistryError::Invalid(
                    "activity document is invalid".into(),
                ));
            }
        }
        if object
            .get("details")
            .is_some_and(|v| !v.is_null() && !v.is_object())
        {
            return Err(RegistryError::Invalid(
                "activity details must be an object".into(),
            ));
        }
        derived_values.insert("activity_actor".into(), actor.into());
        BTreeSet::new()
    } else if kind == ProductRecordKind::QueryHistory {
        let object = value.as_object().expect("object checked above");
        const ALLOWED: [&str; 12] = [
            "id",
            "sql_text",
            "database_name",
            "executed_at",
            "execution_time",
            "row_count",
            "status",
            "error_message",
            "user_email",
            "trigger_source",
            "dataset_id",
            "tables_used",
        ];
        if object.keys().any(|key| !ALLOWED.contains(&key.as_str()))
            || object.get("id").and_then(serde_json::Value::as_str) != Some(id)
        {
            return Err(RegistryError::Invalid(
                "query history document schema is invalid".into(),
            ));
        }
        let owner = object
            .get("user_email")
            .and_then(serde_json::Value::as_str)
            .filter(|v| !v.is_empty())
            .ok_or_else(|| RegistryError::Invalid("query history owner is invalid".into()))?;
        if object
            .get("sql_text")
            .and_then(serde_json::Value::as_str)
            .is_none_or(|v| v.is_empty())
            || object
                .get("executed_at")
                .and_then(serde_json::Value::as_str)
                .is_none_or(|v| v.is_empty())
        {
            return Err(RegistryError::Invalid(
                "query history document is invalid".into(),
            ));
        }
        derived_values.insert("query_owner".into(), owner.into());
        match object
            .get("dataset_id")
            .and_then(serde_json::Value::as_str)
            .filter(|v| !v.is_empty())
        {
            Some(dataset) => BTreeSet::from([ProductRecordReference {
                kind: ProductRecordKind::Dataset,
                id: dataset.into(),
            }]),
            None => BTreeSet::new(),
        }
    } else if kind == ProductRecordKind::UserRecent {
        let object = value.as_object().expect("object checked above");
        const ALLOWED: [&str; 6] = [
            "user_email",
            "item_id",
            "label",
            "href",
            "type",
            "created_at",
        ];
        if object.keys().any(|key| !ALLOWED.contains(&key.as_str())) {
            return Err(RegistryError::Invalid(
                "user recent document schema is invalid".into(),
            ));
        }
        let owner = object
            .get("user_email")
            .and_then(serde_json::Value::as_str)
            .filter(|v| !v.is_empty())
            .ok_or_else(|| RegistryError::Invalid("user recent owner is invalid".into()))?;
        let item = object
            .get("item_id")
            .and_then(serde_json::Value::as_str)
            .filter(|v| !v.is_empty())
            .ok_or_else(|| RegistryError::Invalid("user recent item is invalid".into()))?;
        let target = match object.get("type").and_then(serde_json::Value::as_str) {
            Some("dataset") => ProductRecordKind::Dataset,
            Some("chart") => ProductRecordKind::Chart,
            Some("dashboard") => ProductRecordKind::Dashboard,
            _ => {
                return Err(RegistryError::Invalid(
                    "user recent type is unsupported".into(),
                ));
            }
        };
        // The portal stores recent item IDs with their product type prefix,
        // while product records use the canonical target ID. Strip exactly
        // the prefix for the declared type when it is present; retain
        // already-canonical IDs for backwards compatibility.
        let target_id = match target {
            ProductRecordKind::Dataset => item.strip_prefix("dataset-").unwrap_or(item),
            ProductRecordKind::Chart => item.strip_prefix("chart-").unwrap_or(item),
            ProductRecordKind::Dashboard => item.strip_prefix("dashboard-").unwrap_or(item),
            _ => unreachable!("user recent target is restricted above"),
        };
        if object
            .get("href")
            .and_then(serde_json::Value::as_str)
            .is_none_or(|v| v.is_empty())
            || object
                .get("created_at")
                .and_then(serde_json::Value::as_str)
                .is_none_or(|v| v.is_empty())
        {
            return Err(RegistryError::Invalid(
                "user recent document is invalid".into(),
            ));
        }
        derived_values.insert("recent_owner_item".into(), format!("{owner}:{item}"));
        BTreeSet::from([ProductRecordReference {
            kind: target,
            id: target_id.into(),
        }])
    } else if kind == ProductRecordKind::Source {
        let object = value.as_object().expect("object checked above");
        const ALLOWED: [&str; 21] = [
            "source_kind",
            "source_id",
            "name",
            "catalog_identity",
            "source_type",
            "database_name",
            "region",
            "description",
            "is_active",
            "lifecycle",
            "secret_ref",
            "adapter_type",
            "storage_config",
            "data_format",
            "credential_kind",
            "credential_ref",
            "adapter_config",
            "created_by",
            "modified_by",
            "created_at",
            "modified_at",
        ];
        if object.keys().any(|key| !ALLOWED.contains(&key.as_str()))
            || object.get("source_id").and_then(serde_json::Value::as_str) != Some(id)
            || !matches!(
                object
                    .get("source_kind")
                    .and_then(serde_json::Value::as_str),
                Some("catalog" | "data")
            )
        {
            return Err(RegistryError::Invalid(
                "source document schema is invalid".into(),
            ));
        }
        let secret_ref = object
            .get("secret_ref")
            .and_then(serde_json::Value::as_str)
            .filter(|value| !value.is_empty() && value.len() <= 512)
            .ok_or_else(|| RegistryError::Invalid("source secret_ref is invalid".into()))?;
        if secret_ref.chars().any(char::is_control) {
            return Err(RegistryError::Invalid(
                "source secret_ref is invalid".into(),
            ));
        }
        if object
            .get("source_kind")
            .and_then(serde_json::Value::as_str)
            == Some("catalog")
        {
            let extended =
                object.contains_key("storage_config") || object.contains_key("adapter_config");
            if extended
                && (!object
                    .get("storage_config")
                    .is_some_and(serde_json::Value::is_object)
                    || !object
                        .get("adapter_config")
                        .is_some_and(serde_json::Value::is_object))
            {
                return Err(RegistryError::Invalid(
                    "source configuration is invalid".into(),
                ));
            }
            if let Some(reference) = object
                .get("credential_ref")
                .and_then(serde_json::Value::as_str)
            {
                let identity = matches!(
                    object
                        .get("credential_kind")
                        .and_then(serde_json::Value::as_str),
                    Some("managed_identity" | "workload_identity")
                );
                if reference.len() > 512
                    || reference.chars().any(char::is_control)
                    || (!identity
                        && !(reference.starts_with("https://")
                            && reference.contains(".vault.azure.net/")))
                {
                    return Err(RegistryError::Invalid(
                        "source credential_ref is invalid".into(),
                    ));
                }
            }
        }
        BTreeSet::new()
    } else if kind == ProductRecordKind::Favorite {
        let object = value.as_object().expect("object checked above");
        let owner = object
            .get("user_email")
            .and_then(serde_json::Value::as_str)
            .filter(|value| !value.is_empty())
            .ok_or_else(|| RegistryError::Invalid("favorite owner is invalid".into()))?;
        let object_id = object
            .get("object_id")
            .and_then(serde_json::Value::as_str)
            .filter(|value| !value.is_empty())
            .ok_or_else(|| RegistryError::Invalid("favorite object_id is invalid".into()))?;
        let target_name = object
            .get("object_type")
            .and_then(serde_json::Value::as_str)
            .ok_or_else(|| RegistryError::Invalid("favorite object_type is unsupported".into()))?;
        let target = match target_name {
            "dataset" => ProductRecordKind::Dataset,
            "chart" => ProductRecordKind::Chart,
            "dashboard" => ProductRecordKind::Dashboard,
            "saved_query" => ProductRecordKind::SavedQuery,
            "source" => ProductRecordKind::Source,
            _ => {
                return Err(RegistryError::Invalid(
                    "favorite object_type is unsupported".into(),
                ));
            }
        };
        derived_values.insert(
            "favorite_owner_target".into(),
            format!("{owner}:{target_name}:{object_id}"),
        );
        BTreeSet::from([ProductRecordReference {
            kind: target,
            id: object_id.into(),
        }])
    } else if kind == ProductRecordKind::Dashboard && value.get("chart_revisions").is_some() {
        let revisions = value["chart_revisions"]
            .as_object()
            .ok_or_else(|| RegistryError::Invalid("dashboard chart_revisions is invalid".into()))?;
        if revisions
            .values()
            .any(|revision| revision.as_u64().is_none_or(|value| value == 0))
        {
            return Err(RegistryError::Invalid(
                "dashboard chart revision is invalid".into(),
            ));
        }
        derived_values.insert(
            "dashboard_chart_revisions".into(),
            serde_json::to_string(revisions).map_err(|_| {
                RegistryError::Invalid("dashboard chart revisions cannot be encoded".into())
            })?,
        );
        revisions
            .keys()
            .map(|id| ProductRecordReference {
                kind: ProductRecordKind::Chart,
                id: id.clone(),
            })
            .collect()
    } else if kind == ProductRecordKind::Chart {
        let object = value.as_object().expect("object checked above");
        let dataset_id = object
            .get("dataset_id")
            .and_then(serde_json::Value::as_str)
            .filter(|value| !value.is_empty())
            .ok_or_else(|| RegistryError::Invalid("chart dataset_id is invalid".into()))?;
        let dataset_revision = object
            .get("dataset_revision")
            .and_then(serde_json::Value::as_u64)
            .filter(|value| *value > 0)
            .ok_or_else(|| RegistryError::Invalid("chart dataset_revision is invalid".into()))?;
        derived_values.insert(
            "chart_dataset_revision".into(),
            format!("{dataset_id}:{dataset_revision}"),
        );
        BTreeSet::from([ProductRecordReference {
            kind: ProductRecordKind::Dataset,
            id: dataset_id.into(),
        }])
    } else if kind == ProductRecordKind::DlmDefinition {
        let object = value.as_object().expect("object checked above");
        if object.len() != 2
            || !object.contains_key("dataset_id")
            || !object.contains_key("dataset_revision")
        {
            return Err(RegistryError::Invalid(
                "DLM definition requires only dataset_id and dataset_revision".into(),
            ));
        }
        let dataset_id = object["dataset_id"]
            .as_str()
            .filter(|value| !value.is_empty())
            .ok_or_else(|| RegistryError::Invalid("DLM definition dataset_id is invalid".into()))?;
        if dataset_id != id {
            return Err(RegistryError::Invalid(
                "DLM definition ID must equal dataset_id".into(),
            ));
        }
        object["dataset_revision"]
            .as_u64()
            .filter(|value| *value > 0)
            .ok_or_else(|| {
                RegistryError::Invalid("DLM definition dataset_revision is invalid".into())
            })?;
        BTreeSet::from([ProductRecordReference {
            kind: ProductRecordKind::Dataset,
            id: dataset_id.into(),
        }])
    } else if kind == ProductRecordKind::DlmRun {
        let object = value.as_object().expect("object checked above");
        if object.len() != 4
            || !object.contains_key("definition_id")
            || !object.contains_key("definition_revision")
            || !object.contains_key("status")
            || !object.contains_key("artifact")
        {
            return Err(RegistryError::Invalid(
                "DLM run requires definition_id, definition_revision, status, and artifact".into(),
            ));
        }
        let definition_id = object["definition_id"]
            .as_str()
            .filter(|value| !value.is_empty())
            .ok_or_else(|| RegistryError::Invalid("DLM run definition_id is invalid".into()))?;
        let definition_revision = object["definition_revision"]
            .as_u64()
            .filter(|value| *value > 0)
            .ok_or_else(|| {
                RegistryError::Invalid("DLM run definition_revision is invalid".into())
            })?;
        let status = object["status"]
            .as_str()
            .filter(|value| matches!(*value, "building" | "ready" | "failed"))
            .ok_or_else(|| RegistryError::Invalid("DLM run status is invalid".into()))?;
        match (status, &object["artifact"]) {
            ("ready", serde_json::Value::Object(artifact))
                if artifact.len() == 2
                    && artifact["path"].as_str().is_some_and(valid_artifact_path)
                    && artifact["sha256"]
                        .as_str()
                        .is_some_and(valid_artifact_sha256) => {}
            ("building" | "failed", serde_json::Value::Null) => {}
            _ => {
                return Err(RegistryError::Invalid(
                    "DLM run artifact must be immutable for ready and null otherwise".into(),
                ));
            }
        }
        derived_values.insert("dlm_run_state".into(), format!("{id}:{status}"));
        derived_values.insert(
            "dlm_definition_revision".into(),
            format!("{definition_id}:{definition_revision}"),
        );
        BTreeSet::from([ProductRecordReference {
            kind: ProductRecordKind::DlmDefinition,
            id: definition_id.into(),
        }])
    } else {
        BTreeSet::new()
    };
    let bytes = serde_json::to_vec(&value)
        .map_err(|_| RegistryError::Invalid("document_json cannot be encoded".into()))?;
    let sha256 = format!("{:x}", Sha256::digest(&bytes));
    let kind_name = match kind {
        ProductRecordKind::Dataset => "dataset",
        ProductRecordKind::Chart => "chart",
        ProductRecordKind::Dashboard => "dashboard",
        ProductRecordKind::SavedQuery => "saved_query",
        ProductRecordKind::UserTheme => "user_theme",
        ProductRecordKind::DlmDefinition => "dlm_definition",
        ProductRecordKind::DlmRun => "dlm_run",
        ProductRecordKind::Favorite => "favorite",
        ProductRecordKind::Source => "source",
        ProductRecordKind::UserRecent => "user_recent",
        ProductRecordKind::QueryHistory => "query_history",
        ProductRecordKind::Activity => "activity",
        ProductRecordKind::ChatSession => "chat_session",
        ProductRecordKind::ChatMessage => "chat_message",
    };
    let path = format!("products/{kind_name}/{id}/{revision}-{sha256}.json");
    Ok((
        ImmutableFileRef {
            path: path.clone(),
            sha256,
        },
        (path, bytes),
        references,
        derived_values,
    ))
}

fn valid_artifact_path(path: &str) -> bool {
    !path.is_empty()
        && path.len() <= 1_024
        && !path.starts_with('/')
        && !path.contains(['\\', ':'])
        && path
            .split('/')
            .all(|part| !part.is_empty() && part != "." && part != "..")
}

fn valid_artifact_sha256(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn validate_product_binding(
    snapshot: &CatalogSnapshot,
    kind: ProductRecordKind,
    id: &str,
    current: Option<&ProductRecordRef>,
    references: &BTreeSet<ProductRecordReference>,
    values: &BTreeMap<String, String>,
) -> Result<(), RegistryError> {
    if kind == ProductRecordKind::Dashboard && values.contains_key("dashboard_chart_revisions") {
        let revisions: BTreeMap<String, u64> =
            serde_json::from_str(&values["dashboard_chart_revisions"]).map_err(|_| {
                RegistryError::Invalid("dashboard chart revisions are invalid".into())
            })?;
        for reference in references {
            let chart = snapshot
                .product_record(ProductRecordKind::Chart, &reference.id)
                .map_err(|error| RegistryError::Invalid(error.to_string()))?
                .ok_or_else(|| RegistryError::Invalid("dashboard chart does not exist".into()))?;
            if revisions.get(&reference.id) != Some(&chart.revision) {
                return Err(RegistryError::Invalid(
                    "dashboard chart revision is stale".into(),
                ));
            }
        }
        return Ok(());
    }
    if kind == ProductRecordKind::Chart {
        let reference = references
            .iter()
            .next()
            .ok_or_else(|| RegistryError::Invalid("chart dataset reference is missing".into()))?;
        let dataset = snapshot
            .product_record(ProductRecordKind::Dataset, &reference.id)
            .map_err(|error| RegistryError::Invalid(error.to_string()))?
            .ok_or_else(|| RegistryError::Invalid("chart dataset does not exist".into()))?;
        let expected = values["chart_dataset_revision"]
            .rsplit(':')
            .next()
            .and_then(|value| value.parse::<u64>().ok());
        return if expected == Some(dataset.revision) {
            Ok(())
        } else {
            Err(RegistryError::Invalid(
                "chart dataset revision is stale".into(),
            ))
        };
    }
    if kind != ProductRecordKind::DlmRun {
        return Ok(());
    }
    let reference = references
        .iter()
        .next()
        .ok_or_else(|| RegistryError::Invalid("DLM run definition reference is missing".into()))?;
    let definition = snapshot
        .product_record(ProductRecordKind::DlmDefinition, &reference.id)
        .map_err(|error| RegistryError::Invalid(error.to_string()))?
        .ok_or_else(|| RegistryError::Invalid("DLM run definition does not exist".into()))?;
    let expected = values["dlm_definition_revision"]
        .rsplit(':')
        .next()
        .and_then(|value| value.parse::<u64>().ok());
    if expected != Some(definition.revision) {
        return Err(RegistryError::Invalid(
            "DLM run definition revision is stale".into(),
        ));
    }
    let next = values["dlm_run_state"]
        .strip_prefix(&format!("{id}:"))
        .unwrap_or("");
    match current {
        None if next == "building" => Ok(()),
        Some(record)
            if record.unique_values["dlm_run_state"].ends_with(":building")
                && matches!(next, "ready" | "failed") =>
        {
            Ok(())
        }
        _ => Err(RegistryError::Invalid(
            "DLM run status transition is invalid".into(),
        )),
    }
}

fn expire(sessions: &mut HashMap<String, Session>) {
    let now = Instant::now();
    sessions.retain(|_, session| session.expires_at > now);
}

fn owned_session<'a>(
    sessions: &'a mut HashMap<String, Session>,
    owner: &str,
    id: &str,
) -> Result<&'a mut Session, RegistryError> {
    match sessions.get_mut(id) {
        None => Err(RegistryError::Missing),
        Some(session) if session.owner != owner => Err(RegistryError::SessionForbidden),
        Some(session) => Ok(session),
    }
}

#[derive(Debug, Serialize)]
struct BeginResponse {
    transaction_id: String,
    operation_id: String,
    base: CatalogSnapshot,
}

#[derive(Deserialize)]
struct StageRequest {
    change: CatalogChange,
    #[serde(default)]
    document: Option<DocumentInput>,
}

#[derive(Deserialize)]
struct DocumentInput {
    path: String,
    bytes: Vec<u8>,
}

#[derive(Deserialize)]
struct RecoveryQuery {
    request_digest: String,
    #[serde(default)]
    max_hops: Option<usize>,
}

#[derive(Deserialize)]
struct SqlTransactionRequest {
    sql: String,
    #[serde(default)]
    transaction_id: Option<String>,
    /// Positional values for `$1`, `$2`, … or `?` placeholders. Values are
    /// converted to SQL literals by the bounded transaction binder; they are
    /// never concatenated without escaping.
    #[serde(default)]
    parameters: Vec<serde_json::Value>,
    /// Named values for `:name` placeholders.
    #[serde(default)]
    named_parameters: HashMap<String, serde_json::Value>,
}

#[derive(Deserialize)]
struct SqlBatchRequest {
    statements: Vec<String>,
}

#[derive(Serialize)]
struct ProductReadResponse {
    kind: ProductRecordKind,
    id: String,
    revision: u64,
    generation: u64,
    snapshot_id: String,
    document: serde_json::Value,
}

#[derive(Serialize)]
struct ProductListResponse {
    generation: u64,
    snapshot_id: String,
    records: Vec<ProductReadResponse>,
    next_cursor: Option<String>,
}

#[derive(Serialize)]
struct SystemRowResponse {
    table: String,
    id: String,
    revision: u64,
    generation: u64,
    snapshot_id: String,
    columns: BTreeMap<String, TypedValue>,
}

#[derive(Serialize)]
struct SystemListResponse {
    generation: u64,
    snapshot_id: String,
    rows: Vec<SystemRowResponse>,
    next_cursor: Option<String>,
}

#[derive(Debug, Deserialize)]
struct SystemQueryRequest {
    #[serde(default)]
    filters: BTreeMap<String, TypedValue>,
    #[serde(default)]
    order_by: Option<String>,
    #[serde(default)]
    descending: bool,
    #[serde(default = "default_system_query_limit")]
    limit: usize,
}

fn default_system_query_limit() -> usize {
    100
}

#[derive(Deserialize)]
struct ProductListQuery {
    #[serde(default = "default_product_list_limit")]
    limit: usize,
    #[serde(default)]
    cursor: Option<String>,
}

fn default_product_list_limit() -> usize {
    100
}

async fn list_products(
    State(state): State<Arc<AppState>>,
    Extension(identity): Extension<Identity>,
    Path(kind): Path<String>,
    Query(query): Query<ProductListQuery>,
) -> Response {
    let kind = match record_kind(&kind) {
        Ok(kind) => kind,
        Err(error) => return error_response(error),
    };
    match state
        .product_transactions
        .list_products(&identity, kind, query.limit, query.cursor.as_deref())
        .await
    {
        Ok(page) => Json(page).into_response(),
        Err(error) => error_response(error),
    }
}

async fn read_product(
    State(state): State<Arc<AppState>>,
    Extension(identity): Extension<Identity>,
    Path((kind, id)): Path<(String, String)>,
) -> Response {
    let kind = match record_kind(&kind) {
        Ok(kind) => kind,
        Err(error) => return error_response(error),
    };
    match state
        .product_transactions
        .read_product(&identity, kind, &id)
        .await
    {
        Ok(record) => Json(record).into_response(),
        Err(error) => error_response(error),
    }
}

async fn list_system_rows(
    State(state): State<Arc<AppState>>,
    Extension(identity): Extension<Identity>,
    Path(table): Path<String>,
    Query(query): Query<ProductListQuery>,
) -> Response {
    if identity.role != crate::security::Role::Admin {
        return error_response(RegistryError::OwnershipForbidden);
    }
    match state
        .product_transactions
        .list_system_rows(&table, query.limit, query.cursor.as_deref())
        .await
    {
        Ok(page) => Json(page).into_response(),
        Err(error) => error_response(error),
    }
}

async fn read_system_row(
    State(state): State<Arc<AppState>>,
    Extension(identity): Extension<Identity>,
    Path((table, id)): Path<(String, String)>,
) -> Response {
    if identity.role != crate::security::Role::Admin {
        return error_response(RegistryError::OwnershipForbidden);
    }
    match state
        .product_transactions
        .read_system_row(&table, &id)
        .await
    {
        Ok(row) => Json(row).into_response(),
        Err(error) => error_response(error),
    }
}

async fn query_system_rows(
    State(state): State<Arc<AppState>>,
    Extension(identity): Extension<Identity>,
    Path(table): Path<String>,
    Json(request): Json<SystemQueryRequest>,
) -> Response {
    if identity.role != crate::security::Role::Admin {
        return error_response(RegistryError::OwnershipForbidden);
    }
    match state
        .product_transactions
        .query_system_rows(&table, &request)
        .await
    {
        Ok(page) => Json(page).into_response(),
        Err(error) => error_response(error),
    }
}

async fn recovery(
    State(state): State<Arc<AppState>>,
    Extension(_identity): Extension<Identity>,
    Path(transaction_id): Path<String>,
    Query(query): Query<RecoveryQuery>,
) -> Response {
    let operation_id = match Uuid::parse_str(&transaction_id) {
        Ok(_) => operation_id_for_transaction(&transaction_id),
        Err(_) => {
            return (
                StatusCode::BAD_REQUEST,
                Json(serde_json::json!({
                    "error": "transaction_id must be a UUID",
                    "code": "TRANSACTION_ID_INVALID"
                })),
            )
                .into_response();
        }
    };
    if query.request_digest.len() != 64
        || !query
            .request_digest
            .chars()
            .all(|character| character.is_ascii_hexdigit())
    {
        return (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({
                "error": "request_digest must be a 64-character hexadecimal digest",
                "code": "RECOVERY_DIGEST_INVALID"
            })),
        )
            .into_response();
    }
    let max_hops = query.max_hops.unwrap_or(64).clamp(1, 256);
    let Some(catalog) = state.product_transactions.catalog() else {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(serde_json::json!({
                "error": "product transactions are not configured",
                "code": "TRANSACTION_RECOVERY_UNAVAILABLE"
            })),
        )
            .into_response();
    };
    match catalog
        .resolve_operation(&operation_id, &query.request_digest, max_hops)
        .await
    {
        Ok(resolution) => recovery_resolution_response(&operation_id, resolution),
        Err(_) => (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(serde_json::json!({
                "status": "unavailable",
                "operation_id": operation_id,
                "code": "TRANSACTION_RECOVERY_UNAVAILABLE",
                "recovery_required": true
            })),
        )
            .into_response(),
    }
}

async fn metrics(
    State(state): State<Arc<AppState>>,
    Extension(_identity): Extension<Identity>,
) -> Response {
    match state.product_transactions.metrics() {
        Ok(snapshot) => Json(snapshot).into_response(),
        Err(error) => error_response(error),
    }
}

fn operation_id_for_transaction(transaction_id: &str) -> String {
    format!("operation-{}", transaction_id.replace('-', ""))
}

fn recovery_resolution_response(operation_id: &str, resolution: OperationResolution) -> Response {
    match resolution {
        OperationResolution::Committed(snapshot) => Json(serde_json::json!({
            "status": "committed",
            "operation_id": operation_id,
            "snapshot": snapshot
        }))
        .into_response(),
        OperationResolution::Conflict => Json(serde_json::json!({
            "status": "conflict",
            "operation_id": operation_id
        }))
        .into_response(),
        OperationResolution::NotCommitted => Json(serde_json::json!({
            "status": "not_committed",
            "operation_id": operation_id
        }))
        .into_response(),
        OperationResolution::Unresolved => (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(serde_json::json!({
                "status": "unresolved",
                "operation_id": operation_id,
                "code": "TRANSACTION_RECOVERY_UNRESOLVED",
                "recovery_required": true
            })),
        )
            .into_response(),
    }
}

async fn execute_sql(
    State(state): State<Arc<AppState>>,
    Extension(identity): Extension<Identity>,
    Json(request): Json<SqlTransactionRequest>,
) -> Response {
    let commits = matches!(
        kaveon_sql::parser::parse_native_transactional(&request.sql),
        Ok(kaveon_sql::parser::NativeTransactionalStatement::Commit)
    );
    let response =
        execute_sql_request(&state.product_transactions, &identity.principal, request).await;
    if commits && response.status().is_success() {
        // Committed product data may be what a cached result read.
        state.result_cache.clear();
    }
    response
}

/// Stage a bounded group of product SQL mutations in one authenticated
/// request. Each statement retains the normal parser and transaction checks;
/// this endpoint only removes per-row HTTP overhead for migration batches.
async fn stage_sql_batch(
    State(state): State<Arc<AppState>>,
    Extension(identity): Extension<Identity>,
    Path(transaction_id): Path<String>,
    Json(request): Json<SqlBatchRequest>,
) -> Response {
    use kaveon_sql::parser::{
        NativeTransactionalStatement, adapt_product_dml_batch, parse_native_transactional,
    };
    if request.statements.is_empty() || request.statements.len() > 1000 {
        return error_response(RegistryError::Invalid(
            "SQL batch requires between 1 and 1000 statements".into(),
        ));
    }
    let mut commands = Vec::with_capacity(request.statements.len());
    for sql in request.statements {
        let parsed = match parse_native_transactional(&sql) {
            Ok(NativeTransactionalStatement::Dml(dml)) => dml,
            Ok(_) => {
                return error_response(RegistryError::Invalid(
                    "SQL batch accepts DML statements only".into(),
                ));
            }
            Err(error) => return error_response(RegistryError::Invalid(error.to_string())),
        };
        let mut adapted = match adapt_product_dml_batch(&parsed) {
            Ok(command) => command,
            Err(error) => return error_response(RegistryError::Invalid(error.to_string())),
        };
        commands.append(&mut adapted);
    }
    match state
        .product_transactions
        .stage_product_commands(&identity.principal, &transaction_id, commands, false)
        .await
    {
        Ok((snapshot, _, _)) => Json(snapshot).into_response(),
        Err(error) => error_response(error),
    }
}

fn transaction_parameter_literal(
    value: &serde_json::Value,
    depth: usize,
) -> Result<String, String> {
    if depth > 32 {
        return Err("parameter array nesting exceeds the limit of 32".into());
    }
    match value {
        serde_json::Value::Null => Ok("NULL".into()),
        serde_json::Value::Bool(value) => Ok(if *value { "TRUE" } else { "FALSE" }.into()),
        serde_json::Value::Number(value) => Ok(value.to_string()),
        serde_json::Value::String(value) => Ok(format!("'{}'", value.replace('\'', "''"))),
        serde_json::Value::Array(values) => {
            if values.len() > 10_000 {
                return Err("parameter array exceeds the limit of 10000 elements".into());
            }
            values
                .iter()
                .map(|value| transaction_parameter_literal(value, depth + 1))
                .collect::<Result<Vec<_>, _>>()
                .map(|values| format!("ARRAY[{}]", values.join(", ")))
        }
        serde_json::Value::Object(_) => Err(
            "object parameters are not supported by the product transaction API; send JSON as a string"
                .into(),
        ),
    }
}

fn transaction_identifier_start(character: char) -> bool {
    character == '_' || character.is_ascii_alphabetic()
}

fn transaction_identifier_continue(character: char) -> bool {
    character == '_' || character.is_ascii_alphanumeric()
}

/// Bind transaction placeholders without treating text inside SQL literals or
/// comments as placeholders. Product DML is intentionally small, but the
/// scanner still handles comments and quoted identifiers so a document JSON
/// value containing `$1` or `:name` remains data.
fn bind_transaction_parameters(
    sql: &str,
    positional: &[serde_json::Value],
    named: &HashMap<String, serde_json::Value>,
) -> Result<String, String> {
    let input: Vec<char> = sql.chars().collect();
    let mut output = String::with_capacity(sql.len());
    let mut index = 0;
    let mut question_position = 0usize;
    let mut highest_dollar = 0usize;
    let mut used_named = std::collections::BTreeSet::new();
    let mut saw_question = false;
    let mut saw_dollar = false;

    while index < input.len() {
        let character = input[index];
        if character == '\'' || character == '"' {
            let quote = character;
            output.push(quote);
            index += 1;
            while index < input.len() {
                let current = input[index];
                output.push(current);
                index += 1;
                if current == quote {
                    if index < input.len() && input[index] == quote {
                        output.push(input[index]);
                        index += 1;
                    } else {
                        break;
                    }
                }
            }
            continue;
        }
        if character == '-' && input.get(index + 1) == Some(&'-') {
            while index < input.len() {
                let current = input[index];
                output.push(current);
                index += 1;
                if current == '\n' {
                    break;
                }
            }
            continue;
        }
        if character == '/' && input.get(index + 1) == Some(&'*') {
            output.push('/');
            output.push('*');
            index += 2;
            while index < input.len() {
                let current = input[index];
                output.push(current);
                index += 1;
                if current == '*' && input.get(index) == Some(&'/') {
                    output.push('/');
                    index += 1;
                    break;
                }
            }
            continue;
        }
        if character == '?' {
            if saw_dollar {
                return Err("cannot mix '?' and '$n' transaction parameters".into());
            }
            saw_question = true;
            question_position += 1;
            let value = positional.get(question_position - 1).ok_or_else(|| {
                format!(
                    "parameter ?{} is not bound (received {} values)",
                    question_position,
                    positional.len()
                )
            })?;
            output.push_str(&transaction_parameter_literal(value, 0)?);
            index += 1;
            continue;
        }
        if character == '$' {
            let mut end = index + 1;
            while end < input.len() && input[end].is_ascii_digit() {
                end += 1;
            }
            if end > index + 1 {
                if saw_question {
                    return Err("cannot mix '?' and '$n' transaction parameters".into());
                }
                saw_dollar = true;
                let name: String = input[index + 1..end].iter().collect();
                let position = name
                    .parse::<usize>()
                    .map_err(|_| format!("invalid transaction parameter ${name}"))?;
                if position == 0 {
                    return Err("transaction parameter positions start at $1".into());
                }
                highest_dollar = highest_dollar.max(position);
                let value = positional.get(position - 1).ok_or_else(|| {
                    format!(
                        "parameter ${position} is not bound (received {} values)",
                        positional.len()
                    )
                })?;
                output.push_str(&transaction_parameter_literal(value, 0)?);
                index = end;
                continue;
            }
        }
        if character == ':'
            && input.get(index + 1) != Some(&':')
            && input
                .get(index + 1)
                .is_some_and(|next| transaction_identifier_start(*next))
        {
            let mut end = index + 2;
            while end < input.len() && transaction_identifier_continue(input[end]) {
                end += 1;
            }
            let name: String = input[index + 1..end].iter().collect();
            let value = named
                .get(&name)
                .or_else(|| named.get(&name.to_ascii_lowercase()))
                .ok_or_else(|| format!("named parameter :{name} is not bound"))?;
            used_named.insert(name.to_ascii_lowercase());
            output.push_str(&transaction_parameter_literal(value, 0)?);
            index = end;
            continue;
        }
        output.push(character);
        index += 1;
    }

    if saw_question && positional.len() > question_position {
        return Err(format!(
            "{} positional values were supplied but only {} '?' placeholders are used",
            positional.len(),
            question_position
        ));
    }
    if saw_dollar && positional.len() > highest_dollar {
        return Err(format!(
            "{} positional values were supplied but the highest placeholder is ${highest_dollar}",
            positional.len()
        ));
    }
    if !saw_question && !saw_dollar && !positional.is_empty() {
        return Err("positional transaction values were supplied but the SQL has no positional placeholders".into());
    }
    if let Some(unused) = named
        .keys()
        .map(|name| name.to_ascii_lowercase())
        .find(|name| !used_named.contains(name))
    {
        return Err(format!(
            "named parameter :{unused} was supplied but is not used by the SQL"
        ));
    }
    Ok(output)
}

async fn execute_sql_request(
    registry: &TransactionRegistry,
    owner: &str,
    request: SqlTransactionRequest,
) -> Response {
    use kaveon_sql::parser::{
        NativeTransactionalStatement, adapt_product_dml_batch, parse_native_transactional,
    };
    let bound_sql = match bind_transaction_parameters(
        &request.sql,
        &request.parameters,
        &request.named_parameters,
    ) {
        Ok(sql) => sql,
        Err(error) => return error_response(RegistryError::Invalid(error)),
    };
    let parsed = match parse_native_transactional(&bound_sql) {
        Ok(parsed) => parsed,
        Err(error) => return error_response(RegistryError::Invalid(error.to_string())),
    };
    match parsed {
        NativeTransactionalStatement::Begin => {
            if request.transaction_id.is_some() {
                return error_response(RegistryError::Invalid(
                    "BEGIN cannot include transaction_id".into(),
                ));
            }
            match registry.begin(owner).await {
                Ok(response) => (StatusCode::CREATED, Json(response)).into_response(),
                Err(error) => error_response(error),
            }
        }
        NativeTransactionalStatement::Commit | NativeTransactionalStatement::Rollback
            if request.transaction_id.is_none() =>
        {
            error_response(RegistryError::Invalid("transaction_id is required".into()))
        }
        NativeTransactionalStatement::Commit => {
            let id = request.transaction_id.expect("checked");
            commit_transaction(registry, owner, &id).await
        }
        NativeTransactionalStatement::Rollback => {
            let id = request.transaction_id.expect("checked");
            rollback_transaction(registry, owner, &id).await
        }
        NativeTransactionalStatement::Savepoint(name) => {
            let Some(id) = request.transaction_id else {
                return error_response(RegistryError::Invalid("transaction_id is required".into()));
            };
            match registry.savepoint(owner, &id, &name).await {
                Ok(()) => StatusCode::NO_CONTENT.into_response(),
                Err(error) => error_response(error),
            }
        }
        NativeTransactionalStatement::RollbackToSavepoint(name) => {
            let Some(id) = request.transaction_id else {
                return error_response(RegistryError::Invalid("transaction_id is required".into()));
            };
            match registry.rollback_to_savepoint(owner, &id, &name).await {
                Ok(snapshot) => Json(snapshot).into_response(),
                Err(error) => error_response(error),
            }
        }
        NativeTransactionalStatement::ReleaseSavepoint(name) => {
            let Some(id) = request.transaction_id else {
                return error_response(RegistryError::Invalid("transaction_id is required".into()));
            };
            match registry.release_savepoint(owner, &id, &name).await {
                Ok(()) => StatusCode::NO_CONTENT.into_response(),
                Err(error) => error_response(error),
            }
        }
        NativeTransactionalStatement::Dml(dml) => {
            let Some(id) = request.transaction_id else {
                return error_response(RegistryError::Invalid("transaction_id is required".into()));
            };
            let returning = dml.returning.clone();
            let output_pseudo = dml.output_pseudo.clone();
            let commands = match adapt_product_dml_batch(&dml) {
                Ok(command) => command,
                Err(error) => return error_response(RegistryError::Invalid(error.to_string())),
            };
            if dml.kind == kaveon_sql::parser::NativeDmlKind::Merge
                && !returning.is_empty()
                && match output_pseudo.as_deref() {
                    Some("inserted") => commands.iter().any(|command| {
                        !matches!(
                            command,
                            kaveon_sql::parser::ProductDmlCommand::Upsert { .. }
                                | kaveon_sql::parser::ProductDmlCommand::MergeUpdate { .. }
                                | kaveon_sql::parser::ProductDmlCommand::MergeUpdateOrCreateFiltered { .. }
                                | kaveon_sql::parser::ProductDmlCommand::MergeDeleteOrCreateFiltered { .. }
                                | kaveon_sql::parser::ProductDmlCommand::MergeCreateIfAbsent { .. }
                                | kaveon_sql::parser::ProductDmlCommand::MergeDeleteNotMatched { .. }
                                | kaveon_sql::parser::ProductDmlCommand::MergeDeleteOrInsert { .. }
                                | kaveon_sql::parser::ProductDmlCommand::MergeUpdateNotMatched { .. }
                        )
                    }),
                    Some("deleted") => commands.iter().any(|command| {
                        !matches!(
                            command,
                            kaveon_sql::parser::ProductDmlCommand::Create { .. }
                                | kaveon_sql::parser::ProductDmlCommand::CreateIgnore { .. }
                                | kaveon_sql::parser::ProductDmlCommand::Upsert { .. }
                                | kaveon_sql::parser::ProductDmlCommand::MergeDeleteOrInsert { .. }
                                | kaveon_sql::parser::ProductDmlCommand::MergeDeleteOrCreateFiltered { .. }
                                | kaveon_sql::parser::ProductDmlCommand::MergeDelete { .. }
                                | kaveon_sql::parser::ProductDmlCommand::MergeUpdate { .. }
                                | kaveon_sql::parser::ProductDmlCommand::MergeDeleteNotMatched { .. }
                                | kaveon_sql::parser::ProductDmlCommand::MergeUpdateNotMatched { .. }
                        )
                    }),
                    Some("action") => false,
                    _ => true,
                }
            {
                return error_response(RegistryError::Invalid(
                    "MERGE OUTPUT is supported only for bounded inserted/deleted action batches; unsupported action shapes are rejected".into(),
                ));
            }
            let ignored_conflicts = if dml.on_conflict_do_nothing && !returning.is_empty() {
                match registry
                    .conflict_ignore_existing(owner, &id, &commands)
                    .await
                {
                    Ok(existing) => existing,
                    Err(error) => return error_response(error),
                }
            } else {
                Vec::new()
            };
            let source_copy_documents = match commands.as_slice() {
                [
                    kaveon_sql::parser::ProductDmlCommand::InsertSelect {
                        kind,
                        source_ids: Some(source_ids),
                        ..
                    },
                ] if returning
                    .iter()
                    .any(|column| column.eq_ignore_ascii_case("document_json")) =>
                {
                    match registry
                        .source_copy_documents(owner, &id, kind, source_ids)
                        .await
                    {
                        Ok(documents) => documents,
                        Err(error) => return error_response(error),
                    }
                }
                _ => HashMap::new(),
            };
            match registry
                .stage_product_commands(
                    owner,
                    &id,
                    commands.clone(),
                    returning
                        .iter()
                        .any(|column| column.eq_ignore_ascii_case("document_json")),
                )
                .await
            {
                Ok((snapshot, _staged_commands, _deleted_documents)) if returning.is_empty() => {
                    Json(snapshot).into_response()
                }
                Ok((snapshot, staged_commands, deleted_documents)) => {
                    let result = match commands.as_slice() {
                        [
                            kaveon_sql::parser::ProductDmlCommand::InsertSelect {
                                kind,
                                source_ids: Some(source_ids),
                                ..
                            },
                        ] => product_insert_select_returning_response(
                            &snapshot,
                            &returning,
                            kind,
                            source_ids,
                            &source_copy_documents,
                        ),
                        _ => product_returning_response(
                            &snapshot,
                            &returning,
                            if dml.kind == kaveon_sql::parser::NativeDmlKind::Merge
                                || commands.iter().any(|command| {
                                    matches!(
                                        command,
                                        kaveon_sql::parser::ProductDmlCommand::InsertSelect { .. }
                                            | kaveon_sql::parser::ProductDmlCommand::UpdateFrom { .. }
                                            | kaveon_sql::parser::ProductDmlCommand::DeleteFrom { .. }
                                    )
                                })
                            {
                                &staged_commands
                            } else {
                                &commands
                            },
                            output_pseudo.as_deref(),
                            &deleted_documents,
                            &ignored_conflicts,
                        ),
                    };
                    match result {
                        Ok(body) => Json(body).into_response(),
                        Err(error) => error_response(RegistryError::Invalid(error)),
                    }
                }
                Err(error) => error_response(error),
            }
        }
    }
}

fn product_insert_select_returning_response(
    snapshot: &CatalogSnapshot,
    columns: &[String],
    kind: &str,
    source_ids: &[String],
    source_documents: &HashMap<String, String>,
) -> Result<serde_json::Value, String> {
    let product_kind = record_kind(kind).map_err(|error| format!("{error:?}"))?;
    let mut rows = Vec::new();
    for id in source_ids {
        let Some(record) = snapshot
            .product_record(product_kind, id)
            .map_err(|error| error.to_string())?
        else {
            continue;
        };
        let mut row = serde_json::Map::new();
        for column in columns {
            match column.to_ascii_lowercase().as_str() {
                "id" => {
                    row.insert("id".into(), serde_json::Value::String(id.clone()));
                }
                "revision" => {
                    row.insert("revision".into(), serde_json::json!(record.revision));
                }
                "document_json" => {
                    let document = source_documents
                        .get(id)
                        .cloned()
                        .ok_or_else(|| format!("product document is missing: {id}"))?;
                    row.insert("document_json".into(), serde_json::Value::String(document));
                }
                other => {
                    return Err(format!("unsupported source-copy RETURNING column: {other}"));
                }
            }
        }
        rows.push(serde_json::Value::Object(row));
    }
    let mut body = serde_json::to_value(snapshot).map_err(|error| error.to_string())?;
    body.as_object_mut()
        .ok_or_else(|| "transaction snapshot is not a JSON object".to_owned())?
        .insert("returning".to_owned(), serde_json::Value::Array(rows));
    Ok(body)
}

fn product_returning_response(
    snapshot: &CatalogSnapshot,
    columns: &[String],
    commands: &[kaveon_sql::parser::ProductDmlCommand],
    output_pseudo: Option<&str>,
    deleted_documents: &[Option<String>],
    ignored_conflicts: &[bool],
) -> Result<serde_json::Value, String> {
    let rows = commands
        .iter()
        .enumerate()
        .filter(|(command_index, _)| {
            !ignored_conflicts
                .get(*command_index)
                .copied()
                .unwrap_or(false)
        })
        .filter(|(_, command)| match output_pseudo {
            // SQL Server MERGE OUTPUT emits one row for each action.  An
            // update contributes to both pseudo-tables; an insert/delete
            // contributes only to its corresponding side.  The staging path
            // has already resolved MERGE actions to ordinary commands and
            // removed source rows that were no-ops.
            Some("inserted") => matches!(
                command,
                kaveon_sql::parser::ProductDmlCommand::Create { .. }
                    | kaveon_sql::parser::ProductDmlCommand::CreateIgnore { .. }
                    | kaveon_sql::parser::ProductDmlCommand::Update { .. }
                    | kaveon_sql::parser::ProductDmlCommand::Upsert { .. }
                    | kaveon_sql::parser::ProductDmlCommand::MergeUpdate { .. }
                    | kaveon_sql::parser::ProductDmlCommand::MergeCreateIfAbsent { .. }
            ),
            Some("deleted") => matches!(
                command,
                kaveon_sql::parser::ProductDmlCommand::Delete { .. }
                    | kaveon_sql::parser::ProductDmlCommand::Update { .. }
                    | kaveon_sql::parser::ProductDmlCommand::MergeDeleteOrInsert { .. }
                    | kaveon_sql::parser::ProductDmlCommand::MergeDelete { .. }
                    | kaveon_sql::parser::ProductDmlCommand::MergeUpdate { .. }
                    | kaveon_sql::parser::ProductDmlCommand::MergeDeleteNotMatched { .. }
                    | kaveon_sql::parser::ProductDmlCommand::MergeUpdateNotMatched { .. }
            ),
            _ => true,
        })
        .map(|(command_index, command)| {
            if let kaveon_sql::parser::ProductDmlCommand::MergeUpdate {
                kind,
                id,
                document_json,
            } = command
            {
                let product_kind = record_kind(kind).map_err(|error| format!("{error:?}"))?;
                let Some(record) = snapshot
                    .product_record(product_kind, id)
                    .map_err(|error| error.to_string())?
                else {
                    // An unmatched source row is a MERGE no-op and produces
                    // no OUTPUT row.
                    return Ok(None);
                };
                let mut row = serde_json::Map::new();
                for column in columns {
                    match column.as_str() {
                        "id" => {
                            row.insert("id".to_owned(), serde_json::Value::String(id.clone()));
                        }
                        "revision" => {
                            row.insert("revision".to_owned(), serde_json::json!(record.revision));
                        }
                        "document_json" => {
                            row.insert(
                                "document_json".to_owned(),
                                serde_json::Value::String(document_json.clone()),
                            );
                        }
                        _ => unreachable!("parser validates product RETURNING columns"),
                    }
                }
                return Ok(Some(serde_json::Value::Object(row)));
            }
            let (id, revision) = match command {
                kaveon_sql::parser::ProductDmlCommand::Create { id, .. }
                | kaveon_sql::parser::ProductDmlCommand::CreateIgnore { id, .. } => (id.clone(), 1),
                kaveon_sql::parser::ProductDmlCommand::Update {
                    id,
                    expected_revision,
                    ..
                } => (
                    id.clone(),
                    expected_revision
                        .checked_add(1)
                        .ok_or_else(|| "product revision overflow".to_owned())?,
                ),
                kaveon_sql::parser::ProductDmlCommand::UpdateFrom { .. } => {
                    return Err("product UPDATE FROM must be resolved before RETURNING".to_owned());
                }
                kaveon_sql::parser::ProductDmlCommand::DeleteFrom { .. } => {
                    return Err("product DELETE USING must be resolved before RETURNING".to_owned());
                }
                kaveon_sql::parser::ProductDmlCommand::Delete {
                    id,
                    expected_revision,
                    ..
                } => (id.clone(), *expected_revision),
                kaveon_sql::parser::ProductDmlCommand::InsertSelect { .. } => {
                    return Err("product INSERT SELECT cannot use RETURNING".to_owned());
                }
                kaveon_sql::parser::ProductDmlCommand::Upsert { kind, id, .. } => {
                    if kind == "typed_row" {
                        let mut matches =
                            snapshot.typed_rows.values().filter_map(|rows| rows.get(id));
                        let revision = matches
                            .next()
                            .ok_or_else(|| "typed row does not exist".to_owned())?
                            .revision;
                        if matches.next().is_some() {
                            return Err("typed row primary key is ambiguous across tables".into());
                        }
                        (id.clone(), revision)
                    } else {
                        let product_kind =
                            record_kind(kind).map_err(|error| format!("{error:?}"))?;
                        let revision = snapshot
                            .product_record(product_kind, id)
                            .map_err(|error| error.to_string())?
                            .map(|record| record.revision)
                            .unwrap_or(1);
                        (id.clone(), revision)
                    }
                }
                kaveon_sql::parser::ProductDmlCommand::MergeDeleteOrInsert { .. } => {
                    return Err(
                        "MERGE delete-or-insert must be resolved before RETURNING".to_owned()
                    );
                }
                kaveon_sql::parser::ProductDmlCommand::MergeDelete { .. } => {
                    return Err("MERGE matched-delete must be resolved before RETURNING".to_owned());
                }
                kaveon_sql::parser::ProductDmlCommand::MergeUpdate { .. } => {
                    return Err("MERGE matched-update must be resolved before RETURNING".to_owned());
                }
                kaveon_sql::parser::ProductDmlCommand::MergeUpdateOrCreateFiltered { .. } => {
                    return Err(
                        "MERGE filtered update-or-insert must be resolved before RETURNING"
                            .to_owned(),
                    );
                }
                kaveon_sql::parser::ProductDmlCommand::MergeDeleteOrCreateFiltered { .. } => {
                    return Err(
                        "MERGE filtered delete-or-insert must be resolved before RETURNING"
                            .to_owned(),
                    );
                }
                kaveon_sql::parser::ProductDmlCommand::MergeCreateIfAbsent { .. } => {
                    return Err(
                        "MERGE unmatched-insert must be resolved before RETURNING".to_owned()
                    );
                }
                kaveon_sql::parser::ProductDmlCommand::MergeDeleteNotMatched { .. } => {
                    return Err(
                        "MERGE not-matched-by-source deletes must be resolved before RETURNING"
                            .to_owned(),
                    );
                }
                kaveon_sql::parser::ProductDmlCommand::MergeUpdateNotMatched { .. } => {
                    return Err(
                        "MERGE not-matched-by-source updates must be resolved before RETURNING"
                            .to_owned(),
                    );
                }
            };
            let mut row = serde_json::Map::new();
            for column in columns {
                match column.as_str() {
                    "id" => {
                        row.insert("id".to_owned(), serde_json::Value::String(id.clone()));
                    }
                    "revision" => {
                        row.insert("revision".to_owned(), serde_json::json!(revision));
                    }
                    "action" => {
                        let action = match command {
                            kaveon_sql::parser::ProductDmlCommand::Create { .. }
                            | kaveon_sql::parser::ProductDmlCommand::CreateIgnore { .. }
                            | kaveon_sql::parser::ProductDmlCommand::MergeCreateIfAbsent {
                                ..
                            } => "INSERT",
                            kaveon_sql::parser::ProductDmlCommand::Update { .. }
                            | kaveon_sql::parser::ProductDmlCommand::UpdateFrom { .. }
                            | kaveon_sql::parser::ProductDmlCommand::MergeUpdate { .. }
                            | kaveon_sql::parser::ProductDmlCommand::MergeUpdateOrCreateFiltered {
                                ..
                            }
                            | kaveon_sql::parser::ProductDmlCommand::MergeDeleteOrCreateFiltered {
                                ..
                            }
                            | kaveon_sql::parser::ProductDmlCommand::MergeUpdateNotMatched {
                                ..
                            } => "UPDATE",
                            kaveon_sql::parser::ProductDmlCommand::Delete { .. }
                            | kaveon_sql::parser::ProductDmlCommand::MergeDelete { .. }
                            | kaveon_sql::parser::ProductDmlCommand::MergeDeleteNotMatched {
                                ..
                            } => "DELETE",
                            kaveon_sql::parser::ProductDmlCommand::DeleteFrom { .. } => {
                                return Err(
                                    "product DELETE USING must be resolved before $action".into()
                                );
                            }
                            kaveon_sql::parser::ProductDmlCommand::MergeDeleteOrInsert {
                                ..
                            }
                            | kaveon_sql::parser::ProductDmlCommand::Upsert { .. }
                            | kaveon_sql::parser::ProductDmlCommand::InsertSelect { .. } => {
                                return Err(
                                    "MERGE $action requires resolved action commands".into()
                                );
                            }
                        };
                        row.insert(
                            "action".to_owned(),
                            serde_json::Value::String(action.to_owned()),
                        );
                    }
                    "document_json" => {
                        let document = match command {
                            kaveon_sql::parser::ProductDmlCommand::Create {
                                document_json, ..
                            }
                            | kaveon_sql::parser::ProductDmlCommand::CreateIgnore {
                                document_json,
                                ..
                            }
                            | kaveon_sql::parser::ProductDmlCommand::Update {
                                document_json, ..
                            }
                            | kaveon_sql::parser::ProductDmlCommand::Upsert {
                                document_json, ..
                            } => document_json.clone(),
                            kaveon_sql::parser::ProductDmlCommand::UpdateFrom { .. } => {
                                return Err(
                                    "product UPDATE FROM must be resolved before RETURNING"
                                        .to_owned(),
                                );
                            }
                            kaveon_sql::parser::ProductDmlCommand::DeleteFrom { .. } => {
                                return Err(
                                    "product DELETE USING must be resolved before RETURNING"
                                        .to_owned(),
                                );
                            }
                            kaveon_sql::parser::ProductDmlCommand::MergeDeleteOrInsert {
                                ..
                            } => {
                                return Err(
                                    "document_json RETURNING is unavailable for this command"
                                        .to_owned(),
                                );
                            }
                            kaveon_sql::parser::ProductDmlCommand::MergeDelete { .. } => {
                                return Err(
                                    "document_json RETURNING is unavailable for this command"
                                        .to_owned(),
                                );
                            }
                            kaveon_sql::parser::ProductDmlCommand::MergeUpdate { .. } => {
                                return Err(
                                    "document_json RETURNING is unavailable for this command"
                                        .to_owned(),
                                );
                            }
                            kaveon_sql::parser::ProductDmlCommand::MergeUpdateOrCreateFiltered {
                                ..
                            } => {
                                return Err(
                                    "document_json RETURNING is unavailable for this command"
                                        .to_owned(),
                                );
                            }
                            kaveon_sql::parser::ProductDmlCommand::MergeDeleteOrCreateFiltered {
                                ..
                            } => {
                                return Err(
                                    "document_json RETURNING is unavailable for this command"
                                        .to_owned(),
                                );
                            }
                            kaveon_sql::parser::ProductDmlCommand::MergeCreateIfAbsent {
                                ..
                            } => {
                                return Err(
                                    "document_json RETURNING is unavailable for this command"
                                        .to_owned(),
                                );
                            }
                            kaveon_sql::parser::ProductDmlCommand::MergeDeleteNotMatched {
                                ..
                            } => {
                                return Err(
                                    "document_json RETURNING is unavailable for this command"
                                        .to_owned(),
                                );
                            }
                            kaveon_sql::parser::ProductDmlCommand::MergeUpdateNotMatched {
                                ..
                            } => {
                                return Err(
                                    "document_json RETURNING is unavailable for this command"
                                        .to_owned(),
                                );
                            }
                            kaveon_sql::parser::ProductDmlCommand::Delete { .. } => {
                                deleted_documents
                                    .get(command_index)
                                    .and_then(Option::as_ref)
                                    .cloned()
                                    .ok_or_else(|| {
                                        "document_json RETURNING is unavailable for this command"
                                            .to_owned()
                                    })?
                            }
                            kaveon_sql::parser::ProductDmlCommand::InsertSelect { .. } => {
                                return Err(
                                    "document_json RETURNING is unavailable for this command"
                                        .to_owned(),
                                );
                            }
                        };
                        row.insert(
                            "document_json".to_owned(),
                            serde_json::Value::String(document),
                        );
                    }
                    _ => unreachable!("parser validates product RETURNING columns"),
                }
            }
            Ok(Some(serde_json::Value::Object(row)))
        })
        .collect::<Result<Vec<_>, String>>()?
        .into_iter()
        .flatten()
        .collect();
    let mut body = serde_json::to_value(snapshot).map_err(|error| error.to_string())?;
    body.as_object_mut()
        .ok_or_else(|| "transaction snapshot is not a JSON object".to_owned())?
        .insert("returning".to_owned(), serde_json::Value::Array(rows));
    Ok(body)
}

async fn commit_transaction(registry: &TransactionRegistry, owner: &str, id: &str) -> Response {
    let mut transaction = match registry.take(owner, id).await {
        Ok(transaction) => transaction,
        Err(error) => return error_response(error),
    };
    if let Err(error) = transaction.bind_request_digest(owner.as_bytes()) {
        return error_response(RegistryError::Invalid(error.to_string()));
    }
    commit_outcome_response(transaction.commit().await, id)
}

async fn rollback_transaction(registry: &TransactionRegistry, owner: &str, id: &str) -> Response {
    match registry.take(owner, id).await {
        Ok(transaction) => {
            let _ = transaction.rollback();
            StatusCode::NO_CONTENT.into_response()
        }
        Err(error) => error_response(error),
    }
}

async fn begin(
    State(state): State<Arc<AppState>>,
    Extension(identity): Extension<Identity>,
) -> Response {
    match state.product_transactions.begin(&identity.principal).await {
        Ok(response) => (StatusCode::CREATED, Json(response)).into_response(),
        Err(error) => error_response(error),
    }
}

async fn stage(
    State(state): State<Arc<AppState>>,
    Extension(identity): Extension<Identity>,
    Path(id): Path<String>,
    Json(request): Json<StageRequest>,
) -> Response {
    match state
        .product_transactions
        .stage(&identity.principal, &id, request)
        .await
    {
        Ok(snapshot) => Json(snapshot).into_response(),
        Err(error) => error_response(error),
    }
}

async fn commit(
    State(state): State<Arc<AppState>>,
    Extension(identity): Extension<Identity>,
    Path(id): Path<String>,
) -> Response {
    let mut transaction = match state
        .product_transactions
        .take(&identity.principal, &id)
        .await
    {
        Ok(transaction) => transaction,
        Err(error) => return error_response(error),
    };
    if let Err(error) = transaction.bind_request_digest(identity.principal.as_bytes()) {
        return error_response(RegistryError::Invalid(error.to_string()));
    }
    let outcome = transaction.commit().await;
    if matches!(
        outcome,
        Ok(CommitOutcome::Committed(_) | CommitOutcome::Replayed(_))
    ) {
        // A replay confirms that the product write is durable too. Clear the
        // result cache for both outcomes so a retry cannot expose a stale
        // query result after an ambiguous client response.
        state.result_cache.clear();
    }
    commit_outcome_response(outcome, &id)
}

fn commit_outcome_response(
    outcome: Result<CommitOutcome, kaveon_catalog::product_transaction::TransactionError>,
    transaction_id: &str,
) -> Response {
    match outcome {
        Ok(CommitOutcome::Committed(snapshot) | CommitOutcome::Replayed(snapshot)) => {
            Json(snapshot).into_response()
        }
        Ok(CommitOutcome::Conflict) => error_response(RegistryError::Conflict),
        Ok(CommitOutcome::Rejected) => {
            error_response(RegistryError::Invalid("transaction rejected".into()))
        }
        Ok(CommitOutcome::Indeterminate) => (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(serde_json::json!({
                "error": "transaction outcome is indeterminate; the operation is neither confirmed committed nor confirmed rolled back",
                "code": "TRANSACTION_OUTCOME_INDETERMINATE",
                "transaction_id": transaction_id,
                "recovery_required": true,
                "recovery_action": "inspect the durable catalog head and operation history before retrying"
            })),
        )
            .into_response(),
        Err(error) => error_response(RegistryError::Invalid(error.to_string())),
    }
}

async fn rollback(
    State(state): State<Arc<AppState>>,
    Extension(identity): Extension<Identity>,
    Path(id): Path<String>,
) -> Response {
    match state
        .product_transactions
        .take(&identity.principal, &id)
        .await
    {
        Ok(transaction) => {
            let _ = transaction.rollback();
            StatusCode::NO_CONTENT.into_response()
        }
        Err(error) => error_response(error),
    }
}

fn error_response(error: RegistryError) -> Response {
    let (status, message) = match error {
        RegistryError::Disabled => (
            StatusCode::SERVICE_UNAVAILABLE,
            "product transactions are not configured".to_owned(),
        ),
        RegistryError::Capacity => (
            StatusCode::TOO_MANY_REQUESTS,
            "transaction session capacity exceeded".to_owned(),
        ),
        RegistryError::Missing => (
            StatusCode::NOT_FOUND,
            "transaction session not found or expired".to_owned(),
        ),
        RegistryError::ProductMissing => {
            (StatusCode::NOT_FOUND, "product record not found".to_owned())
        }
        RegistryError::SessionForbidden => (
            StatusCode::FORBIDDEN,
            "transaction session belongs to another principal".to_owned(),
        ),
        RegistryError::OwnershipForbidden => (
            StatusCode::FORBIDDEN,
            "product record owner does not match the authenticated principal".to_owned(),
        ),
        RegistryError::Conflict => (StatusCode::CONFLICT, "transaction conflict".to_owned()),
        RegistryError::Invalid(message) => (StatusCode::BAD_REQUEST, message),
        RegistryError::Corrupt => (
            StatusCode::INTERNAL_SERVER_ERROR,
            "product document is missing, corrupt, or does not match its digest".to_owned(),
        ),
        RegistryError::Unavailable => (
            StatusCode::SERVICE_UNAVAILABLE,
            "product store is unavailable".to_owned(),
        ),
    };
    (status, Json(serde_json::json!({"error": message}))).into_response()
}

#[cfg(test)]
mod tests {
    use kaveon_catalog::{
        product_manifest::{CatalogSnapshot, ImmutableFileRef, TableManifestRef},
        product_metrics::TransactionMetrics,
    };
    use kaveon_storage::AdlsConditionalCommit;
    use object_store::memory::InMemory;
    use std::sync::Arc;

    use super::*;
    use kaveon_sql::parser::ProductDmlCommand;
    const DIGEST: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";

    async fn registry() -> (TransactionRegistry, ProductCatalogCommit) {
        let catalog = ProductCatalogCommit::new(
            AdlsConditionalCommit::new(Arc::new(InMemory::new())),
            "product",
            Arc::new(TransactionMetrics::default()),
        )
        .unwrap();
        catalog
            .initialize(CatalogSnapshot::empty("genesis").unwrap())
            .await;
        (TransactionRegistry::enabled(catalog.clone()), catalog)
    }

    #[tokio::test]
    async fn indeterminate_commit_response_requires_durable_recovery() {
        let response = commit_outcome_response(Ok(CommitOutcome::Indeterminate), "transaction-1");
        assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
        let body = axum::body::to_bytes(response.into_body(), 4096)
            .await
            .unwrap();
        let value: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(value["code"], "TRANSACTION_OUTCOME_INDETERMINATE");
        assert_eq!(value["transaction_id"], "transaction-1");
        assert_eq!(value["recovery_required"], true);
        assert!(
            value["error"]
                .as_str()
                .unwrap()
                .contains("neither confirmed committed nor confirmed rolled back")
        );
    }

    #[tokio::test]
    async fn authorization_errors_distinguish_session_and_record_ownership() {
        for (error, expected) in [
            (
                RegistryError::SessionForbidden,
                "transaction session belongs to another principal",
            ),
            (
                RegistryError::OwnershipForbidden,
                "product record owner does not match the authenticated principal",
            ),
        ] {
            let response = error_response(error);
            assert_eq!(response.status(), StatusCode::FORBIDDEN);
            let body = axum::body::to_bytes(response.into_body(), 4096)
                .await
                .unwrap();
            let value: serde_json::Value = serde_json::from_slice(&body).unwrap();
            assert_eq!(value["error"], expected);
        }
    }

    #[tokio::test]
    async fn transaction_metrics_are_payload_free_and_available_when_enabled() {
        let (registry, catalog) = registry().await;
        let snapshot = registry.metrics().unwrap();
        assert_eq!(snapshot.attempts, 1);
        assert_eq!(snapshot.committed, 1);
        assert_eq!(snapshot.in_flight, 0);
        assert_eq!(snapshot.replayed, 0);
        assert_eq!(snapshot.conflicts, 0);
        assert_eq!(catalog.transaction_metrics().attempts, 1);
        assert!(matches!(
            TransactionRegistry::disabled().metrics(),
            Err(RegistryError::Disabled)
        ));
    }

    #[tokio::test]
    async fn a_session_cannot_stage_a_catalog_grant_for_its_owner() {
        use kaveon_catalog::product_manifest::{TypedRow, TypedValue};
        let (registry, _) = registry().await;
        let begun = registry.begin("ana").await.unwrap();
        let row = TypedRow {
            primary_key: "lake/ana".into(),
            revision: 1,
            columns: BTreeMap::from([
                ("principal".to_owned(), TypedValue::String("ana".into())),
                ("catalog".to_owned(), TypedValue::String("lake".into())),
                ("access".to_owned(), TypedValue::String("manage".into())),
                ("granted_by".to_owned(), TypedValue::String("ana".into())),
                ("granted_at_ms".to_owned(), TypedValue::Integer(1)),
            ]),
            unique_keys: BTreeMap::new(),
        };
        // As a staged change.
        let staged = registry
            .stage(
                "ana",
                &begun.transaction_id,
                StageRequest {
                    change: CatalogChange::InsertTypedRow {
                        table: crate::catalog_access::GRANTS_TABLE.into(),
                        row: row.clone(),
                    },
                    document: None,
                },
            )
            .await;
        assert!(
            matches!(&staged, Err(RegistryError::Invalid(message)) if message.contains("catalog access API")),
            "{staged:?}"
        );
        // As typed-row DML.
        let dml = registry
            .stage_product_command(
                "ana",
                &begun.transaction_id,
                ProductDmlCommand::Create {
                    kind: "typed_row".into(),
                    id: "lake/ana".into(),
                    document_json: serde_json::json!({
                        "table": crate::catalog_access::GRANTS_TABLE,
                        "primary_key": "lake/ana",
                        "revision": 1,
                        "columns": row.columns,
                        "unique_keys": {},
                        "owner_principal": "ana"
                    })
                    .to_string(),
                },
            )
            .await;
        assert!(
            matches!(&dml, Err(RegistryError::Invalid(message)) if message.contains("catalog access API")),
            "{dml:?}"
        );
        // The session is intact and stages ordinary work.
        let snapshot = registry
            .stage(
                "ana",
                &begun.transaction_id,
                StageRequest {
                    change: CatalogChange::InsertTypedRow {
                        table: "notes".into(),
                        row,
                    },
                    document: None,
                },
            )
            .await
            .unwrap();
        assert!(snapshot.typed_rows.contains_key("notes"));
        assert!(
            !snapshot
                .typed_rows
                .contains_key(crate::catalog_access::GRANTS_TABLE)
        );
    }

    #[tokio::test]
    async fn transaction_sessions_do_not_survive_registry_restart() {
        let (registry, catalog) = registry().await;
        let begun = registry.begin("alice").await.unwrap();
        let restarted = TransactionRegistry::enabled(catalog);
        assert!(matches!(
            restarted.take("alice", &begun.transaction_id).await,
            Err(RegistryError::Missing)
        ));
    }

    #[tokio::test]
    async fn committed_product_records_survive_registry_restart() {
        let (registry, catalog) = registry().await;
        let begun = registry.begin("alice").await.unwrap();
        let response = execute_sql_request(
            &registry,
            "alice",
            SqlTransactionRequest {
                sql: r#"INSERT INTO kaveon.product.datasets (id, document_json) VALUES ('restart-proof', '{"name":"Restart proof"}')"#.into(),
                transaction_id: Some(begun.transaction_id.clone()),
                parameters: vec![],
                named_parameters: HashMap::new(),
            },
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);
        let response = execute_sql_request(
            &registry,
            "alice",
            SqlTransactionRequest {
                sql: "COMMIT".into(),
                transaction_id: Some(begun.transaction_id),
                parameters: vec![],
                named_parameters: HashMap::new(),
            },
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);

        let restarted = TransactionRegistry::enabled(catalog);
        let record = restarted
            .read_product(
                &Identity {
                    principal: "alice".into(),
                    display_identity: None,
                    role: crate::security::Role::Analyst,
                },
                ProductRecordKind::Dataset,
                "restart-proof",
            )
            .await
            .unwrap();
        assert_eq!(record.revision, 1);
        assert_eq!(record.document["name"], "Restart proof");
    }

    #[test]
    fn recovery_resolution_exposes_committed_conflict_and_unresolved_states() {
        let operation_id = "operation-1";
        let committed = recovery_resolution_response(
            operation_id,
            OperationResolution::Committed(CatalogSnapshot::empty("snapshot-1").unwrap()),
        );
        assert_eq!(committed.status(), StatusCode::OK);
        let conflict = recovery_resolution_response(operation_id, OperationResolution::Conflict);
        assert_eq!(conflict.status(), StatusCode::OK);
        let unresolved =
            recovery_resolution_response(operation_id, OperationResolution::Unresolved);
        assert_eq!(unresolved.status(), StatusCode::SERVICE_UNAVAILABLE);
    }

    #[test]
    fn commit_outcome_statuses_cover_all_durable_result_states() {
        let snapshot = CatalogSnapshot::empty("snapshot-1").unwrap();
        assert_eq!(
            commit_outcome_response(Ok(CommitOutcome::Committed(snapshot.clone())), "tx-1")
                .status(),
            StatusCode::OK
        );
        assert_eq!(
            commit_outcome_response(Ok(CommitOutcome::Replayed(snapshot)), "tx-1").status(),
            StatusCode::OK
        );
        assert_eq!(
            commit_outcome_response(Ok(CommitOutcome::Conflict), "tx-1").status(),
            StatusCode::CONFLICT
        );
        assert_eq!(
            commit_outcome_response(Ok(CommitOutcome::Rejected), "tx-1").status(),
            StatusCode::BAD_REQUEST
        );
        assert_eq!(
            commit_outcome_response(Ok(CommitOutcome::Indeterminate), "tx-1").status(),
            StatusCode::SERVICE_UNAVAILABLE
        );
    }

    #[test]
    fn transaction_operation_id_is_stable_for_recovery_lookup() {
        assert_eq!(
            operation_id_for_transaction("123e4567-e89b-12d3-a456-426614174000"),
            "operation-123e4567e89b12d3a456426614174000"
        );
    }

    fn stage_request(name: &str) -> StageRequest {
        StageRequest {
            change: CatalogChange::Put {
                table: format!("app.{name}"),
                reference: TableManifestRef {
                    manifest: ImmutableFileRef {
                        path: format!("tables/{name}.json"),
                        sha256: DIGEST.into(),
                    },
                    parquet_files: vec![],
                },
            },
            document: None,
        }
    }

    #[tokio::test]
    async fn owner_can_stage_commit_and_session_is_removed() {
        let (registry, catalog) = registry().await;
        let begun = registry.begin("alice").await.unwrap();
        assert_eq!(
            registry
                .stage("alice", &begun.transaction_id, stage_request("users"))
                .await
                .unwrap()
                .tables
                .len(),
            1
        );
        let transaction = registry.take("alice", &begun.transaction_id).await.unwrap();
        assert!(matches!(
            transaction.commit().await.unwrap(),
            CommitOutcome::Committed(_)
        ));
        assert_eq!(
            registry
                .take("alice", &begun.transaction_id)
                .await
                .err()
                .unwrap(),
            RegistryError::Missing
        );
        assert!(
            catalog
                .read_current()
                .await
                .unwrap()
                .tables
                .contains_key("app.users")
        );
    }

    #[tokio::test]
    async fn another_principal_cannot_observe_or_end_a_session() {
        let (registry, _) = registry().await;
        let begun = registry.begin("alice").await.unwrap();
        assert_eq!(
            registry
                .stage("bob", &begun.transaction_id, stage_request("users"))
                .await
                .err()
                .unwrap(),
            RegistryError::SessionForbidden
        );
        assert_eq!(
            registry
                .take("bob", &begun.transaction_id)
                .await
                .err()
                .unwrap(),
            RegistryError::SessionForbidden
        );
        let _ = registry
            .take("alice", &begun.transaction_id)
            .await
            .unwrap()
            .rollback();
    }

    #[tokio::test]
    async fn rollback_removes_session_without_publication() {
        let (registry, catalog) = registry().await;
        let begun = registry.begin("alice").await.unwrap();
        let _ = registry
            .stage("alice", &begun.transaction_id, stage_request("users"))
            .await
            .unwrap();
        let _ = registry
            .take("alice", &begun.transaction_id)
            .await
            .unwrap()
            .rollback();
        assert!(catalog.read_current().await.unwrap().tables.is_empty());
        assert_eq!(
            registry
                .take("alice", &begun.transaction_id)
                .await
                .err()
                .unwrap(),
            RegistryError::Missing
        );
    }

    #[tokio::test]
    async fn per_principal_capacity_is_bounded() {
        let (registry, _) = registry().await;
        for _ in 0..MAX_SESSIONS_PER_PRINCIPAL {
            registry.begin("alice").await.unwrap();
        }
        assert_eq!(
            registry.begin("alice").await.err().unwrap(),
            RegistryError::Capacity
        );
        assert!(registry.begin("bob").await.is_ok());
    }

    #[tokio::test]
    async fn expired_session_is_removed_and_cannot_publish() {
        let (mut registry, catalog) = registry().await;
        registry.ttl = Duration::ZERO;
        let begun = registry.begin("alice").await.unwrap();
        assert!(matches!(
            registry
                .stage("alice", &begun.transaction_id, stage_request("users"))
                .await,
            Err(RegistryError::Missing)
        ));
        assert!(catalog.read_current().await.unwrap().tables.is_empty());
    }

    #[tokio::test]
    async fn product_sql_commands_stage_revisioned_documents_and_delete() {
        let (registry, catalog) = registry().await;
        let begun = registry.begin("alice").await.unwrap();
        let preview = registry
            .stage_product_command(
                "alice",
                &begun.transaction_id,
                ProductDmlCommand::Create {
                    kind: "dataset".into(),
                    id: "orders".into(),
                    document_json: r#"{"name":"Orders"}"#.into(),
                },
            )
            .await
            .unwrap();
        assert_eq!(
            preview
                .product_record(ProductRecordKind::Dataset, "orders")
                .unwrap()
                .unwrap()
                .revision,
            1
        );
        let mut transaction = registry.take("alice", &begun.transaction_id).await.unwrap();
        transaction.bind_request_digest(b"alice").unwrap();
        assert!(matches!(
            transaction.commit().await.unwrap(),
            CommitOutcome::Committed(_)
        ));

        let begun = registry.begin("alice").await.unwrap();
        let preview = registry
            .stage_product_command(
                "alice",
                &begun.transaction_id,
                ProductDmlCommand::Update {
                    kind: "dataset".into(),
                    id: "orders".into(),
                    expected_revision: 1,
                    document_json: r#"{"name":"Orders v2"}"#.into(),
                },
            )
            .await
            .unwrap();
        assert_eq!(
            preview
                .product_record(ProductRecordKind::Dataset, "orders")
                .unwrap()
                .unwrap()
                .revision,
            2
        );
        let mut transaction = registry.take("alice", &begun.transaction_id).await.unwrap();
        transaction.bind_request_digest(b"alice").unwrap();
        assert!(matches!(
            transaction.commit().await.unwrap(),
            CommitOutcome::Committed(_)
        ));

        let begun = registry.begin("alice").await.unwrap();
        let preview = registry
            .stage_product_command(
                "alice",
                &begun.transaction_id,
                ProductDmlCommand::Delete {
                    kind: "dataset".into(),
                    id: "orders".into(),
                    expected_revision: 2,
                },
            )
            .await
            .unwrap();
        assert!(
            preview
                .product_record(ProductRecordKind::Dataset, "orders")
                .unwrap()
                .is_none()
        );
        let mut transaction = registry.take("alice", &begun.transaction_id).await.unwrap();
        transaction.bind_request_digest(b"alice").unwrap();
        assert!(matches!(
            transaction.commit().await.unwrap(),
            CommitOutcome::Committed(_)
        ));
        assert!(
            catalog
                .read_current()
                .await
                .unwrap()
                .product_record(ProductRecordKind::Dataset, "orders")
                .unwrap()
                .is_none()
        );
    }

    #[tokio::test]
    async fn invalid_product_json_does_not_change_the_transaction_preview() {
        let (registry, _) = registry().await;
        let begun = registry.begin("alice").await.unwrap();
        let error = registry
            .stage_product_command(
                "alice",
                &begun.transaction_id,
                ProductDmlCommand::Create {
                    kind: "dataset".into(),
                    id: "orders".into(),
                    document_json: "[]".into(),
                },
            )
            .await
            .unwrap_err();
        assert!(matches!(error, RegistryError::Invalid(_)));

        let transaction = registry.take("alice", &begun.transaction_id).await.unwrap();
        assert_eq!(transaction.staged_change_count(), 0);
    }

    #[test]
    fn dlm_definition_schema_is_canonical_and_references_its_dataset() {
        let (_, (path, bytes), references, _) = product_document(
            ProductRecordKind::DlmDefinition,
            "orders",
            1,
            r#"{"dataset_revision":3,"dataset_id":"orders"}"#,
        )
        .unwrap();
        assert_eq!(bytes, br#"{"dataset_id":"orders","dataset_revision":3}"#);
        assert!(path.starts_with("products/dlm_definition/orders/1-"));
        assert_eq!(
            references,
            BTreeSet::from([ProductRecordReference {
                kind: ProductRecordKind::Dataset,
                id: "orders".into(),
            }])
        );
    }

    #[test]
    fn dlm_definition_rejects_unpinned_or_mismatched_documents() {
        for document in [
            r#"{"dataset_id":"orders"}"#,
            r#"{"dataset_id":"orders","dataset_revision":0}"#,
            r#"{"dataset_id":"other","dataset_revision":1}"#,
            r#"{"dataset_id":"orders","dataset_revision":1,"run":{}}"#,
        ] {
            assert!(
                product_document(ProductRecordKind::DlmDefinition, "orders", 1, document).is_err(),
                "unexpectedly accepted {document}"
            );
        }
    }

    #[test]
    fn favorite_is_owner_unique_and_references_supported_target() {
        let (_, _, references, values) = product_document(ProductRecordKind::Favorite, "fav", 1,
            r#"{"user_email":"alice","object_type":"dataset","object_id":"orders","object_name":"Orders"}"#).unwrap();
        assert_eq!(
            references,
            BTreeSet::from([ProductRecordReference {
                kind: ProductRecordKind::Dataset,
                id: "orders".into()
            }])
        );
        assert_eq!(values["favorite_owner_target"], "alice:dataset:orders");
        assert!(validate_favorite_owner(ProductRecordKind::Favorite, "alice", &values).is_ok());
        assert_eq!(
            validate_favorite_owner(ProductRecordKind::Favorite, "bob", &values),
            Err(RegistryError::OwnershipForbidden)
        );
        assert!(
            product_document(
                ProductRecordKind::Favorite,
                "fav",
                1,
                r#"{"user_email":"alice","object_type":"data_source","object_id":"source"}"#
            )
            .is_err()
        );
    }

    #[test]
    fn source_accepts_only_non_secret_metadata_and_opaque_reference() {
        let document = r#"{"source_kind":"catalog","source_id":"catalog:lake","name":"Lake","catalog_identity":"lake","source_type":"adls_gen2","database_name":null,"region":null,"description":null,"is_active":true,"lifecycle":"active","secret_ref":"https://example.vault.azure.net/secrets/lake"}"#;
        let (_, _, references, _) =
            product_document(ProductRecordKind::Source, "catalog:lake", 1, document).unwrap();
        assert!(references.is_empty());
        for rejected in [
            r#"{"source_kind":"data","source_id":"data:1","secret_ref":"ref","connection_string":"secret"}"#,
            r#"{"source_kind":"data","source_id":"data:1"}"#,
            r#"{"source_kind":"data","source_id":"data:2","secret_ref":"ref"}"#,
        ] {
            assert!(product_document(ProductRecordKind::Source, "data:1", 1, rejected).is_err());
        }
    }

    #[test]
    fn user_recent_is_owner_unique_and_references_target() {
        let (_,_,references,values)=product_document(ProductRecordKind::UserRecent,"recent",1,
            r#"{"user_email":"alice","item_id":"dash","label":"Dashboard","href":"/dashboards/dash","type":"dashboard","created_at":"2026-01-01T00:00:00"}"#).unwrap();
        assert_eq!(values["recent_owner_item"], "alice:dash");
        assert_eq!(
            references,
            BTreeSet::from([ProductRecordReference {
                kind: ProductRecordKind::Dashboard,
                id: "dash".into()
            }])
        );
        assert!(validate_favorite_owner(ProductRecordKind::UserRecent, "alice", &values).is_ok());
        assert_eq!(
            validate_favorite_owner(ProductRecordKind::UserRecent, "bob", &values),
            Err(RegistryError::OwnershipForbidden)
        );

        let (_, _, prefixed_references, prefixed_values) = product_document(
            ProductRecordKind::UserRecent,
            "recent-prefixed",
            1,
            r#"{"user_email":"alice","item_id":"dashboard-dash","label":"Dashboard","href":"/dashboards/dash","type":"dashboard","created_at":"2026-01-01T00:00:00"}"#,
        )
        .unwrap();
        assert_eq!(prefixed_values["recent_owner_item"], "alice:dashboard-dash");
        assert_eq!(
            prefixed_references,
            BTreeSet::from([ProductRecordReference {
                kind: ProductRecordKind::Dashboard,
                id: "dash".into()
            }])
        );
    }

    #[test]
    fn query_history_is_owner_isolated_and_binds_optional_dataset() {
        let (_,_,references,values)=product_document(ProductRecordKind::QueryHistory,"q1",1,
            r#"{"id":"q1","sql_text":"SELECT 1","database_name":"db","executed_at":"2026-01-01T00:00:00","execution_time":1,"row_count":1,"status":"success","error_message":null,"user_email":"alice","trigger_source":"lab","dataset_id":"ds","tables_used":"[]"}"#).unwrap();
        assert_eq!(values["query_owner"], "alice");
        assert_eq!(
            references,
            BTreeSet::from([ProductRecordReference {
                kind: ProductRecordKind::Dataset,
                id: "ds".into()
            }])
        );
        assert!(validate_favorite_owner(ProductRecordKind::QueryHistory, "alice", &values).is_ok());
        assert_eq!(
            validate_favorite_owner(ProductRecordKind::QueryHistory, "bob", &values),
            Err(RegistryError::OwnershipForbidden)
        );
    }

    #[test]
    fn activity_is_actor_isolated_and_rejects_unstructured_details() {
        let (_,_,references,values)=product_document(ProductRecordKind::Activity,"a1",1,
            r#"{"id":"a1","action":"created","object_type":"catalog_source","object_id":"c1","object_name":"Lake","timestamp":"2026-01-01T00:00:00","user_email":"alice","details":{"storage_type":"adls_gen2"}}"#).unwrap();
        assert!(references.is_empty());
        assert_eq!(values["activity_actor"], "alice");
        assert!(validate_favorite_owner(ProductRecordKind::Activity, "alice", &values).is_ok());
        assert_eq!(
            validate_favorite_owner(ProductRecordKind::Activity, "bob", &values),
            Err(RegistryError::OwnershipForbidden)
        );
        assert!(product_document(ProductRecordKind::Activity,"a1",1,r#"{"id":"a1","action":"created","object_type":"source","object_id":"c1","object_name":"Lake","timestamp":"now","user_email":"alice","details":"raw"}"#).is_err());
    }

    #[tokio::test]
    async fn chart_binds_exact_dataset_revision_and_owner() {
        let (registry, _) = registry().await;
        let begun = registry.begin("alice").await.unwrap();
        for command in [
            ProductDmlCommand::Create {
                kind: "dataset".into(),
                id: "orders".into(),
                document_json: r#"{"name":"Orders"}"#.into(),
            },
            ProductDmlCommand::Create {
                kind: "chart".into(),
                id: "chart-1".into(),
                document_json: r#"{"dataset_id":"orders","dataset_revision":1,"name":"Chart"}"#
                    .into(),
            },
            ProductDmlCommand::Create {
                kind: "dashboard".into(),
                id: "dashboard-1".into(),
                document_json: r#"{"chart_revisions":{"chart-1":1},"name":"Dashboard"}"#.into(),
            },
        ] {
            registry
                .stage_product_command("alice", &begun.transaction_id, command)
                .await
                .unwrap();
        }
        let stale = registry
            .stage_product_command(
                "alice",
                &begun.transaction_id,
                ProductDmlCommand::Create {
                    kind: "chart".into(),
                    id: "chart-2".into(),
                    document_json: r#"{"dataset_id":"orders","dataset_revision":2}"#.into(),
                },
            )
            .await;
        assert!(matches!(stale, Err(RegistryError::Invalid(_))));
        let transaction = registry.take("alice", &begun.transaction_id).await.unwrap();
        let chart = transaction
            .snapshot()
            .product_record(ProductRecordKind::Chart, "chart-1")
            .unwrap()
            .unwrap();
        assert_eq!(
            chart.references,
            BTreeSet::from([ProductRecordReference {
                kind: ProductRecordKind::Dataset,
                id: "orders".into()
            }])
        );
        let dashboard = transaction
            .snapshot()
            .product_record(ProductRecordKind::Dashboard, "dashboard-1")
            .unwrap()
            .unwrap();
        assert_eq!(
            dashboard.references,
            BTreeSet::from([ProductRecordReference {
                kind: ProductRecordKind::Chart,
                id: "chart-1".into()
            }])
        );
    }

    #[tokio::test]
    async fn dlm_definition_commits_with_dataset_and_is_owner_isolated() {
        let (registry, _) = registry().await;
        let begun = registry.begin("alice").await.unwrap();
        for command in [
            ProductDmlCommand::Create {
                kind: "dataset".into(),
                id: "orders".into(),
                document_json: r#"{"name":"Orders"}"#.into(),
            },
            ProductDmlCommand::Create {
                kind: "dlm_definition".into(),
                id: "orders".into(),
                document_json: r#"{"dataset_id":"orders","dataset_revision":1}"#.into(),
            },
        ] {
            registry
                .stage_product_command("alice", &begun.transaction_id, command)
                .await
                .unwrap();
        }
        let mut transaction = registry.take("alice", &begun.transaction_id).await.unwrap();
        transaction.bind_request_digest(b"dlm-definition").unwrap();
        assert!(matches!(
            transaction.commit().await.unwrap(),
            CommitOutcome::Committed(_)
        ));
        let alice = Identity {
            principal: "alice".into(),
            display_identity: None,
            role: crate::security::Role::Analyst,
        };
        let bob = Identity {
            principal: "bob".into(),
            ..alice.clone()
        };
        let record = registry
            .read_product(&alice, ProductRecordKind::DlmDefinition, "orders")
            .await
            .unwrap();
        assert_eq!(record.document["dataset_revision"], 1);
        assert!(matches!(
            registry
                .read_product(&bob, ProductRecordKind::DlmDefinition, "orders")
                .await,
            Err(RegistryError::OwnershipForbidden)
        ));
    }

    #[tokio::test]
    async fn dlm_run_binds_definition_and_enforces_terminal_lifecycle() {
        let (registry, _) = registry().await;
        let begun = registry.begin("alice").await.unwrap();
        for command in [
            ProductDmlCommand::Create {
                kind: "dataset".into(), id: "orders".into(),
                document_json: r#"{"name":"Orders"}"#.into(),
            },
            ProductDmlCommand::Create {
                kind: "dlm_definition".into(), id: "orders".into(),
                document_json: r#"{"dataset_id":"orders","dataset_revision":1}"#.into(),
            },
            ProductDmlCommand::Create {
                kind: "dlm_run".into(), id: "run-1".into(),
                document_json: r#"{"definition_id":"orders","definition_revision":1,"status":"building","artifact":null}"#.into(),
            },
        ] {
            registry.stage_product_command("alice", &begun.transaction_id, command).await.unwrap();
        }
        let mut transaction = registry.take("alice", &begun.transaction_id).await.unwrap();
        transaction.bind_request_digest(b"dlm-run-create").unwrap();
        transaction.commit().await.unwrap();

        let bob_session = registry.begin("bob").await.unwrap();
        assert!(matches!(
            registry
                .stage_product_command(
                    "bob",
                    &bob_session.transaction_id,
                    ProductDmlCommand::Update {
                        kind: "dlm_run".into(),
                        id: "run-1".into(),
                        expected_revision: 1,
                        document_json: r#"{"definition_id":"orders","definition_revision":1,"status":"failed","artifact":null}"#.into(),
                    },
                )
                .await,
            Err(RegistryError::OwnershipForbidden)
        ));

        for (id, document) in [
            (
                "run-failed",
                r#"{"definition_id":"orders","definition_revision":1,"status":"failed","artifact":null}"#,
            ),
            (
                "run-stale",
                r#"{"definition_id":"orders","definition_revision":2,"status":"building","artifact":null}"#,
            ),
        ] {
            let invalid = registry.begin("alice").await.unwrap();
            assert!(matches!(
                registry
                    .stage_product_command(
                        "alice",
                        &invalid.transaction_id,
                        ProductDmlCommand::Create {
                            kind: "dlm_run".into(),
                            id: id.into(),
                            document_json: document.into(),
                        },
                    )
                    .await,
                Err(RegistryError::Invalid(_))
            ));
        }

        let begun = registry.begin("alice").await.unwrap();
        registry.stage_product_command("alice", &begun.transaction_id, ProductDmlCommand::Update {
            kind: "dlm_run".into(), id: "run-1".into(), expected_revision: 1,
            document_json: format!(r#"{{"definition_id":"orders","definition_revision":1,"status":"ready","artifact":{{"path":"dlm/orders/run-1/manifest.json","sha256":"{}"}}}}"#, "a".repeat(64)),
        }).await.unwrap();
        let mut transaction = registry.take("alice", &begun.transaction_id).await.unwrap();
        transaction.bind_request_digest(b"dlm-run-ready").unwrap();
        transaction.commit().await.unwrap();

        let begun = registry.begin("alice").await.unwrap();
        let error = registry.stage_product_command("alice", &begun.transaction_id, ProductDmlCommand::Update {
            kind: "dlm_run".into(), id: "run-1".into(), expected_revision: 2,
            document_json: r#"{"definition_id":"orders","definition_revision":1,"status":"failed","artifact":null}"#.into(),
        }).await.unwrap_err();
        assert!(matches!(error, RegistryError::Invalid(_)));

        let alice = Identity {
            principal: "alice".into(),
            display_identity: None,
            role: crate::security::Role::Analyst,
        };
        let bob = Identity {
            principal: "bob".into(),
            ..alice.clone()
        };
        let run = registry
            .read_product(&alice, ProductRecordKind::DlmRun, "run-1")
            .await
            .unwrap();
        assert_eq!(run.document["status"], "ready");
        assert!(matches!(
            registry
                .read_product(&bob, ProductRecordKind::DlmRun, "run-1")
                .await,
            Err(RegistryError::OwnershipForbidden)
        ));
    }

    #[test]
    fn dlm_run_rejects_invalid_artifacts_and_initial_states() {
        for document in [
            r#"{"definition_id":"d","definition_revision":1,"status":"ready","artifact":null}"#,
            r#"{"definition_id":"d","definition_revision":1,"status":"building","artifact":{"path":"x","sha256":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"}}"#,
            r#"{"definition_id":"d","definition_revision":1,"status":"ready","artifact":{"path":"../x","sha256":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"}}"#,
            r#"{"definition_id":"d","definition_revision":1,"status":"ready","artifact":{"path":"x","sha256":"UPPER"}}"#,
        ] {
            assert!(product_document(ProductRecordKind::DlmRun, "run", 1, document).is_err());
        }
    }

    #[tokio::test]
    async fn product_updates_and_deletes_are_record_owner_isolated() {
        let (registry, _) = registry().await;
        let begun = registry.begin("alice").await.unwrap();
        registry
            .stage_product_command(
                "alice",
                &begun.transaction_id,
                ProductDmlCommand::Create {
                    kind: "dashboard".into(),
                    id: "private-dashboard".into(),
                    document_json: r#"{"name":"Private"}"#.into(),
                },
            )
            .await
            .unwrap();
        let mut transaction = registry.take("alice", &begun.transaction_id).await.unwrap();
        transaction.bind_request_digest(b"alice").unwrap();
        transaction.commit().await.unwrap();

        for command in [
            ProductDmlCommand::Update {
                kind: "dashboard".into(),
                id: "private-dashboard".into(),
                expected_revision: 1,
                document_json: r#"{"name":"Stolen"}"#.into(),
            },
            ProductDmlCommand::Delete {
                kind: "dashboard".into(),
                id: "private-dashboard".into(),
                expected_revision: 1,
            },
        ] {
            let begun = registry.begin("bob").await.unwrap();
            assert_eq!(
                registry
                    .stage_product_command("bob", &begun.transaction_id, command)
                    .await
                    .unwrap_err(),
                RegistryError::OwnershipForbidden
            );
            assert_eq!(
                registry
                    .take("bob", &begun.transaction_id)
                    .await
                    .unwrap()
                    .staged_change_count(),
                0
            );
        }
    }

    #[tokio::test]
    async fn sql_endpoint_dispatches_product_dml_and_commit_to_owner_session() {
        let (registry, catalog) = registry().await;
        let begun = registry.begin("alice").await.unwrap();
        let response = execute_sql_request(&registry, "alice", SqlTransactionRequest {
            sql: r#"INSERT INTO kaveon.product.datasets (id, document_json) VALUES ('orders', '{"name":"Orders"}')"#.into(),
            transaction_id: Some(begun.transaction_id.clone()),
            parameters: vec![],
            named_parameters: HashMap::new(),
        }).await;
        assert_eq!(response.status(), StatusCode::OK);
        let response = execute_sql_request(
            &registry,
            "alice",
            SqlTransactionRequest {
                sql: "COMMIT".into(),
                transaction_id: Some(begun.transaction_id),
                parameters: vec![],
                named_parameters: HashMap::new(),
            },
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);
        assert!(
            catalog
                .read_current()
                .await
                .unwrap()
                .product_record(ProductRecordKind::Dataset, "orders")
                .unwrap()
                .is_some()
        );
    }

    #[tokio::test]
    async fn sql_endpoint_expands_bounded_product_id_list_dml_under_one_cas_revision() {
        let (registry, _catalog) = registry().await;
        let begun = registry.begin("alice").await.unwrap();
        for sql in [
            r#"INSERT INTO kaveon.product.datasets (id, document_json) VALUES ('one', '{"name":"One"}'), ('two', '{"name":"Two"}')"#,
            "COMMIT",
        ] {
            let response = execute_sql_request(
                &registry,
                "alice",
                SqlTransactionRequest {
                    sql: sql.into(),
                    transaction_id: Some(begun.transaction_id.clone()),
                    parameters: vec![],
                    named_parameters: HashMap::new(),
                },
            )
            .await;
            assert_eq!(response.status(), StatusCode::OK, "{sql}");
        }

        let begun = registry.begin("alice").await.unwrap();
        let response = execute_sql_request(
            &registry,
            "alice",
            SqlTransactionRequest {
                sql: "UPDATE kaveon.product.datasets SET document_json = '{\"updated\":true}' WHERE id IN ('one', 'two') AND revision = 1".into(),
                transaction_id: Some(begun.transaction_id.clone()),
                parameters: vec![],
                named_parameters: HashMap::new(),
            },
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);
        let response = execute_sql_request(
            &registry,
            "alice",
            SqlTransactionRequest {
                sql: "COMMIT".into(),
                transaction_id: Some(begun.transaction_id),
                parameters: vec![],
                named_parameters: HashMap::new(),
            },
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);

        let identity = Identity {
            principal: "alice".into(),
            display_identity: None,
            role: crate::security::Role::Analyst,
        };
        for id in ["one", "two"] {
            let record = registry
                .read_product(&identity, ProductRecordKind::Dataset, id)
                .await
                .unwrap();
            assert_eq!(record.revision, 2);
            assert_eq!(record.document["updated"], true);
        }
    }

    #[tokio::test]
    async fn failed_product_id_list_dml_does_not_leave_a_partial_write_staged() {
        let (registry, catalog) = registry().await;
        let begun = registry.begin("alice").await.unwrap();
        for sql in [
            r#"INSERT INTO kaveon.product.datasets (id, document_json) VALUES ('one', '{"name":"One"}')"#,
            "COMMIT",
        ] {
            let response = execute_sql_request(
                &registry,
                "alice",
                SqlTransactionRequest {
                    sql: sql.into(),
                    transaction_id: Some(begun.transaction_id.clone()),
                    parameters: vec![],
                    named_parameters: HashMap::new(),
                },
            )
            .await;
            assert_eq!(response.status(), StatusCode::OK, "{sql}");
        }

        let begun = registry.begin("alice").await.unwrap();
        let response = execute_sql_request(
            &registry,
            "alice",
            SqlTransactionRequest {
                sql: "UPDATE kaveon.product.datasets SET document_json = '{\"updated\":true}' WHERE id IN ('one', 'missing') AND revision = 1".into(),
                transaction_id: Some(begun.transaction_id.clone()),
                parameters: vec![],
                named_parameters: HashMap::new(),
            },
        )
        .await;
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);

        // A failed batch must not leave its first command staged in a still
        // live session that can publish it with COMMIT.
        let response = execute_sql_request(
            &registry,
            "alice",
            SqlTransactionRequest {
                sql: "COMMIT".into(),
                transaction_id: Some(begun.transaction_id),
                parameters: vec![],
                named_parameters: HashMap::new(),
            },
        )
        .await;
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);

        let snapshot = catalog.read_current().await.unwrap();
        let record = snapshot
            .product_record(ProductRecordKind::Dataset, "one")
            .unwrap()
            .unwrap();
        assert_eq!(record.revision, 1);
        let document = catalog
            .fetch_product_document_at(&snapshot, ProductRecordKind::Dataset, "one")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(document, br#"{"name":"One"}"#);
    }

    #[tokio::test]
    async fn sql_endpoint_returns_product_identity_metadata_from_staged_dml() {
        let (registry, _catalog) = registry().await;
        let begun = registry.begin("alice").await.unwrap();
        let response = execute_sql_request(
            &registry,
            "alice",
            SqlTransactionRequest {
                sql: r#"INSERT INTO kaveon.product.datasets (id, document_json) VALUES ('orders', '{"name":"Orders"}') RETURNING *"#.into(),
                transaction_id: Some(begun.transaction_id),
                parameters: vec![],
                named_parameters: HashMap::new(),
            },
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);
        let body = axum::body::to_bytes(response.into_body(), 16 * 1024)
            .await
            .unwrap();
        let value: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(value["returning"][0]["id"], "orders");
        assert_eq!(value["returning"][0]["revision"], 1);
        assert_eq!(
            value["returning"][0]["document_json"],
            r#"{"name":"Orders"}"#
        );
    }

    #[tokio::test]
    async fn sql_endpoint_on_conflict_do_nothing_preserves_existing_product() {
        let (registry, catalog) = registry().await;
        let first = registry.begin("alice").await.unwrap();
        let response = execute_sql_request(
            &registry,
            "alice",
            SqlTransactionRequest {
                sql: r#"INSERT INTO kaveon.product.datasets (id, document_json) VALUES ('orders', '{"name":"Orders"}')"#.into(),
                transaction_id: Some(first.transaction_id.clone()),
                parameters: vec![],
                named_parameters: HashMap::new(),
            },
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            execute_sql_request(
                &registry,
                "alice",
                SqlTransactionRequest {
                    sql: "COMMIT".into(),
                    transaction_id: Some(first.transaction_id),
                    parameters: vec![],
                    named_parameters: HashMap::new(),
                },
            )
            .await
            .status(),
            StatusCode::OK
        );

        let retry = registry.begin("alice").await.unwrap();
        let response = execute_sql_request(
            &registry,
            "alice",
            SqlTransactionRequest {
                sql: r#"INSERT INTO kaveon.product.datasets (id, document_json) VALUES ('orders', '{"name":"Changed"}') ON CONFLICT ON CONSTRAINT datasets_pkey DO NOTHING RETURNING id, revision, document_json"#.into(),
                transaction_id: Some(retry.transaction_id.clone()),
                parameters: vec![],
                named_parameters: HashMap::new(),
            },
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);
        let body = axum::body::to_bytes(response.into_body(), 16 * 1024)
            .await
            .unwrap();
        let value: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(value["returning"], serde_json::json!([]));
        assert_eq!(
            execute_sql_request(
                &registry,
                "alice",
                SqlTransactionRequest {
                    sql: "COMMIT".into(),
                    transaction_id: Some(retry.transaction_id),
                    parameters: vec![],
                    named_parameters: HashMap::new(),
                },
            )
            .await
            .status(),
            StatusCode::OK
        );
        let snapshot = catalog.read_current().await.unwrap();
        let record = snapshot
            .product_record(ProductRecordKind::Dataset, "orders")
            .unwrap()
            .unwrap();
        assert_eq!(record.revision, 1);
    }

    #[tokio::test]
    async fn sql_endpoint_update_and_delete_return_identity_and_revision() {
        let (registry, catalog) = registry().await;
        let create = registry.begin("alice").await.unwrap();
        assert_eq!(
            execute_sql_request(
                &registry,
                "alice",
                SqlTransactionRequest {
                    sql: "INSERT INTO product.datasets (id, document_json) VALUES ('orders', '{}')"
                        .into(),
                    transaction_id: Some(create.transaction_id.clone()),
                    parameters: vec![],
                    named_parameters: HashMap::new(),
                },
            )
            .await
            .status(),
            StatusCode::OK
        );
        assert_eq!(
            execute_sql_request(
                &registry,
                "alice",
                SqlTransactionRequest {
                    sql: "COMMIT".into(),
                    transaction_id: Some(create.transaction_id),
                    parameters: vec![],
                    named_parameters: HashMap::new(),
                },
            )
            .await
            .status(),
            StatusCode::OK
        );

        let update = registry.begin("alice").await.unwrap();
        let response = execute_sql_request(
            &registry,
            "alice",
            SqlTransactionRequest {
                sql: "UPDATE product.datasets AS target SET document_json = CAST('{\"name\":\"Orders\"}' AS JSON) WHERE id = 'orders' AND revision = 1 RETURNING *".into(),
                transaction_id: Some(update.transaction_id.clone()),
                parameters: vec![],
                named_parameters: HashMap::new(),
            },
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);
        let body = axum::body::to_bytes(response.into_body(), 16 * 1024)
            .await
            .unwrap();
        let value: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(value["returning"][0]["id"], "orders");
        assert_eq!(value["returning"][0]["revision"], 2);
        assert_eq!(
            value["returning"][0]["document_json"],
            r#"{"name":"Orders"}"#
        );
        assert_eq!(
            execute_sql_request(
                &registry,
                "alice",
                SqlTransactionRequest {
                    sql: "COMMIT".into(),
                    transaction_id: Some(update.transaction_id),
                    parameters: vec![],
                    named_parameters: HashMap::new(),
                },
            )
            .await
            .status(),
            StatusCode::OK
        );

        let delete = registry.begin("alice").await.unwrap();
        let response = execute_sql_request(
            &registry,
            "alice",
            SqlTransactionRequest {
                sql: "DELETE FROM product.datasets AS target OUTPUT deleted.id, deleted.revision, deleted.document_json WHERE id = 'orders' AND revision = 2".into(),
                transaction_id: Some(delete.transaction_id.clone()),
                parameters: vec![],
                named_parameters: HashMap::new(),
            },
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);
        let body = axum::body::to_bytes(response.into_body(), 16 * 1024)
            .await
            .unwrap();
        let value: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(value["returning"][0]["id"], "orders");
        assert_eq!(value["returning"][0]["revision"], 2);
        assert_eq!(
            value["returning"][0]["document_json"],
            r#"{"name":"Orders"}"#
        );
        assert_eq!(
            execute_sql_request(
                &registry,
                "alice",
                SqlTransactionRequest {
                    sql: "COMMIT".into(),
                    transaction_id: Some(delete.transaction_id),
                    parameters: vec![],
                    named_parameters: HashMap::new(),
                },
            )
            .await
            .status(),
            StatusCode::OK
        );
        assert!(
            catalog
                .read_current()
                .await
                .unwrap()
                .product_record(ProductRecordKind::Dataset, "orders")
                .unwrap()
                .is_none()
        );
    }

    #[tokio::test]
    async fn sql_endpoint_bounded_update_from_copies_source_document_and_revises_target() {
        let (registry, catalog) = registry().await;
        let seed = registry.begin("alice").await.unwrap();
        assert_eq!(
            execute_sql_request(
                &registry,
                "alice",
                SqlTransactionRequest {
                    sql: r#"INSERT INTO product.datasets (id, document_json) VALUES ('target', '{"name":"before"}'), ('source', '{"name":"after"}')"#.into(),
                    transaction_id: Some(seed.transaction_id.clone()),
                    parameters: vec![],
                    named_parameters: HashMap::new(),
                },
            )
            .await
            .status(),
            StatusCode::OK
        );
        assert_eq!(
            execute_sql_request(
                &registry,
                "alice",
                SqlTransactionRequest {
                    sql: "COMMIT".into(),
                    transaction_id: Some(seed.transaction_id),
                    parameters: vec![],
                    named_parameters: HashMap::new(),
                },
            )
            .await
            .status(),
            StatusCode::OK
        );

        let update = registry.begin("alice").await.unwrap();
        let response = execute_sql_request(
            &registry,
            "alice",
            SqlTransactionRequest {
                sql: "UPDATE product.datasets AS target SET target.document_json = CAST(source.document_json AS JSON) FROM product.datasets AS source WHERE target.id = 'target' AND target.revision = 1 AND source.id = 'source' RETURNING id, revision, document_json".into(),
                transaction_id: Some(update.transaction_id.clone()),
                parameters: vec![],
                named_parameters: HashMap::new(),
            },
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);
        let body = axum::body::to_bytes(response.into_body(), 16 * 1024)
            .await
            .unwrap();
        let value: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(
            value["returning"],
            serde_json::json!([{
                "id": "target",
                "revision": 2,
                "document_json": "{\"name\":\"after\"}"
            }])
        );
        assert_eq!(
            execute_sql_request(
                &registry,
                "alice",
                SqlTransactionRequest {
                    sql: "COMMIT".into(),
                    transaction_id: Some(update.transaction_id),
                    parameters: vec![],
                    named_parameters: HashMap::new(),
                },
            )
            .await
            .status(),
            StatusCode::OK
        );
        let snapshot = catalog.read_current().await.unwrap();
        let target = snapshot
            .product_record(ProductRecordKind::Dataset, "target")
            .unwrap()
            .unwrap();
        assert_eq!(target.revision, 2);
        assert_eq!(
            catalog
                .fetch_product_document_at(&snapshot, ProductRecordKind::Dataset, "target")
                .await
                .unwrap()
                .unwrap(),
            b"{\"name\":\"after\"}"
        );
    }

    #[tokio::test]
    async fn sql_endpoint_bounded_update_from_batch_is_atomic_and_returns_each_target() {
        let (registry, catalog) = registry().await;
        let seed = registry.begin("alice").await.unwrap();
        assert_eq!(
            execute_sql_request(
                &registry,
                "alice",
                SqlTransactionRequest {
                    sql: r#"INSERT INTO product.datasets (id, document_json) VALUES ('target-1', '{"name":"before-1"}'), ('target-2', '{"name":"before-2"}'), ('source', '{"name":"after"}')"#.into(),
                    transaction_id: Some(seed.transaction_id.clone()),
                    parameters: vec![],
                    named_parameters: HashMap::new(),
                },
            )
            .await
            .status(),
            StatusCode::OK
        );
        assert_eq!(
            execute_sql_request(
                &registry,
                "alice",
                SqlTransactionRequest {
                    sql: "COMMIT".into(),
                    transaction_id: Some(seed.transaction_id),
                    parameters: vec![],
                    named_parameters: HashMap::new(),
                },
            )
            .await
            .status(),
            StatusCode::OK
        );

        let update = registry.begin("alice").await.unwrap();
        let response = execute_sql_request(
            &registry,
            "alice",
            SqlTransactionRequest {
                sql: "UPDATE product.datasets AS target SET target.document_json = source.document_json FROM product.datasets AS source WHERE target.id IN ('target-1', 'target-2') AND target.revision = 1 AND source.id = 'source' RETURNING id, revision, document_json".into(),
                transaction_id: Some(update.transaction_id.clone()),
                parameters: vec![],
                named_parameters: HashMap::new(),
            },
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);
        let body = axum::body::to_bytes(response.into_body(), 16 * 1024)
            .await
            .unwrap();
        let value: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(
            value["returning"],
            serde_json::json!([
                {"id": "target-1", "revision": 2, "document_json": "{\"name\":\"after\"}"},
                {"id": "target-2", "revision": 2, "document_json": "{\"name\":\"after\"}"}
            ])
        );
        assert_eq!(
            execute_sql_request(
                &registry,
                "alice",
                SqlTransactionRequest {
                    sql: "COMMIT".into(),
                    transaction_id: Some(update.transaction_id),
                    parameters: vec![],
                    named_parameters: HashMap::new(),
                },
            )
            .await
            .status(),
            StatusCode::OK
        );

        let snapshot = catalog.read_current().await.unwrap();
        for id in ["target-1", "target-2"] {
            let target = snapshot
                .product_record(ProductRecordKind::Dataset, id)
                .unwrap()
                .unwrap();
            assert_eq!(target.revision, 2);
            assert_eq!(
                catalog
                    .fetch_product_document_at(&snapshot, ProductRecordKind::Dataset, id)
                    .await
                    .unwrap()
                    .unwrap(),
                b"{\"name\":\"after\"}"
            );
        }
    }

    #[tokio::test]
    async fn sql_endpoint_bounded_delete_using_batch_is_atomic_and_returns_each_target() {
        let (registry, catalog) = registry().await;
        let seed = registry.begin("alice").await.unwrap();
        assert_eq!(
            execute_sql_request(
                &registry,
                "alice",
                SqlTransactionRequest {
                    sql: r#"INSERT INTO product.datasets (id, document_json) VALUES ('target-1', '{"name":"one"}'), ('target-2', '{"name":"two"}'), ('source', '{"name":"source"}')"#.into(),
                    transaction_id: Some(seed.transaction_id.clone()),
                    parameters: vec![],
                    named_parameters: HashMap::new(),
                },
            )
            .await
            .status(),
            StatusCode::OK
        );
        assert_eq!(
            execute_sql_request(
                &registry,
                "alice",
                SqlTransactionRequest {
                    sql: "COMMIT".into(),
                    transaction_id: Some(seed.transaction_id),
                    parameters: vec![],
                    named_parameters: HashMap::new(),
                },
            )
            .await
            .status(),
            StatusCode::OK
        );

        let delete = registry.begin("alice").await.unwrap();
        let response = execute_sql_request(
            &registry,
            "alice",
            SqlTransactionRequest {
                sql: "DELETE FROM product.datasets AS target USING product.datasets AS source WHERE target.id IN ('target-1', 'target-2') AND target.revision = 1 AND source.id = 'source' RETURNING id, revision".into(),
                transaction_id: Some(delete.transaction_id.clone()),
                parameters: vec![],
                named_parameters: HashMap::new(),
            },
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);
        let body = axum::body::to_bytes(response.into_body(), 16 * 1024)
            .await
            .unwrap();
        let value: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(
            value["returning"],
            serde_json::json!([
                {"id": "target-1", "revision": 1},
                {"id": "target-2", "revision": 1}
            ])
        );
        assert_eq!(
            execute_sql_request(
                &registry,
                "alice",
                SqlTransactionRequest {
                    sql: "COMMIT".into(),
                    transaction_id: Some(delete.transaction_id),
                    parameters: vec![],
                    named_parameters: HashMap::new(),
                },
            )
            .await
            .status(),
            StatusCode::OK
        );
        let snapshot = catalog.read_current().await.unwrap();
        for id in ["target-1", "target-2"] {
            assert!(
                snapshot
                    .product_record(ProductRecordKind::Dataset, id)
                    .unwrap()
                    .is_none()
            );
        }
        assert!(
            snapshot
                .product_record(ProductRecordKind::Dataset, "source")
                .unwrap()
                .is_some()
        );
    }

    #[tokio::test]
    async fn sql_endpoint_on_conflict_do_update_revises_owned_product() {
        let (registry, catalog) = registry().await;
        let first = registry.begin("alice").await.unwrap();
        assert_eq!(
            execute_sql_request(
                &registry,
                "alice",
                SqlTransactionRequest {
                    sql: r#"INSERT INTO kaveon.product.datasets (id, document_json) VALUES ('orders', '{"name":"Orders"}')"#.into(),
                    transaction_id: Some(first.transaction_id.clone()),
                    parameters: vec![],
                    named_parameters: HashMap::new(),
                },
            )
            .await
            .status(),
            StatusCode::OK
        );
        assert_eq!(
            execute_sql_request(
                &registry,
                "alice",
                SqlTransactionRequest {
                    sql: "COMMIT".into(),
                    transaction_id: Some(first.transaction_id),
                    parameters: vec![],
                    named_parameters: HashMap::new(),
                },
            )
            .await
            .status(),
            StatusCode::OK
        );

        let upsert = registry.begin("alice").await.unwrap();
        let response = execute_sql_request(
            &registry,
            "alice",
            SqlTransactionRequest {
                sql: r#"INSERT INTO kaveon.product.datasets (id, document_json) VALUES ('orders', '{"name":"Inserted"}') ON CONFLICT ON CONSTRAINT datasets_pkey DO UPDATE SET document_json = CAST('{"name":"Conflict override"}' AS JSON) WHERE id = excluded.id RETURNING id, revision, document_json"#.into(),
                transaction_id: Some(upsert.transaction_id.clone()),
                parameters: vec![],
                named_parameters: HashMap::new(),
            },
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);
        let body = axum::body::to_bytes(response.into_body(), 16 * 1024)
            .await
            .unwrap();
        let value: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(value["returning"][0]["id"], "orders");
        assert_eq!(value["returning"][0]["revision"], 2);
        assert_eq!(
            value["returning"][0]["document_json"],
            "{\"name\":\"Conflict override\"}"
        );
        assert_eq!(
            execute_sql_request(
                &registry,
                "alice",
                SqlTransactionRequest {
                    sql: "COMMIT".into(),
                    transaction_id: Some(upsert.transaction_id),
                    parameters: vec![],
                    named_parameters: HashMap::new(),
                },
            )
            .await
            .status(),
            StatusCode::OK
        );
        let snapshot = catalog.read_current().await.unwrap();
        let record = snapshot
            .product_record(ProductRecordKind::Dataset, "orders")
            .unwrap()
            .unwrap();
        assert_eq!(record.revision, 2);
    }

    #[tokio::test]
    async fn sql_endpoint_bounded_merge_batch_updates_and_inserts_atomically() {
        let (registry, catalog) = registry().await;
        let first = registry.begin("alice").await.unwrap();
        let response = execute_sql_request(
            &registry,
            "alice",
            SqlTransactionRequest {
                sql: r#"INSERT INTO kaveon.product.datasets (id, document_json) VALUES ('one', '{}'), ('two', '{}')"#.into(),
                transaction_id: Some(first.transaction_id.clone()),
                parameters: vec![],
                named_parameters: HashMap::new(),
            },
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            execute_sql_request(
                &registry,
                "alice",
                SqlTransactionRequest {
                    sql: "COMMIT".into(),
                    transaction_id: Some(first.transaction_id),
                    parameters: vec![],
                    named_parameters: HashMap::new(),
                },
            )
            .await
            .status(),
            StatusCode::OK
        );

        let merge = registry.begin("alice").await.unwrap();
        let response = execute_sql_request(
            &registry,
            "alice",
            SqlTransactionRequest {
                sql: r#"MERGE INTO product.datasets AS target USING (VALUES ('one', '{"n":1}'), ('two', '{"n":2}'), ('three', '{"n":3}')) AS source (id, document_json) ON target.id = source.id WHEN MATCHED AND target.id = source.id THEN UPDATE SET document_json = CAST(source.document_json AS JSON) WHEN NOT MATCHED THEN INSERT (id, document_json) VALUES (source.id, CAST(source.document_json AS JSON))"#.into(),
                transaction_id: Some(merge.transaction_id.clone()),
                parameters: vec![],
                named_parameters: HashMap::new(),
            },
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            execute_sql_request(
                &registry,
                "alice",
                SqlTransactionRequest {
                    sql: "COMMIT".into(),
                    transaction_id: Some(merge.transaction_id),
                    parameters: vec![],
                    named_parameters: HashMap::new(),
                },
            )
            .await
            .status(),
            StatusCode::OK
        );
        let snapshot = catalog.read_current().await.unwrap();
        for (id, revision) in [("one", 2), ("two", 2), ("three", 1)] {
            assert_eq!(
                snapshot
                    .product_record(ProductRecordKind::Dataset, id)
                    .unwrap()
                    .unwrap()
                    .revision,
                revision
            );
        }
    }

    #[tokio::test]
    async fn sql_endpoint_bounded_merge_accepts_inserted_output() {
        let (registry, _catalog) = registry().await;
        let seed = registry.begin("alice").await.unwrap();
        assert_eq!(
            execute_sql_request(
                &registry,
                "alice",
                SqlTransactionRequest {
                    sql:
                        "INSERT INTO product.datasets (id, document_json) VALUES ('existing', '{}')"
                            .into(),
                    transaction_id: Some(seed.transaction_id.clone()),
                    parameters: vec![],
                    named_parameters: HashMap::new(),
                },
            )
            .await
            .status(),
            StatusCode::OK
        );
        assert_eq!(
            execute_sql_request(
                &registry,
                "alice",
                SqlTransactionRequest {
                    sql: "COMMIT".into(),
                    transaction_id: Some(seed.transaction_id),
                    parameters: vec![],
                    named_parameters: HashMap::new(),
                },
            )
            .await
            .status(),
            StatusCode::OK
        );

        let merge = registry.begin("alice").await.unwrap();
        let response = execute_sql_request(
            &registry,
            "alice",
            SqlTransactionRequest {
                sql: r#"MERGE INTO product.datasets AS target USING (VALUES ('existing', '{"updated":true}'), ('new', '{"created":true}')) AS source (id, document_json) ON target.id = source.id WHEN MATCHED THEN UPDATE SET document_json = source.document_json WHEN NOT MATCHED THEN INSERT (id, document_json) VALUES (source.id, source.document_json) OUTPUT inserted.*"#.into(),
                transaction_id: Some(merge.transaction_id),
                parameters: vec![],
                named_parameters: HashMap::new(),
            },
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);
        let body = axum::body::to_bytes(response.into_body(), 16 * 1024)
            .await
            .unwrap();
        let value: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(value["returning"][0]["id"], "existing");
        assert_eq!(value["returning"][0]["revision"], 2);
        assert_eq!(value["returning"][1]["id"], "new");
        assert_eq!(value["returning"][1]["revision"], 1);
    }

    #[tokio::test]
    async fn sql_endpoint_bounded_merge_matched_update_accepts_inserted_output() {
        let (registry, _catalog) = registry().await;
        let seed = registry.begin("alice").await.unwrap();
        assert_eq!(
            execute_sql_request(
                &registry,
                "alice",
                SqlTransactionRequest {
                    sql: "INSERT INTO product.datasets (id, document_json) VALUES ('update-me', '{}')".into(),
                    transaction_id: Some(seed.transaction_id.clone()),
                    parameters: vec![],
                    named_parameters: HashMap::new(),
                },
            )
            .await
            .status(),
            StatusCode::OK
        );
        assert_eq!(
            execute_sql_request(
                &registry,
                "alice",
                SqlTransactionRequest {
                    sql: "COMMIT".into(),
                    transaction_id: Some(seed.transaction_id),
                    parameters: vec![],
                    named_parameters: HashMap::new(),
                },
            )
            .await
            .status(),
            StatusCode::OK
        );

        let merge = registry.begin("alice").await.unwrap();
        let response = execute_sql_request(
            &registry,
            "alice",
            SqlTransactionRequest {
                sql: r#"MERGE INTO product.datasets AS target USING (VALUES ('update-me', '{"changed":true}'), ('missing', '{"ignored":true}')) AS source (id, document_json) ON target.id = source.id WHEN MATCHED THEN UPDATE SET document_json = source.document_json OUTPUT inserted.id, inserted.revision, inserted.document_json"#.into(),
                transaction_id: Some(merge.transaction_id),
                parameters: vec![],
                named_parameters: HashMap::new(),
            },
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);
        let body = axum::body::to_bytes(response.into_body(), 16 * 1024)
            .await
            .unwrap();
        let value: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(value["returning"].as_array().map(Vec::len), Some(1));
        assert_eq!(value["returning"][0]["id"], "update-me");
        assert_eq!(value["returning"][0]["revision"], 2);
        assert_eq!(
            value["returning"][0]["document_json"],
            r#"{"changed":true}"#
        );
    }

    #[tokio::test]
    async fn sql_endpoint_bounded_merge_matched_delete_accepts_deleted_output() {
        let (registry, _catalog) = registry().await;
        let seed = registry.begin("alice").await.unwrap();
        assert_eq!(
            execute_sql_request(
                &registry,
                "alice",
                SqlTransactionRequest {
                    sql: "INSERT INTO product.datasets (id, document_json) VALUES ('remove-me', '{}')".into(),
                    transaction_id: Some(seed.transaction_id.clone()),
                    parameters: vec![],
                    named_parameters: HashMap::new(),
                },
            )
            .await
            .status(),
            StatusCode::OK
        );
        assert_eq!(
            execute_sql_request(
                &registry,
                "alice",
                SqlTransactionRequest {
                    sql: "COMMIT".into(),
                    transaction_id: Some(seed.transaction_id),
                    parameters: vec![],
                    named_parameters: HashMap::new(),
                },
            )
            .await
            .status(),
            StatusCode::OK
        );

        let merge = registry.begin("alice").await.unwrap();
        let response = execute_sql_request(
            &registry,
            "alice",
            SqlTransactionRequest {
                sql: r#"MERGE INTO product.datasets AS target USING (VALUES ('remove-me', '{}'), ('missing', '{}')) AS source (id, document_json) ON target.id = source.id WHEN MATCHED THEN DELETE OUTPUT deleted.id, deleted.revision, deleted.document_json"#.into(),
                transaction_id: Some(merge.transaction_id),
                parameters: vec![],
                named_parameters: HashMap::new(),
            },
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);
        let body = axum::body::to_bytes(response.into_body(), 16 * 1024)
            .await
            .unwrap();
        let value: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(value["returning"].as_array().map(Vec::len), Some(1));
        assert_eq!(value["returning"][0]["id"], "remove-me");
        assert_eq!(value["returning"][0]["revision"], 1);
        assert_eq!(value["returning"][0]["document_json"], "{}");
    }

    #[tokio::test]
    async fn sql_endpoint_bounded_merge_unmatched_insert_accepts_inserted_output() {
        let (registry, _catalog) = registry().await;
        let seed = registry.begin("alice").await.unwrap();
        assert_eq!(
            execute_sql_request(
                &registry,
                "alice",
                SqlTransactionRequest {
                    sql: "INSERT INTO product.datasets (id, document_json) VALUES ('already-there', '{}')".into(),
                    transaction_id: Some(seed.transaction_id.clone()),
                    parameters: vec![],
                    named_parameters: HashMap::new(),
                },
            )
            .await
            .status(),
            StatusCode::OK
        );
        assert_eq!(
            execute_sql_request(
                &registry,
                "alice",
                SqlTransactionRequest {
                    sql: "COMMIT".into(),
                    transaction_id: Some(seed.transaction_id),
                    parameters: vec![],
                    named_parameters: HashMap::new(),
                },
            )
            .await
            .status(),
            StatusCode::OK
        );

        let merge = registry.begin("alice").await.unwrap();
        let response = execute_sql_request(
            &registry,
            "alice",
            SqlTransactionRequest {
                sql: r#"MERGE INTO product.datasets AS target USING (VALUES ('already-there', '{}'), ('new-row', '{"created":true}')) AS source (id, document_json) ON target.id = source.id WHEN NOT MATCHED THEN INSERT (id, document_json) VALUES (source.id, source.document_json) OUTPUT inserted.id, inserted.revision"#.into(),
                transaction_id: Some(merge.transaction_id),
                parameters: vec![],
                named_parameters: HashMap::new(),
            },
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);
        let body = axum::body::to_bytes(response.into_body(), 16 * 1024)
            .await
            .unwrap();
        let value: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(value["returning"].as_array().map(Vec::len), Some(1));
        assert_eq!(value["returning"][0]["id"], "new-row");
        assert_eq!(value["returning"][0]["revision"], 1);
    }

    #[tokio::test]
    async fn sql_endpoint_bounded_merge_delete_or_insert_output_is_action_aware() {
        let (registry, _catalog) = registry().await;
        let seed = registry.begin("alice").await.unwrap();
        assert_eq!(
            execute_sql_request(
                &registry,
                "alice",
                SqlTransactionRequest {
                    sql: "INSERT INTO product.datasets (id, document_json) VALUES ('delete-me', '{}'), ('second-delete', '{}')".into(),
                    transaction_id: Some(seed.transaction_id.clone()),
                    parameters: vec![],
                    named_parameters: HashMap::new(),
                },
            )
            .await
            .status(),
            StatusCode::OK
        );
        assert_eq!(
            execute_sql_request(
                &registry,
                "alice",
                SqlTransactionRequest {
                    sql: "COMMIT".into(),
                    transaction_id: Some(seed.transaction_id),
                    parameters: vec![],
                    named_parameters: HashMap::new(),
                },
            )
            .await
            .status(),
            StatusCode::OK
        );

        let inserted = registry.begin("alice").await.unwrap();
        let response = execute_sql_request(
            &registry,
            "alice",
            SqlTransactionRequest {
                sql: r#"MERGE INTO product.datasets AS target USING (VALUES ('delete-me', '{}'), ('new-row', '{"created":true}')) AS source (id, document_json) ON target.id = source.id WHEN MATCHED THEN DELETE WHEN NOT MATCHED THEN INSERT (id, document_json) VALUES (source.id, source.document_json) OUTPUT inserted.id, inserted.revision"#.into(),
                transaction_id: Some(inserted.transaction_id.clone()),
                parameters: vec![],
                named_parameters: HashMap::new(),
            },
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);
        let body = axum::body::to_bytes(response.into_body(), 16 * 1024)
            .await
            .unwrap();
        let value: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(value["returning"].as_array().map(Vec::len), Some(1));
        assert_eq!(value["returning"][0]["id"], "new-row");
        assert_eq!(value["returning"][0]["revision"], 1);
        assert_eq!(
            execute_sql_request(
                &registry,
                "alice",
                SqlTransactionRequest {
                    sql: "COMMIT".into(),
                    transaction_id: Some(inserted.transaction_id),
                    parameters: vec![],
                    named_parameters: HashMap::new(),
                },
            )
            .await
            .status(),
            StatusCode::OK
        );

        let deleted = registry.begin("alice").await.unwrap();
        let response = execute_sql_request(
            &registry,
            "alice",
            SqlTransactionRequest {
                sql: r#"MERGE INTO product.datasets AS target USING (VALUES ('second-delete', '{}'), ('another-row', '{"created":true}')) AS source (id, document_json) ON target.id = source.id WHEN MATCHED THEN DELETE WHEN NOT MATCHED THEN INSERT (id, document_json) VALUES (source.id, source.document_json) OUTPUT deleted.id, deleted.revision"#.into(),
                transaction_id: Some(deleted.transaction_id),
                parameters: vec![],
                named_parameters: HashMap::new(),
            },
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);
        let body = axum::body::to_bytes(response.into_body(), 16 * 1024)
            .await
            .unwrap();
        let value: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(value["returning"].as_array().map(Vec::len), Some(1));
        assert_eq!(value["returning"][0]["id"], "second-delete");
        assert_eq!(value["returning"][0]["revision"], 1);
    }

    #[tokio::test]
    async fn sql_endpoint_merge_by_source_output_returns_deleted_identity() {
        let (registry, _catalog) = registry().await;
        let seed = registry.begin("alice").await.unwrap();
        assert_eq!(
            execute_sql_request(
                &registry,
                "alice",
                SqlTransactionRequest {
                    sql: "INSERT INTO product.datasets (id, document_json) VALUES ('remove-me', '{}')".into(),
                    transaction_id: Some(seed.transaction_id.clone()),
                    parameters: vec![],
                    named_parameters: HashMap::new(),
                },
            )
            .await
            .status(),
            StatusCode::OK
        );
        assert_eq!(
            execute_sql_request(
                &registry,
                "alice",
                SqlTransactionRequest {
                    sql: "COMMIT".into(),
                    transaction_id: Some(seed.transaction_id),
                    parameters: vec![],
                    named_parameters: HashMap::new(),
                },
            )
            .await
            .status(),
            StatusCode::OK
        );

        let transaction = registry.begin("alice").await.unwrap();
        let response = execute_sql_request(
            &registry,
            "alice",
            SqlTransactionRequest {
                sql: r#"MERGE INTO product.datasets AS target USING (VALUES ('keep-me', '{}')) AS source (id, document_json) ON target.id = source.id WHEN MATCHED THEN UPDATE SET document_json = source.document_json WHEN NOT MATCHED THEN INSERT (id, document_json) VALUES (source.id, source.document_json) WHEN NOT MATCHED BY SOURCE THEN DELETE OUTPUT deleted.id, deleted.revision, deleted.document_json"#.into(),
                transaction_id: Some(transaction.transaction_id.clone()),
                parameters: vec![],
                named_parameters: HashMap::new(),
            },
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);
        let body = axum::body::to_bytes(response.into_body(), 16 * 1024)
            .await
            .unwrap();
        let value: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(value["returning"].as_array().map(Vec::len), Some(1));
        assert_eq!(value["returning"][0]["id"], "remove-me");
        assert_eq!(value["returning"][0]["revision"], 1);
        assert_eq!(value["returning"][0]["document_json"], "{}");

        let response = execute_sql_request(
            &registry,
            "alice",
            SqlTransactionRequest {
                sql: "COMMIT".into(),
                transaction_id: Some(transaction.transaction_id),
                parameters: vec![],
                named_parameters: HashMap::new(),
            },
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);

        let transaction = registry.begin("alice").await.unwrap();
        let response = execute_sql_request(
            &registry,
            "alice",
            SqlTransactionRequest {
                sql: r#"MERGE INTO product.datasets AS target USING (VALUES ('new-row', '{"created":true}')) AS source (id, document_json) ON target.id = source.id WHEN MATCHED THEN UPDATE SET document_json = source.document_json WHEN NOT MATCHED THEN INSERT (id, document_json) VALUES (source.id, source.document_json) WHEN NOT MATCHED BY SOURCE THEN DELETE OUTPUT inserted.id, inserted.revision, inserted.document_json"#.into(),
                transaction_id: Some(transaction.transaction_id.clone()),
                parameters: vec![],
                named_parameters: HashMap::new(),
            },
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);
        let body = axum::body::to_bytes(response.into_body(), 16 * 1024)
            .await
            .unwrap();
        let value: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(value["returning"].as_array().map(Vec::len), Some(1));
        assert_eq!(value["returning"][0]["id"], "new-row");
        assert_eq!(value["returning"][0]["revision"], 1);
        assert_eq!(
            value["returning"][0]["document_json"],
            r#"{"created":true}"#
        );

        let response = execute_sql_request(
            &registry,
            "alice",
            SqlTransactionRequest {
                sql: "COMMIT".into(),
                transaction_id: Some(transaction.transaction_id),
                parameters: vec![],
                named_parameters: HashMap::new(),
            },
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);

        let transaction = registry.begin("alice").await.unwrap();
        let response = execute_sql_request(
            &registry,
            "alice",
            SqlTransactionRequest {
                sql: r#"MERGE INTO product.datasets AS target USING (VALUES ('created', '{"created":true}')) AS source (id, document_json) ON target.id = source.id WHEN MATCHED THEN UPDATE SET document_json = source.document_json WHEN NOT MATCHED THEN INSERT (id, document_json) VALUES (source.id, source.document_json) WHEN NOT MATCHED BY SOURCE THEN DELETE OUTPUT $action"#.into(),
                transaction_id: Some(transaction.transaction_id.clone()),
                parameters: vec![],
                named_parameters: HashMap::new(),
            },
        )
        .await;
        let status = response.status();
        let body = axum::body::to_bytes(response.into_body(), 16 * 1024)
            .await
            .unwrap();
        assert_eq!(status, StatusCode::OK, "{}", String::from_utf8_lossy(&body));
        let value: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(
            value["returning"],
            serde_json::json!([
                {"action": "INSERT"},
                {"action": "DELETE"}
            ])
        );
        assert_eq!(
            execute_sql_request(
                &registry,
                "alice",
                SqlTransactionRequest {
                    sql: "COMMIT".into(),
                    transaction_id: Some(transaction.transaction_id),
                    parameters: vec![],
                    named_parameters: HashMap::new(),
                },
            )
            .await
            .status(),
            StatusCode::OK
        );
    }

    #[tokio::test]
    async fn sql_endpoint_bounded_merge_matched_delete_is_atomic_and_unmatched_is_noop() {
        let (registry, catalog) = registry().await;
        let first = registry.begin("alice").await.unwrap();
        assert_eq!(
            execute_sql_request(
                &registry,
                "alice",
                SqlTransactionRequest {
                    sql: "INSERT INTO product.datasets (id, document_json) VALUES ('remove-me', '{}'), ('keep-me', '{}')".into(),
                    transaction_id: Some(first.transaction_id.clone()),
                    parameters: vec![],
                    named_parameters: HashMap::new(),
                },
            )
            .await
            .status(),
            StatusCode::OK
        );
        assert_eq!(
            execute_sql_request(
                &registry,
                "alice",
                SqlTransactionRequest {
                    sql: "COMMIT".into(),
                    transaction_id: Some(first.transaction_id),
                    parameters: vec![],
                    named_parameters: HashMap::new(),
                },
            )
            .await
            .status(),
            StatusCode::OK
        );

        let merge = registry.begin("alice").await.unwrap();
        assert_eq!(
            execute_sql_request(
                &registry,
                "alice",
                SqlTransactionRequest {
                    sql: "MERGE INTO product.datasets AS target USING (VALUES ('remove-me', '{}'), ('keep-me', '{}'), ('missing', '{}')) AS source (id, document_json) ON target.id = source.id WHEN MATCHED AND source.id IN ('remove-me') THEN DELETE".into(),
                    transaction_id: Some(merge.transaction_id.clone()),
                    parameters: vec![],
                    named_parameters: HashMap::new(),
                },
            )
            .await
            .status(),
            StatusCode::OK
        );
        assert_eq!(
            execute_sql_request(
                &registry,
                "alice",
                SqlTransactionRequest {
                    sql: "COMMIT".into(),
                    transaction_id: Some(merge.transaction_id),
                    parameters: vec![],
                    named_parameters: HashMap::new(),
                },
            )
            .await
            .status(),
            StatusCode::OK
        );
        let snapshot = catalog.read_current().await.unwrap();
        assert!(
            snapshot
                .product_record(ProductRecordKind::Dataset, "remove-me")
                .unwrap()
                .is_none()
        );
        assert!(
            snapshot
                .product_record(ProductRecordKind::Dataset, "keep-me")
                .unwrap()
                .is_some()
        );
        assert!(
            snapshot
                .product_record(ProductRecordKind::Dataset, "missing")
                .unwrap()
                .is_none()
        );
    }

    #[tokio::test]
    async fn sql_endpoint_bounded_merge_not_matched_by_source_deletes_owned_rows() {
        let (registry, catalog) = registry().await;
        let first = registry.begin("alice").await.unwrap();
        assert_eq!(
            execute_sql_request(
                &registry,
                "alice",
                SqlTransactionRequest {
                    sql: "INSERT INTO product.datasets (id, document_json) VALUES ('keep-me', '{}'), ('remove-me', '{}'), ('also-remove', '{}')".into(),
                    transaction_id: Some(first.transaction_id.clone()),
                    parameters: vec![],
                    named_parameters: HashMap::new(),
                },
            )
            .await
            .status(),
            StatusCode::OK
        );
        assert_eq!(
            execute_sql_request(
                &registry,
                "alice",
                SqlTransactionRequest {
                    sql: "COMMIT".into(),
                    transaction_id: Some(first.transaction_id),
                    parameters: vec![],
                    named_parameters: HashMap::new(),
                },
            )
            .await
            .status(),
            StatusCode::OK
        );

        let merge = registry.begin("alice").await.unwrap();
        assert_eq!(
            execute_sql_request(
                &registry,
                "alice",
                SqlTransactionRequest {
                    sql: r#"MERGE INTO product.datasets AS target USING (VALUES ('keep-me', '{"kept":true}')) AS source (id, document_json) ON target.id = source.id WHEN MATCHED THEN UPDATE SET document_json = source.document_json WHEN NOT MATCHED THEN INSERT (id, document_json) VALUES (source.id, source.document_json) WHEN NOT MATCHED BY SOURCE AND target.id IN ('remove-me') THEN DELETE"#.into(),
                    transaction_id: Some(merge.transaction_id.clone()),
                    parameters: vec![],
                    named_parameters: HashMap::new(),
                },
            )
            .await
            .status(),
            StatusCode::OK
        );
        assert_eq!(
            execute_sql_request(
                &registry,
                "alice",
                SqlTransactionRequest {
                    sql: "COMMIT".into(),
                    transaction_id: Some(merge.transaction_id),
                    parameters: vec![],
                    named_parameters: HashMap::new(),
                },
            )
            .await
            .status(),
            StatusCode::OK
        );
        let snapshot = catalog.read_current().await.unwrap();
        let keep = snapshot
            .product_record(ProductRecordKind::Dataset, "keep-me")
            .unwrap()
            .unwrap();
        assert_eq!(keep.revision, 2);
        assert!(
            snapshot
                .product_record(ProductRecordKind::Dataset, "remove-me")
                .unwrap()
                .is_none()
        );
        assert!(
            snapshot
                .product_record(ProductRecordKind::Dataset, "also-remove")
                .unwrap()
                .is_some()
        );
    }

    #[tokio::test]
    async fn sql_endpoint_bounded_merge_not_matched_by_source_updates_owned_rows() {
        let (registry, catalog) = registry().await;
        let first = registry.begin("alice").await.unwrap();
        assert_eq!(
            execute_sql_request(
                &registry,
                "alice",
                SqlTransactionRequest {
                    sql: "INSERT INTO product.datasets (id, document_json) VALUES ('keep-me', '{}'), ('archive-me', '{}'), ('leave-me', '{}')".into(),
                    transaction_id: Some(first.transaction_id.clone()),
                    parameters: vec![],
                    named_parameters: HashMap::new(),
                },
            )
            .await
            .status(),
            StatusCode::OK
        );
        assert_eq!(
            execute_sql_request(
                &registry,
                "alice",
                SqlTransactionRequest {
                    sql: "COMMIT".into(),
                    transaction_id: Some(first.transaction_id),
                    parameters: vec![],
                    named_parameters: HashMap::new(),
                },
            )
            .await
            .status(),
            StatusCode::OK
        );

        let merge = registry.begin("alice").await.unwrap();
        assert_eq!(
            execute_sql_request(
                &registry,
                "alice",
                SqlTransactionRequest {
                    sql: r#"MERGE INTO product.datasets AS target USING (VALUES ('keep-me', '{"kept":true}')) AS source (id, document_json) ON target.id = source.id WHEN MATCHED THEN UPDATE SET document_json = source.document_json WHEN NOT MATCHED THEN INSERT (id, document_json) VALUES (source.id, CAST('{"created":true}' AS JSON)) WHEN NOT MATCHED BY SOURCE AND target.id IN ('archive-me') THEN UPDATE SET document_json = CAST('{"archived":true}' AS JSON)"#.into(),
                    transaction_id: Some(merge.transaction_id.clone()),
                    parameters: vec![],
                    named_parameters: HashMap::new(),
                },
            )
            .await
            .status(),
            StatusCode::OK
        );
        assert_eq!(
            execute_sql_request(
                &registry,
                "alice",
                SqlTransactionRequest {
                    sql: "COMMIT".into(),
                    transaction_id: Some(merge.transaction_id),
                    parameters: vec![],
                    named_parameters: HashMap::new(),
                },
            )
            .await
            .status(),
            StatusCode::OK
        );
        let snapshot = catalog.read_current().await.unwrap();
        let keep = snapshot
            .product_record(ProductRecordKind::Dataset, "keep-me")
            .unwrap()
            .unwrap();
        let archived = snapshot
            .product_record(ProductRecordKind::Dataset, "archive-me")
            .unwrap()
            .unwrap();
        assert_eq!(keep.revision, 2);
        assert_eq!(archived.revision, 2);
        let archived_document = catalog
            .fetch_product_document_at(&snapshot, ProductRecordKind::Dataset, "archive-me")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            String::from_utf8(archived_document).unwrap(),
            r#"{"archived":true}"#
        );
        let untouched = snapshot
            .product_record(ProductRecordKind::Dataset, "leave-me")
            .unwrap()
            .unwrap();
        assert_eq!(untouched.revision, 1);
        let untouched_document = catalog
            .fetch_product_document_at(&snapshot, ProductRecordKind::Dataset, "leave-me")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(String::from_utf8(untouched_document).unwrap(), "{}");
    }

    #[tokio::test]
    async fn sql_endpoint_bounded_merge_matched_update_is_cas_checked_and_unmatched_is_noop() {
        let (registry, catalog) = registry().await;
        let first = registry.begin("alice").await.unwrap();
        assert_eq!(
            execute_sql_request(
                &registry,
                "alice",
                SqlTransactionRequest {
                    sql: "INSERT INTO product.datasets (id, document_json) VALUES ('update-me', '{}')".into(),
                    transaction_id: Some(first.transaction_id.clone()),
                    parameters: vec![],
                    named_parameters: HashMap::new(),
                },
            )
            .await
            .status(),
            StatusCode::OK
        );
        assert_eq!(
            execute_sql_request(
                &registry,
                "alice",
                SqlTransactionRequest {
                    sql: "COMMIT".into(),
                    transaction_id: Some(first.transaction_id),
                    parameters: vec![],
                    named_parameters: HashMap::new(),
                },
            )
            .await
            .status(),
            StatusCode::OK
        );

        let merge = registry.begin("alice").await.unwrap();
        assert_eq!(
            execute_sql_request(
                &registry,
                "alice",
                SqlTransactionRequest {
                    sql: r#"MERGE INTO product.datasets AS target USING (VALUES ('update-me', '{"changed":true}'), ('missing', '{"ignored":true}')) AS source (id, document_json) ON target.id = source.id WHEN MATCHED THEN UPDATE SET document_json = source.document_json"#.into(),
                    transaction_id: Some(merge.transaction_id.clone()),
                    parameters: vec![],
                    named_parameters: HashMap::new(),
                },
            )
            .await
            .status(),
            StatusCode::OK
        );
        assert_eq!(
            execute_sql_request(
                &registry,
                "alice",
                SqlTransactionRequest {
                    sql: "COMMIT".into(),
                    transaction_id: Some(merge.transaction_id),
                    parameters: vec![],
                    named_parameters: HashMap::new(),
                },
            )
            .await
            .status(),
            StatusCode::OK
        );
        let snapshot = catalog.read_current().await.unwrap();
        let updated = snapshot
            .product_record(ProductRecordKind::Dataset, "update-me")
            .unwrap()
            .unwrap();
        assert_eq!(updated.revision, 2);
        let document = catalog
            .fetch_product_document_at(&snapshot, ProductRecordKind::Dataset, "update-me")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            serde_json::from_slice::<serde_json::Value>(&document).unwrap(),
            serde_json::json!({"changed": true})
        );
        assert!(
            snapshot
                .product_record(ProductRecordKind::Dataset, "missing")
                .unwrap()
                .is_none()
        );

        // Keep the server path qualified for the bounded literal form too:
        // the parser acceptance alone is not enough to prove transaction
        // adaptation, revision-CAS, and snapshot visibility.
        let literal_merge = registry.begin("alice").await.unwrap();
        assert_eq!(
            execute_sql_request(
                &registry,
                "alice",
                SqlTransactionRequest {
                    sql: r#"MERGE INTO product.datasets AS target USING (VALUES ('update-me', '{"ignored":true}'), ('missing-literal', '{}')) AS source (id, document_json) ON target.id = source.id WHEN MATCHED THEN UPDATE SET document_json = CAST('{"literal":true}' AS JSON)"#.into(),
                    transaction_id: Some(literal_merge.transaction_id.clone()),
                    parameters: vec![],
                    named_parameters: HashMap::new(),
                },
            )
            .await
            .status(),
            StatusCode::OK
        );
        assert_eq!(
            execute_sql_request(
                &registry,
                "alice",
                SqlTransactionRequest {
                    sql: "COMMIT".into(),
                    transaction_id: Some(literal_merge.transaction_id),
                    parameters: vec![],
                    named_parameters: HashMap::new(),
                },
            )
            .await
            .status(),
            StatusCode::OK
        );
        let snapshot = catalog.read_current().await.unwrap();
        let updated = snapshot
            .product_record(ProductRecordKind::Dataset, "update-me")
            .unwrap()
            .unwrap();
        assert_eq!(updated.revision, 3);
        let document = catalog
            .fetch_product_document_at(&snapshot, ProductRecordKind::Dataset, "update-me")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            serde_json::from_slice::<serde_json::Value>(&document).unwrap(),
            serde_json::json!({"literal": true})
        );
        assert!(
            snapshot
                .product_record(ProductRecordKind::Dataset, "missing-literal")
                .unwrap()
                .is_none()
        );

        let filtered_merge = registry.begin("alice").await.unwrap();
        assert_eq!(
            execute_sql_request(
                &registry,
                "alice",
                SqlTransactionRequest {
                    sql: r#"MERGE INTO product.datasets AS target USING (VALUES ('update-me', '{"ignored":true}'), ('missing-literal', '{"ignored":true}')) AS source (id, document_json) ON target.id = source.id WHEN MATCHED AND source.id IN ('update-me') THEN UPDATE SET document_json = '{"filtered":true}'"#.into(),
                    transaction_id: Some(filtered_merge.transaction_id.clone()),
                    parameters: vec![],
                    named_parameters: HashMap::new(),
                },
            )
            .await
            .status(),
            StatusCode::OK
        );
        assert_eq!(
            execute_sql_request(
                &registry,
                "alice",
                SqlTransactionRequest {
                    sql: "COMMIT".into(),
                    transaction_id: Some(filtered_merge.transaction_id),
                    parameters: vec![],
                    named_parameters: HashMap::new(),
                },
            )
            .await
            .status(),
            StatusCode::OK
        );
        let snapshot = catalog.read_current().await.unwrap();
        let updated = snapshot
            .product_record(ProductRecordKind::Dataset, "update-me")
            .unwrap()
            .unwrap();
        assert_eq!(updated.revision, 4);
        let document = catalog
            .fetch_product_document_at(&snapshot, ProductRecordKind::Dataset, "update-me")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            serde_json::from_slice::<serde_json::Value>(&document).unwrap(),
            serde_json::json!({"filtered": true})
        );

        let target_filtered_merge = registry.begin("alice").await.unwrap();
        assert_eq!(
            execute_sql_request(
                &registry,
                "alice",
                SqlTransactionRequest {
                    sql: r#"MERGE INTO product.datasets AS target USING (VALUES ('update-me', '{"ignored":true}'), ('missing-target', '{"ignored":true}')) AS source (id, document_json) ON target.id = source.id WHEN MATCHED AND target.id = 'update-me' THEN UPDATE SET document_json = '{"target_filtered":true}'"#.into(),
                    transaction_id: Some(target_filtered_merge.transaction_id.clone()),
                    parameters: vec![],
                    named_parameters: HashMap::new(),
                },
            )
            .await
            .status(),
            StatusCode::OK
        );
        assert_eq!(
            execute_sql_request(
                &registry,
                "alice",
                SqlTransactionRequest {
                    sql: "COMMIT".into(),
                    transaction_id: Some(target_filtered_merge.transaction_id),
                    parameters: vec![],
                    named_parameters: HashMap::new(),
                },
            )
            .await
            .status(),
            StatusCode::OK
        );
        let snapshot = catalog.read_current().await.unwrap();
        let updated = snapshot
            .product_record(ProductRecordKind::Dataset, "update-me")
            .unwrap()
            .unwrap();
        assert_eq!(updated.revision, 5);
        let document = catalog
            .fetch_product_document_at(&snapshot, ProductRecordKind::Dataset, "update-me")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            serde_json::from_slice::<serde_json::Value>(&document).unwrap(),
            serde_json::json!({"target_filtered": true})
        );
        assert!(
            snapshot
                .product_record(ProductRecordKind::Dataset, "missing-target")
                .unwrap()
                .is_none()
        );
    }

    #[tokio::test]
    async fn sql_endpoint_bounded_merge_filtered_match_updates_and_inserts() {
        let (registry, catalog) = registry().await;
        let seed = registry.begin("alice").await.unwrap();
        for (id, document) in [
            ("update-me", "{\"old\":true}"),
            ("keep-existing", "{\"keep\":true}"),
        ] {
            assert_eq!(
                execute_sql_request(
                    &registry,
                    "alice",
                    SqlTransactionRequest {
                        sql: format!(
                            "INSERT INTO product.datasets (id, document_json) VALUES ('{id}', '{document}')"
                        ),
                        transaction_id: Some(seed.transaction_id.clone()),
                        parameters: vec![],
                        named_parameters: HashMap::new(),
                    },
                )
                .await
                .status(),
                StatusCode::OK
            );
        }
        assert_eq!(
            execute_sql_request(
                &registry,
                "alice",
                SqlTransactionRequest {
                    sql: "COMMIT".into(),
                    transaction_id: Some(seed.transaction_id),
                    parameters: vec![],
                    named_parameters: HashMap::new(),
                },
            )
            .await
            .status(),
            StatusCode::OK
        );

        let merge = registry.begin("alice").await.unwrap();
        assert_eq!(
            execute_sql_request(
                &registry,
                "alice",
                SqlTransactionRequest {
                    sql: r#"MERGE INTO product.datasets AS target USING (VALUES ('update-me', '{"changed":true}'), ('keep-existing', '{"must_not_change":true}'), ('new-row', '{"created":true}')) AS source (id, document_json) ON target.id = source.id WHEN MATCHED AND source.id IN ('update-me') THEN UPDATE SET document_json = source.document_json WHEN NOT MATCHED THEN INSERT (id, document_json) VALUES (source.id, source.document_json)"#.into(),
                    transaction_id: Some(merge.transaction_id.clone()),
                    parameters: vec![],
                    named_parameters: HashMap::new(),
                },
            )
            .await
            .status(),
            StatusCode::OK
        );
        assert_eq!(
            execute_sql_request(
                &registry,
                "alice",
                SqlTransactionRequest {
                    sql: "COMMIT".into(),
                    transaction_id: Some(merge.transaction_id),
                    parameters: vec![],
                    named_parameters: HashMap::new(),
                },
            )
            .await
            .status(),
            StatusCode::OK
        );

        let snapshot = catalog.read_current().await.unwrap();
        let updated = snapshot
            .product_record(ProductRecordKind::Dataset, "update-me")
            .unwrap()
            .unwrap();
        assert_eq!(updated.revision, 2);
        let unchanged = snapshot
            .product_record(ProductRecordKind::Dataset, "keep-existing")
            .unwrap()
            .unwrap();
        assert_eq!(unchanged.revision, 1);
        let created = snapshot
            .product_record(ProductRecordKind::Dataset, "new-row")
            .unwrap()
            .unwrap();
        assert_eq!(created.revision, 1);
        let updated_document = catalog
            .fetch_product_document_at(&snapshot, ProductRecordKind::Dataset, "update-me")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            serde_json::from_slice::<serde_json::Value>(&updated_document).unwrap(),
            serde_json::json!({"changed": true})
        );
        let unchanged_document = catalog
            .fetch_product_document_at(&snapshot, ProductRecordKind::Dataset, "keep-existing")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            serde_json::from_slice::<serde_json::Value>(&unchanged_document).unwrap(),
            serde_json::json!({"keep": true})
        );

        let actions = registry.begin("alice").await.unwrap();
        let response = execute_sql_request(
            &registry,
            "alice",
            SqlTransactionRequest {
                sql: r#"MERGE INTO product.datasets AS target USING (VALUES ('keep-existing', '{"ignored":true}'), ('action-new', '{"created":true}')) AS source (id, document_json) ON target.id = source.id WHEN MATCHED AND source.id IN ('keep-existing') THEN DELETE WHEN NOT MATCHED THEN INSERT (id, document_json) VALUES (source.id, source.document_json) OUTPUT $action"#.into(),
                transaction_id: Some(actions.transaction_id.clone()),
                parameters: vec![],
                named_parameters: HashMap::new(),
            },
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);
        let body = axum::body::to_bytes(response.into_body(), 16 * 1024)
            .await
            .unwrap();
        let value: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(
            value["returning"],
            serde_json::json!([{"action": "DELETE"}, {"action": "INSERT"}])
        );
        assert_eq!(
            execute_sql_request(
                &registry,
                "alice",
                SqlTransactionRequest {
                    sql: "COMMIT".into(),
                    transaction_id: Some(actions.transaction_id),
                    parameters: vec![],
                    named_parameters: HashMap::new(),
                },
            )
            .await
            .status(),
            StatusCode::OK
        );
    }

    #[tokio::test]
    async fn sql_endpoint_bounded_merge_filtered_match_deletes_and_inserts() {
        let (registry, catalog) = registry().await;
        let seed = registry.begin("alice").await.unwrap();
        for (id, document) in [
            ("delete-me", "{\"old\":true}"),
            ("keep-existing", "{\"keep\":true}"),
        ] {
            assert_eq!(
                execute_sql_request(
                    &registry,
                    "alice",
                    SqlTransactionRequest {
                        sql: format!(
                            "INSERT INTO product.datasets (id, document_json) VALUES ('{id}', '{document}')"
                        ),
                        transaction_id: Some(seed.transaction_id.clone()),
                        parameters: vec![],
                        named_parameters: HashMap::new(),
                    },
                )
                .await
                .status(),
                StatusCode::OK
            );
        }
        assert_eq!(
            execute_sql_request(
                &registry,
                "alice",
                SqlTransactionRequest {
                    sql: "COMMIT".into(),
                    transaction_id: Some(seed.transaction_id),
                    parameters: vec![],
                    named_parameters: HashMap::new(),
                },
            )
            .await
            .status(),
            StatusCode::OK
        );

        let merge = registry.begin("alice").await.unwrap();
        let response = execute_sql_request(
            &registry,
            "alice",
            SqlTransactionRequest {
                sql: r#"MERGE INTO product.datasets AS target USING (VALUES ('delete-me', '{"ignored":true}'), ('keep-existing', '{"must_not_change":true}'), ('new-row', '{"created":true}')) AS source (id, document_json) ON target.id = source.id WHEN MATCHED AND source.id IN ('delete-me') THEN DELETE WHEN NOT MATCHED THEN INSERT (id, document_json) VALUES (source.id, source.document_json) OUTPUT deleted.id, deleted.revision, deleted.document_json"#.into(),
                transaction_id: Some(merge.transaction_id.clone()),
                parameters: vec![],
                named_parameters: HashMap::new(),
            },
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);
        let body = axum::body::to_bytes(response.into_body(), 16 * 1024)
            .await
            .unwrap();
        let value: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(value["returning"].as_array().map(Vec::len), Some(1));
        assert_eq!(value["returning"][0]["id"], "delete-me");
        assert_eq!(value["returning"][0]["revision"], 1);
        assert_eq!(value["returning"][0]["document_json"], "{\"old\":true}");
        assert_eq!(
            execute_sql_request(
                &registry,
                "alice",
                SqlTransactionRequest {
                    sql: "COMMIT".into(),
                    transaction_id: Some(merge.transaction_id),
                    parameters: vec![],
                    named_parameters: HashMap::new(),
                },
            )
            .await
            .status(),
            StatusCode::OK
        );

        let snapshot = catalog.read_current().await.unwrap();
        assert!(
            snapshot
                .product_record(ProductRecordKind::Dataset, "delete-me")
                .unwrap()
                .is_none()
        );
        let unchanged = snapshot
            .product_record(ProductRecordKind::Dataset, "keep-existing")
            .unwrap()
            .unwrap();
        assert_eq!(unchanged.revision, 1);
        let new_row = snapshot
            .product_record(ProductRecordKind::Dataset, "new-row")
            .unwrap()
            .unwrap();
        assert_eq!(new_row.revision, 1);
        let unchanged_document = catalog
            .fetch_product_document_at(&snapshot, ProductRecordKind::Dataset, "keep-existing")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            serde_json::from_slice::<serde_json::Value>(&unchanged_document).unwrap(),
            serde_json::json!({"keep": true})
        );
    }

    #[tokio::test]
    async fn sql_endpoint_bounded_merge_unmatched_insert_is_idempotent() {
        let (registry, catalog) = registry().await;
        let first = registry.begin("alice").await.unwrap();
        assert_eq!(
            execute_sql_request(
                &registry,
                "alice",
                SqlTransactionRequest {
                    sql:
                        "INSERT INTO product.datasets (id, document_json) VALUES ('existing', '{}')"
                            .into(),
                    transaction_id: Some(first.transaction_id.clone()),
                    parameters: vec![],
                    named_parameters: HashMap::new(),
                },
            )
            .await
            .status(),
            StatusCode::OK
        );
        assert_eq!(
            execute_sql_request(
                &registry,
                "alice",
                SqlTransactionRequest {
                    sql: "COMMIT".into(),
                    transaction_id: Some(first.transaction_id),
                    parameters: vec![],
                    named_parameters: HashMap::new(),
                },
            )
            .await
            .status(),
            StatusCode::OK
        );
        let merge = registry.begin("alice").await.unwrap();
        assert_eq!(
            execute_sql_request(
                &registry,
                "alice",
                SqlTransactionRequest {
                    sql: "MERGE INTO product.datasets AS target USING (VALUES ('existing', '{\"changed\":true}'), ('new', '{\"n\":1}')) AS source (id, document_json) ON target.id = source.id WHEN NOT MATCHED AND source.id IN ('new') THEN INSERT (id, document_json) VALUES (source.id, source.document_json)".into(),
                    transaction_id: Some(merge.transaction_id.clone()),
                    parameters: vec![],
                    named_parameters: HashMap::new(),
                },
            )
            .await
            .status(),
            StatusCode::OK
        );
        assert_eq!(
            execute_sql_request(
                &registry,
                "alice",
                SqlTransactionRequest {
                    sql: "COMMIT".into(),
                    transaction_id: Some(merge.transaction_id),
                    parameters: vec![],
                    named_parameters: HashMap::new(),
                },
            )
            .await
            .status(),
            StatusCode::OK
        );
        let snapshot = catalog.read_current().await.unwrap();
        assert_eq!(
            snapshot
                .product_record(ProductRecordKind::Dataset, "existing")
                .unwrap()
                .unwrap()
                .revision,
            1
        );
        assert_eq!(
            snapshot
                .product_record(ProductRecordKind::Dataset, "new")
                .unwrap()
                .unwrap()
                .revision,
            1
        );
    }

    #[tokio::test]
    async fn sql_endpoint_bounded_merge_delete_or_insert_is_owner_checked_and_atomic() {
        let (registry, catalog) = registry().await;
        let first = registry.begin("alice").await.unwrap();
        assert_eq!(
            execute_sql_request(
                &registry,
                "alice",
                SqlTransactionRequest {
                    sql: r#"INSERT INTO product.datasets (id, document_json) VALUES ('remove-me', '{}')"#.into(),
                    transaction_id: Some(first.transaction_id.clone()),
                    parameters: vec![],
                    named_parameters: HashMap::new(),
                },
            )
            .await
            .status(),
            StatusCode::OK
        );
        assert_eq!(
            execute_sql_request(
                &registry,
                "alice",
                SqlTransactionRequest {
                    sql: "COMMIT".into(),
                    transaction_id: Some(first.transaction_id),
                    parameters: vec![],
                    named_parameters: HashMap::new(),
                },
            )
            .await
            .status(),
            StatusCode::OK
        );

        let merge = registry.begin("alice").await.unwrap();
        let response = execute_sql_request(
            &registry,
            "alice",
            SqlTransactionRequest {
                sql: r#"MERGE INTO product.datasets AS target USING (VALUES ('remove-me', '{"ignored":true}'), ('new-row', '{"n":2}')) AS source (id, document_json) ON target.id = source.id WHEN MATCHED THEN DELETE WHEN NOT MATCHED THEN INSERT (id, document_json) VALUES (source.id, source.document_json)"#.into(),
                transaction_id: Some(merge.transaction_id.clone()),
                parameters: vec![],
                named_parameters: HashMap::new(),
            },
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            execute_sql_request(
                &registry,
                "alice",
                SqlTransactionRequest {
                    sql: "COMMIT".into(),
                    transaction_id: Some(merge.transaction_id),
                    parameters: vec![],
                    named_parameters: HashMap::new(),
                },
            )
            .await
            .status(),
            StatusCode::OK
        );
        let snapshot = catalog.read_current().await.unwrap();
        assert!(
            snapshot
                .product_record(ProductRecordKind::Dataset, "remove-me")
                .unwrap()
                .is_none()
        );
        assert_eq!(
            snapshot
                .product_record(ProductRecordKind::Dataset, "new-row")
                .unwrap()
                .unwrap()
                .revision,
            1
        );
    }

    #[tokio::test]
    async fn sql_endpoint_savepoint_rolls_back_staged_product_changes() {
        let (registry, catalog) = registry().await;
        let begun = registry.begin("alice").await.unwrap();
        let id = begun.transaction_id.clone();
        registry
            .stage_product_command(
                "alice",
                &id,
                ProductDmlCommand::Create {
                    kind: "dataset".into(),
                    id: "orders".into(),
                    document_json: r#"{"name":"Orders"}"#.into(),
                },
            )
            .await
            .unwrap();
        let savepoint = execute_sql_request(
            &registry,
            "alice",
            SqlTransactionRequest {
                sql: "SAVEPOINT before_extra".into(),
                transaction_id: Some(id.clone()),
                parameters: vec![],
                named_parameters: HashMap::new(),
            },
        )
        .await;
        assert_eq!(savepoint.status(), StatusCode::NO_CONTENT);
        registry
            .stage_product_command(
                "alice",
                &id,
                ProductDmlCommand::Create {
                    kind: "dataset".into(),
                    id: "customers".into(),
                    document_json: r#"{"name":"Customers"}"#.into(),
                },
            )
            .await
            .unwrap();
        let rollback = execute_sql_request(
            &registry,
            "alice",
            SqlTransactionRequest {
                sql: "ROLLBACK TO SAVEPOINT before_extra".into(),
                transaction_id: Some(id.clone()),
                parameters: vec![],
                named_parameters: HashMap::new(),
            },
        )
        .await;
        assert_eq!(rollback.status(), StatusCode::OK);
        let commit = execute_sql_request(
            &registry,
            "alice",
            SqlTransactionRequest {
                sql: "COMMIT".into(),
                transaction_id: Some(id),
                parameters: vec![],
                named_parameters: HashMap::new(),
            },
        )
        .await;
        assert_eq!(commit.status(), StatusCode::OK);
        let snapshot = catalog.read_current().await.unwrap();
        assert!(
            snapshot
                .product_record(ProductRecordKind::Dataset, "orders")
                .unwrap()
                .is_some()
        );
        assert!(
            snapshot
                .product_record(ProductRecordKind::Dataset, "customers")
                .unwrap()
                .is_none()
        );
    }

    #[tokio::test]
    async fn sql_endpoint_stages_multi_row_product_insert_in_one_commit() {
        let (registry, catalog) = registry().await;
        let begun = registry.begin("alice").await.unwrap();
        let response = execute_sql_request(
            &registry,
            "alice",
            SqlTransactionRequest {
                sql: r#"INSERT INTO kaveon.product.datasets (id, document_json) VALUES ('one', '{}'), ('two', '{"name":"Two"}')"#.into(),
                transaction_id: Some(begun.transaction_id.clone()),
                parameters: vec![],
                named_parameters: HashMap::new(),
            },
        )
        .await;
        if response.status() != StatusCode::OK {
            let body = axum::body::to_bytes(response.into_body(), usize::MAX)
                .await
                .unwrap();
            panic!("source copy failed: {}", String::from_utf8_lossy(&body));
        }
        let response = execute_sql_request(
            &registry,
            "alice",
            SqlTransactionRequest {
                sql: "COMMIT".into(),
                transaction_id: Some(begun.transaction_id),
                parameters: vec![],
                named_parameters: HashMap::new(),
            },
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);
        let snapshot = catalog.read_current().await.unwrap();
        assert!(
            snapshot
                .product_record(ProductRecordKind::Dataset, "one")
                .unwrap()
                .is_some()
        );
        assert!(
            snapshot
                .product_record(ProductRecordKind::Dataset, "two")
                .unwrap()
                .is_some()
        );
    }

    #[tokio::test]
    async fn sql_endpoint_product_insert_accepts_sql_server_output() {
        let (registry, catalog) = registry().await;
        let begun = registry.begin("alice").await.unwrap();
        let response = execute_sql_request(
            &registry,
            "alice",
            SqlTransactionRequest {
                sql: "INSERT INTO product.datasets (id, document_json) OUTPUT inserted.id, inserted.revision VALUES ('output-insert', '{}')".into(),
                transaction_id: Some(begun.transaction_id.clone()),
                parameters: vec![],
                named_parameters: HashMap::new(),
            },
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);
        let body = axum::body::to_bytes(response.into_body(), 16 * 1024)
            .await
            .unwrap();
        let value: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(value["returning"][0]["id"], "output-insert");
        assert_eq!(value["returning"][0]["revision"], 1);
        let commit = execute_sql_request(
            &registry,
            "alice",
            SqlTransactionRequest {
                sql: "COMMIT".into(),
                transaction_id: Some(begun.transaction_id),
                parameters: vec![],
                named_parameters: HashMap::new(),
            },
        )
        .await;
        assert_eq!(commit.status(), StatusCode::OK);
        assert!(
            catalog
                .read_current()
                .await
                .unwrap()
                .product_record(ProductRecordKind::Dataset, "output-insert")
                .unwrap()
                .is_some()
        );
    }

    #[tokio::test]
    async fn sql_endpoint_product_insert_accepts_sql_server_output_wildcard() {
        let (registry, _catalog) = registry().await;
        let begun = registry.begin("alice").await.unwrap();
        let response = execute_sql_request(
            &registry,
            "alice",
            SqlTransactionRequest {
                sql: "INSERT INTO product.datasets (id, document_json) OUTPUT inserted.* VALUES ('output-wildcard', '{}')".into(),
                transaction_id: Some(begun.transaction_id),
                parameters: vec![],
                named_parameters: HashMap::new(),
            },
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);
        let body = axum::body::to_bytes(response.into_body(), 16 * 1024)
            .await
            .unwrap();
        let value: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(value["returning"][0]["id"], "output-wildcard");
        assert_eq!(value["returning"][0]["revision"], 1);
        assert_eq!(value["returning"][0]["document_json"], "{}");
    }

    #[tokio::test]
    async fn sql_endpoint_commits_product_insert_with_reordered_columns() {
        let (registry, _catalog) = registry().await;
        let begun = registry.begin("alice").await.unwrap();
        let response = execute_sql_request(
            &registry,
            "alice",
            SqlTransactionRequest {
                sql: r#"INSERT INTO product.datasets (document_json, id) VALUES ('{"name":"Orders"}', 'orders')"#.into(),
                transaction_id: Some(begun.transaction_id.clone()),
                parameters: vec![],
                named_parameters: HashMap::new(),
            },
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);
        let response = execute_sql_request(
            &registry,
            "alice",
            SqlTransactionRequest {
                sql: "COMMIT".into(),
                transaction_id: Some(begun.transaction_id),
                parameters: vec![],
                named_parameters: HashMap::new(),
            },
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);

        let identity = Identity {
            principal: "alice".into(),
            display_identity: None,
            role: crate::security::Role::Analyst,
        };
        let record = registry
            .read_product(&identity, ProductRecordKind::Dataset, "orders")
            .await
            .expect("reordered insert should commit by column name");
        assert_eq!(record.document["name"], "Orders");
    }

    #[tokio::test]
    async fn sql_endpoint_commits_constant_product_insert_select() {
        let (registry, _catalog) = registry().await;
        let begun = registry.begin("alice").await.unwrap();
        let response = execute_sql_request(
            &registry,
            "alice",
            SqlTransactionRequest {
                sql: r#"INSERT INTO product.datasets (id, document_json) SELECT 'orders', '{"name":"Orders"}'"#.into(),
                transaction_id: Some(begun.transaction_id.clone()),
                parameters: vec![],
                named_parameters: HashMap::new(),
            },
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);
        let response = execute_sql_request(
            &registry,
            "alice",
            SqlTransactionRequest {
                sql: "COMMIT".into(),
                transaction_id: Some(begun.transaction_id),
                parameters: vec![],
                named_parameters: HashMap::new(),
            },
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);
        let identity = Identity {
            principal: "alice".into(),
            display_identity: None,
            role: crate::security::Role::Analyst,
        };
        let record = registry
            .read_product(&identity, ProductRecordKind::Dataset, "orders")
            .await
            .expect("constant INSERT SELECT should commit");
        assert_eq!(record.document["name"], "Orders");
    }

    #[tokio::test]
    async fn sql_endpoint_commits_bounded_product_table_insert_select() {
        let (registry, _catalog) = registry().await;
        let seed = registry.begin("alice").await.unwrap();
        let response = execute_sql_request(
            &registry,
            "alice",
            SqlTransactionRequest {
                sql: r#"INSERT INTO product.datasets (id, document_json) VALUES ('orders', '{"name":"Orders"}'), ('other', '{"name":"Other"}')"#.into(),
                transaction_id: Some(seed.transaction_id.clone()),
                parameters: vec![],
                named_parameters: HashMap::new(),
            },
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);
        let response = execute_sql_request(
            &registry,
            "alice",
            SqlTransactionRequest {
                sql: "COMMIT".into(),
                transaction_id: Some(seed.transaction_id),
                parameters: vec![],
                named_parameters: HashMap::new(),
            },
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);

        let copy = registry.begin("alice").await.unwrap();
        let response = execute_sql_request(
            &registry,
            "alice",
            SqlTransactionRequest {
                sql: "INSERT INTO product.datasets (id, document_json) SELECT id, CAST(document_json AS JSON) FROM product.datasets WHERE id IN ('orders') ON CONFLICT (id) DO UPDATE SET document_json = excluded.document_json RETURNING *".into(),
                transaction_id: Some(copy.transaction_id.clone()),
                parameters: vec![],
                named_parameters: HashMap::new(),
            },
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);
        let body = axum::body::to_bytes(response.into_body(), 16 * 1024)
            .await
            .unwrap();
        let value: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(value["returning"][0]["id"], "orders");
        assert_eq!(value["returning"][0]["revision"], 2);
        assert_eq!(
            value["returning"][0]["document_json"],
            r#"{"name":"Orders"}"#
        );
        let response = execute_sql_request(
            &registry,
            "alice",
            SqlTransactionRequest {
                sql: "COMMIT".into(),
                transaction_id: Some(copy.transaction_id),
                parameters: vec![],
                named_parameters: HashMap::new(),
            },
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);
        let identity = Identity {
            principal: "alice".into(),
            display_identity: None,
            role: crate::security::Role::Analyst,
        };
        let record = registry
            .read_product(&identity, ProductRecordKind::Dataset, "orders")
            .await
            .expect("source-backed INSERT SELECT should commit");
        assert_eq!(record.revision, 2);
        assert_eq!(record.document["name"], "Orders");
        let other = registry
            .read_product(&identity, ProductRecordKind::Dataset, "other")
            .await
            .expect("unselected source row should remain readable");
        assert_eq!(other.revision, 1);

        let unfiltered = registry.begin("alice").await.unwrap();
        let response = execute_sql_request(
            &registry,
            "alice",
            SqlTransactionRequest {
                sql: "INSERT INTO product.datasets (id, document_json) SELECT id, document_json FROM product.datasets ON CONFLICT (id) DO UPDATE SET document_json = excluded.document_json RETURNING id, revision, document_json".into(),
                transaction_id: Some(unfiltered.transaction_id.clone()),
                parameters: vec![],
                named_parameters: HashMap::new(),
            },
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);
        let body = axum::body::to_bytes(response.into_body(), 16 * 1024)
            .await
            .unwrap();
        let value: serde_json::Value = serde_json::from_slice(&body).unwrap();
        let rows = value["returning"].as_array().unwrap();
        assert_eq!(rows.len(), 2);
        assert!(rows.iter().any(|row| {
            row["id"] == "orders"
                && row["revision"] == 3
                && row["document_json"] == r#"{"name":"Orders"}"#
        }));
        assert!(rows.iter().any(|row| {
            row["id"] == "other"
                && row["revision"] == 2
                && row["document_json"] == r#"{"name":"Other"}"#
        }));
        let response = execute_sql_request(
            &registry,
            "alice",
            SqlTransactionRequest {
                sql: "COMMIT".into(),
                transaction_id: Some(unfiltered.transaction_id),
                parameters: vec![],
                named_parameters: HashMap::new(),
            },
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn bounded_source_copy_returning_document_is_owner_isolated() {
        let (registry, _catalog) = registry().await;
        let seed = registry.begin("bob").await.unwrap();
        let response = execute_sql_request(
            &registry,
            "bob",
            SqlTransactionRequest {
                sql: r#"INSERT INTO product.datasets (id, document_json) VALUES ('private', '{"owner":"bob"}')"#.into(),
                transaction_id: Some(seed.transaction_id.clone()),
                parameters: vec![],
                named_parameters: HashMap::new(),
            },
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);
        let response = execute_sql_request(
            &registry,
            "bob",
            SqlTransactionRequest {
                sql: "COMMIT".into(),
                transaction_id: Some(seed.transaction_id),
                parameters: vec![],
                named_parameters: HashMap::new(),
            },
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);

        let copy = registry.begin("alice").await.unwrap();
        let response = execute_sql_request(
            &registry,
            "alice",
            SqlTransactionRequest {
                sql: "INSERT INTO product.datasets (id, document_json) SELECT id, document_json FROM product.datasets WHERE id IN ('private') ON CONFLICT (id) DO UPDATE SET document_json = excluded.document_json RETURNING id, revision, document_json".into(),
                transaction_id: Some(copy.transaction_id.clone()),
                parameters: vec![],
                named_parameters: HashMap::new(),
            },
        )
        .await;
        assert_eq!(response.status(), StatusCode::FORBIDDEN);
        let body = axum::body::to_bytes(response.into_body(), 4096)
            .await
            .unwrap();
        let value: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(
            value["error"],
            "product record owner does not match the authenticated principal"
        );
    }

    #[test]
    fn transaction_parameter_binder_preserves_literals_comments_and_arrays() {
        let bound = bind_transaction_parameters(
            "INSERT INTO product.datasets (id, document_json) VALUES ($1, $2) -- :ignored\n",
            &[
                serde_json::json!("ds-1"),
                serde_json::json!("{\"name\":\"$3\"}"),
            ],
            &HashMap::new(),
        )
        .unwrap();
        assert_eq!(
            bound,
            "INSERT INTO product.datasets (id, document_json) VALUES ('ds-1', '{\"name\":\"$3\"}') -- :ignored\n"
        );

        let bound = bind_transaction_parameters(
            "UPDATE product.datasets SET document_json = :document WHERE id = ? AND revision = 1",
            &[serde_json::json!("ds-1")],
            &HashMap::from([(String::from("document"), serde_json::json!("{}"))]),
        )
        .unwrap();
        assert_eq!(
            bound,
            "UPDATE product.datasets SET document_json = '{}' WHERE id = 'ds-1' AND revision = 1"
        );

        let array = bind_transaction_parameters(
            "SELECT ?",
            &[serde_json::json!([1, null, "two"])],
            &HashMap::new(),
        )
        .unwrap();
        assert_eq!(array, "SELECT ARRAY[1, NULL, 'two']");
    }

    #[test]
    fn transaction_parameter_binder_rejects_missing_extra_and_mixed_values() {
        for (sql, positional, named, expected) in [
            (
                "SELECT $2",
                vec![serde_json::json!(1)],
                HashMap::new(),
                "parameter $2 is not bound",
            ),
            (
                "SELECT ?",
                vec![serde_json::json!(1), serde_json::json!(2)],
                HashMap::new(),
                "only 1 '?' placeholders",
            ),
            (
                "SELECT :name",
                vec![],
                HashMap::new(),
                "named parameter :name is not bound",
            ),
            (
                "SELECT $1",
                vec![serde_json::json!(1)],
                HashMap::from([(String::from("unused"), serde_json::json!(2))]),
                "named parameter :unused",
            ),
            (
                "SELECT ? + $1",
                vec![serde_json::json!(1)],
                HashMap::new(),
                "cannot mix",
            ),
        ] {
            let error = bind_transaction_parameters(sql, &positional, &named).unwrap_err();
            assert!(
                error.contains(expected),
                "{error} did not contain {expected}"
            );
        }
    }

    #[tokio::test]
    async fn sql_endpoint_binds_product_dml_parameters_before_staging() {
        let (registry, catalog) = registry().await;
        let begun = registry.begin("alice").await.unwrap();
        let response = execute_sql_request(
            &registry,
            "alice",
            SqlTransactionRequest {
                sql: "INSERT INTO product.datasets (id, document_json) VALUES ($1, $2)".into(),
                transaction_id: Some(begun.transaction_id.clone()),
                parameters: vec![
                    serde_json::json!("bound"),
                    serde_json::json!("{\"ok\":true}"),
                ],
                named_parameters: HashMap::new(),
            },
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);
        let response = execute_sql_request(
            &registry,
            "alice",
            SqlTransactionRequest {
                sql: "COMMIT".into(),
                transaction_id: Some(begun.transaction_id),
                parameters: vec![],
                named_parameters: HashMap::new(),
            },
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);
        let snapshot = catalog.read_current().await.unwrap();
        let record = snapshot
            .product_record(ProductRecordKind::Dataset, "bound")
            .unwrap()
            .unwrap();
        assert!(record.document.path.contains("products/dataset/bound/"));
    }

    #[tokio::test]
    async fn sql_endpoint_rejects_generic_row_dml_without_staging() {
        let (registry, _) = registry().await;
        let begun = registry.begin("alice").await.unwrap();
        let response = execute_sql_request(
            &registry,
            "alice",
            SqlTransactionRequest {
                sql: "DELETE FROM app.users WHERE id = 'alice'".into(),
                transaction_id: Some(begun.transaction_id.clone()),
                parameters: vec![],
                named_parameters: HashMap::new(),
            },
        )
        .await;
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        let transaction = registry.take("alice", &begun.transaction_id).await.unwrap();
        assert_eq!(transaction.staged_change_count(), 0);
    }

    #[tokio::test]
    async fn product_point_read_is_owner_isolated_and_returns_no_snapshot_body() {
        let (registry, _) = registry().await;
        let begun = registry.begin("alice").await.unwrap();
        registry
            .stage_product_command(
                "alice",
                &begun.transaction_id,
                ProductDmlCommand::Create {
                    kind: "dataset".into(),
                    id: "orders".into(),
                    document_json: r#"{"name":"Orders"}"#.into(),
                },
            )
            .await
            .unwrap();
        let mut transaction = registry.take("alice", &begun.transaction_id).await.unwrap();
        transaction.bind_request_digest(b"alice").unwrap();
        transaction.commit().await.unwrap();

        let alice = Identity {
            principal: "alice".into(),
            display_identity: None,
            role: crate::security::Role::Analyst,
        };
        let bob = Identity {
            principal: "bob".into(),
            ..alice.clone()
        };
        let admin = Identity {
            principal: "admin".into(),
            role: crate::security::Role::Admin,
            display_identity: None,
        };
        let record = registry
            .read_product(&alice, ProductRecordKind::Dataset, "orders")
            .await
            .unwrap();
        let json = serde_json::to_value(&record).unwrap();
        assert_eq!(json["document"]["name"], "Orders");
        assert_eq!(json["revision"], 1);
        assert!(json.get("tables").is_none());
        assert!(json.get("product_records").is_none());
        assert_eq!(
            registry
                .read_product(&bob, ProductRecordKind::Dataset, "orders")
                .await
                .err()
                .unwrap(),
            RegistryError::OwnershipForbidden
        );
        assert!(
            registry
                .read_product(&admin, ProductRecordKind::Dataset, "orders")
                .await
                .is_ok()
        );
    }

    #[tokio::test]
    async fn product_point_read_distinguishes_missing_and_invalid_ids() {
        let (registry, _) = registry().await;
        let admin = Identity {
            principal: "admin".into(),
            role: crate::security::Role::Admin,
            display_identity: None,
        };
        assert_eq!(
            registry
                .read_product(&admin, ProductRecordKind::Dataset, "missing")
                .await
                .err()
                .unwrap(),
            RegistryError::ProductMissing
        );
        assert!(matches!(
            registry
                .read_product(&admin, ProductRecordKind::Dataset, "../unsafe")
                .await,
            Err(RegistryError::Invalid(_))
        ));
    }

    #[tokio::test]
    async fn product_list_is_bounded_paginated_and_owner_isolated() {
        let (registry, _) = registry().await;
        for (owner, id) in [("alice", "alpha"), ("bob", "bravo")] {
            let begun = registry.begin(owner).await.unwrap();
            registry
                .stage_product_command(
                    owner,
                    &begun.transaction_id,
                    ProductDmlCommand::Create {
                        kind: "dataset".into(),
                        id: id.into(),
                        document_json: format!(r#"{{"name":"{id}"}}"#),
                    },
                )
                .await
                .unwrap();
            let mut transaction = registry.take(owner, &begun.transaction_id).await.unwrap();
            transaction.bind_request_digest(owner.as_bytes()).unwrap();
            transaction.commit().await.unwrap();
        }
        let alice = Identity {
            principal: "alice".into(),
            display_identity: None,
            role: crate::security::Role::Analyst,
        };
        let admin = Identity {
            principal: "admin".into(),
            display_identity: None,
            role: crate::security::Role::Admin,
        };
        let owned = registry
            .list_products(&alice, ProductRecordKind::Dataset, 100, None)
            .await
            .unwrap();
        assert_eq!(owned.records.len(), 1);
        assert_eq!(owned.records[0].id, "alpha");
        let first = registry
            .list_products(&admin, ProductRecordKind::Dataset, 1, None)
            .await
            .unwrap();
        assert_eq!(first.records[0].id, "alpha");
        let second = registry
            .list_products(
                &admin,
                ProductRecordKind::Dataset,
                1,
                first.next_cursor.as_deref(),
            )
            .await
            .unwrap();
        assert_eq!(second.records[0].id, "bravo");
        assert!(second.next_cursor.is_none());
        assert!(matches!(
            registry
                .list_products(&admin, ProductRecordKind::Dataset, 0, None)
                .await,
            Err(RegistryError::Invalid(_))
        ));
    }

    #[tokio::test]
    async fn product_point_read_reports_document_corruption() {
        use object_store::{ObjectStore, path::Path as ObjectPath};

        let raw = Arc::new(InMemory::new());
        let catalog = ProductCatalogCommit::new(
            AdlsConditionalCommit::new(raw.clone()),
            "product",
            Arc::new(TransactionMetrics::default()),
        )
        .unwrap();
        catalog
            .initialize(CatalogSnapshot::empty("genesis").unwrap())
            .await;
        let registry = TransactionRegistry::enabled(catalog.clone());
        let begun = registry.begin("alice").await.unwrap();
        registry
            .stage_product_command(
                "alice",
                &begun.transaction_id,
                ProductDmlCommand::Create {
                    kind: "dataset".into(),
                    id: "orders".into(),
                    document_json: r#"{"name":"Orders"}"#.into(),
                },
            )
            .await
            .unwrap();
        let mut transaction = registry.take("alice", &begun.transaction_id).await.unwrap();
        transaction.bind_request_digest(b"alice").unwrap();
        transaction.commit().await.unwrap();
        let snapshot = catalog.read_current().await.unwrap();
        let path = &snapshot
            .product_record(ProductRecordKind::Dataset, "orders")
            .unwrap()
            .unwrap()
            .document
            .path;
        raw.put(
            &ObjectPath::from(format!("product/{path}")),
            b"corrupt".to_vec().into(),
        )
        .await
        .unwrap();
        let admin = Identity {
            principal: "admin".into(),
            role: crate::security::Role::Admin,
            display_identity: None,
        };
        assert_eq!(
            registry
                .read_product(&admin, ProductRecordKind::Dataset, "orders")
                .await
                .err()
                .unwrap(),
            RegistryError::Corrupt
        );
    }

    #[tokio::test]
    async fn typed_row_transactions_are_owner_isolated_and_rollback_is_invisible() {
        let (registry, catalog) = registry().await;
        let create = r#"{"table":"app.users","primary_key":"u-1","revision":1,"owner_principal":"alice","columns":{"email":{"type":"string","value":"ada@example.com"}},"unique_keys":{"email":"ada@example.com"}}"#;
        let begun = registry.begin("alice").await.unwrap();
        registry
            .stage_product_command(
                "alice",
                &begun.transaction_id,
                ProductDmlCommand::Create {
                    kind: "typed_row".into(),
                    id: "u-1".into(),
                    document_json: create.into(),
                },
            )
            .await
            .unwrap();
        let mut transaction = registry.take("alice", &begun.transaction_id).await.unwrap();
        transaction.bind_request_digest(b"typed-create").unwrap();
        transaction.commit().await.unwrap();
        assert_eq!(
            catalog.read_current().await.unwrap().typed_rows["app.users"]["u-1"].revision,
            1
        );

        let replay = registry.begin("alice").await.unwrap();
        registry
            .stage_product_commands(
                "alice",
                &replay.transaction_id,
                vec![ProductDmlCommand::CreateIgnore {
                    kind: "typed_row".into(),
                    id: "u-1".into(),
                    document_json: create.into(),
                }],
                false,
            )
            .await
            .unwrap();
        let mut transaction = registry
            .take("alice", &replay.transaction_id)
            .await
            .unwrap();
        transaction.bind_request_digest(b"typed-replay").unwrap();
        transaction.commit().await.unwrap();
        assert_eq!(
            catalog.read_current().await.unwrap().typed_rows["app.users"]["u-1"].revision,
            1
        );

        let bob = registry.begin("bob").await.unwrap();
        assert!(matches!(
            registry
                .stage_product_command(
                    "bob",
                    &bob.transaction_id,
                    ProductDmlCommand::Update {
                        kind: "typed_row".into(),
                        id: "u-1".into(),
                        expected_revision: 1,
                        document_json: create
                            .replace("\"revision\":1", "\"revision\":2")
                            .replace(
                                "\"owner_principal\":\"alice\"",
                                "\"owner_principal\":\"bob\""
                            )
                            .replace("ada@example.com", "bob@example.com"),
                    },
                )
                .await,
            Err(RegistryError::OwnershipForbidden)
        ));

        let update = registry.begin("alice").await.unwrap();
        registry
            .stage_product_command(
                "alice",
                &update.transaction_id,
                ProductDmlCommand::Update {
                    kind: "typed_row".into(),
                    id: "u-1".into(),
                    expected_revision: 1,
                    document_json: create
                        .replace("ada@example.com", "grace@example.com")
                        .replace("\"revision\":1", "\"revision\":2"),
                },
            )
            .await
            .unwrap();
        let mut transaction = registry
            .take("alice", &update.transaction_id)
            .await
            .unwrap();
        transaction.bind_request_digest(b"typed-update").unwrap();
        transaction.commit().await.unwrap();

        let upsert = registry.begin("alice").await.unwrap();
        registry
            .stage_product_command(
                "alice",
                &upsert.transaction_id,
                ProductDmlCommand::Upsert {
                    kind: "typed_row".into(),
                    id: "u-1".into(),
                    document_json: create
                        .replace("ada@example.com", "linus@example.com")
                        .replace("\"revision\":1", "\"revision\":3"),
                },
            )
            .await
            .unwrap();
        let mut transaction = registry
            .take("alice", &upsert.transaction_id)
            .await
            .unwrap();
        transaction.bind_request_digest(b"typed-upsert").unwrap();
        transaction.commit().await.unwrap();
        assert_eq!(
            catalog.read_current().await.unwrap().typed_rows["app.users"]["u-1"].revision,
            3
        );

        let sql_upsert = registry.begin("alice").await.unwrap();
        let response = execute_sql_request(
            &registry,
            "alice",
            SqlTransactionRequest {
                sql: "INSERT INTO product.typed_rows (id, document_json) VALUES ('u-1', '{\"table\":\"app.users\",\"primary_key\":\"u-1\",\"revision\":4,\"owner_principal\":\"alice\",\"columns\":{\"email\":{\"type\":\"string\",\"value\":\"alan@example.com\"}},\"unique_keys\":{\"email\":\"alan@example.com\"}}') ON CONFLICT (id) DO UPDATE SET document_json = excluded.document_json RETURNING *".into(),
                transaction_id: Some(sql_upsert.transaction_id.clone()),
                parameters: vec![],
                named_parameters: HashMap::new(),
            },
        )
        .await;
        let status = response.status();
        let body = axum::body::to_bytes(response.into_body(), 16 * 1024)
            .await
            .unwrap();
        assert_eq!(status, StatusCode::OK, "{}", String::from_utf8_lossy(&body));
        let value: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(value["returning"][0]["id"], "u-1");
        assert_eq!(value["returning"][0]["revision"], 4);
        assert!(
            value["returning"][0]["document_json"]
                .as_str()
                .is_some_and(|document| document.contains("owner_principal"))
        );
        let mut transaction = registry
            .take("alice", &sql_upsert.transaction_id)
            .await
            .unwrap();
        transaction
            .bind_request_digest(b"typed-sql-upsert")
            .unwrap();
        transaction.commit().await.unwrap();

        let sql_update = registry.begin("alice").await.unwrap();
        let response = execute_sql_request(
            &registry,
            "alice",
            SqlTransactionRequest {
                sql: "UPDATE product.typed_rows SET document_json = CAST('{\"table\":\"app.users\",\"primary_key\":\"u-1\",\"revision\":5,\"owner_principal\":\"alice\",\"columns\":{\"email\":{\"type\":\"string\",\"value\":\"ada@example.com\"}},\"unique_keys\":{\"email\":\"ada@example.com\"}}' AS JSON) WHERE id = 'u-1' AND revision = 4 RETURNING *".into(),
                transaction_id: Some(sql_update.transaction_id.clone()),
                parameters: vec![],
                named_parameters: HashMap::new(),
            },
        )
        .await;
        let status = response.status();
        let body = axum::body::to_bytes(response.into_body(), 16 * 1024)
            .await
            .unwrap();
        assert_eq!(status, StatusCode::OK, "{}", String::from_utf8_lossy(&body));
        let value: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(value["returning"][0]["id"], "u-1");
        assert_eq!(value["returning"][0]["revision"], 5);
        assert!(
            value["returning"][0]["document_json"]
                .as_str()
                .is_some_and(|document| document.contains("ada@example.com"))
        );
        let mut transaction = registry
            .take("alice", &sql_update.transaction_id)
            .await
            .unwrap();
        transaction
            .bind_request_digest(b"typed-sql-update")
            .unwrap();
        transaction.commit().await.unwrap();

        let delete_returning = registry.begin("alice").await.unwrap();
        let response = execute_sql_request(
            &registry,
            "alice",
            SqlTransactionRequest {
                sql: "DELETE FROM product.typed_rows WHERE id = 'u-1' AND revision = 5 RETURNING *"
                    .into(),
                transaction_id: Some(delete_returning.transaction_id.clone()),
                parameters: vec![],
                named_parameters: HashMap::new(),
            },
        )
        .await;
        let status = response.status();
        let body = axum::body::to_bytes(response.into_body(), 16 * 1024)
            .await
            .unwrap();
        assert_eq!(status, StatusCode::OK, "{}", String::from_utf8_lossy(&body));
        let value: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(value["returning"][0]["id"], "u-1");
        assert_eq!(value["returning"][0]["revision"], 5);
        assert!(
            value["returning"][0]["document_json"]
                .as_str()
                .is_some_and(|document| document.contains("owner_principal"))
        );

        let rollback = registry.begin("alice").await.unwrap();
        registry
            .stage_product_command(
                "alice",
                &rollback.transaction_id,
                ProductDmlCommand::Delete {
                    kind: "typed_row".into(),
                    id: "u-1".into(),
                    expected_revision: 5,
                },
            )
            .await
            .unwrap();
        let transaction = registry
            .take("alice", &rollback.transaction_id)
            .await
            .unwrap();
        assert_eq!(
            transaction.rollback().typed_rows["app.users"]["u-1"].revision,
            5
        );
        assert_eq!(
            catalog.read_current().await.unwrap().typed_rows["app.users"]["u-1"].revision,
            5
        );
    }

    #[tokio::test]
    async fn system_query_filters_and_orders_rows_from_one_snapshot() {
        let (registry, _) = registry().await;
        for (id, status, rank) in [("a", "ready", 2), ("b", "ready", 9), ("c", "failed", 10)] {
            let begun = registry.begin("admin").await.unwrap();
            registry
                .stage_product_command(
                    "admin",
                    &begun.transaction_id,
                    ProductDmlCommand::Create {
                        kind: "typed_row".into(),
                        id: id.into(),
                        document_json: format!(
                            r#"{{"table":"system.jobs","primary_key":"{id}","revision":1,"owner_principal":"admin","columns":{{"status":{{"type":"string","value":"{status}"}},"rank":{{"type":"integer","value":{rank}}}}},"unique_keys":{{}}}}"#
                        ),
                    },
                )
                .await
                .unwrap();
            let mut transaction = registry.take("admin", &begun.transaction_id).await.unwrap();
            transaction.bind_request_digest(id.as_bytes()).unwrap();
            transaction.commit().await.unwrap();
        }
        let result = registry
            .query_system_rows(
                "system.jobs",
                &SystemQueryRequest {
                    filters: BTreeMap::from([(
                        "status".into(),
                        TypedValue::String("ready".into()),
                    )]),
                    order_by: Some("rank".into()),
                    descending: true,
                    limit: 10,
                },
            )
            .await
            .unwrap();
        assert_eq!(
            result
                .rows
                .iter()
                .map(|row| row.id.as_str())
                .collect::<Vec<_>>(),
            ["b", "a"]
        );
        assert!(
            result.rows.iter().all(|row| {
                row.columns.get("status") == Some(&TypedValue::String("ready".into()))
            })
        );
        assert!(result.next_cursor.is_none());
    }
}
