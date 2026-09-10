//! Authenticated, owner-isolated API sessions for product-catalog transactions.

use std::{
    collections::HashMap,
    sync::Arc,
    time::{Duration, Instant},
};

use axum::{
    Extension, Json, Router,
    extract::{Path, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::{get, post},
};
use kaveon_catalog::{
    product_commit::{CommitOutcome, ProductCatalogCommit},
    product_manifest::{
        CatalogChange, CatalogSnapshot, ImmutableFileRef, ProductRecordKind, ProductRecordRef,
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
const SESSION_TTL: Duration = Duration::from_secs(5 * 60);
type StagedDocument = (String, Vec<u8>);
type ProductStage = (CatalogChange, Option<StagedDocument>);

pub fn routes() -> Router<Arc<AppState>> {
    Router::new()
        .route("/v1/transaction", post(begin))
        .route("/v1/transaction/{transaction_id}/stage", post(stage))
        .route("/v1/transaction/{transaction_id}/commit", post(commit))
        .route("/v1/transaction/{transaction_id}/rollback", post(rollback))
        .route("/v1/transaction/sql", post(execute_sql))
        .route("/v1/product/{kind}/{id}", get(read_product))
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
    Forbidden,
    Conflict,
    Invalid(String),
    Corrupt,
    Unavailable,
}

impl TransactionRegistry {
    pub(crate) fn catalog(&self) -> Option<ProductCatalogCommit> {
        self.catalog.clone()
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
            base,
        })
    }

    async fn stage(
        &self,
        owner: &str,
        id: &str,
        request: StageRequest,
    ) -> Result<CatalogSnapshot, RegistryError> {
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

    async fn take(&self, owner: &str, id: &str) -> Result<ProductTransaction, RegistryError> {
        let mut sessions = self.sessions.lock().await;
        expire(&mut sessions);
        match sessions.get(id) {
            None => return Err(RegistryError::Missing),
            Some(session) if session.owner != owner => return Err(RegistryError::Forbidden),
            Some(_) => {}
        }
        Ok(sessions
            .remove(id)
            .expect("session was checked")
            .transaction)
    }

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

    async fn read_product(
        &self,
        identity: &Identity,
        kind: ProductRecordKind,
        id: &str,
    ) -> Result<ProductReadResponse, RegistryError> {
        let catalog = self.catalog.as_ref().ok_or(RegistryError::Disabled)?;
        let snapshot = catalog.read_current().await.map_err(|error| match error {
            kaveon_storage::CommitErrorKind::Missing | kaveon_storage::CommitErrorKind::Invalid => {
                RegistryError::Corrupt
            }
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
            return Err(RegistryError::Forbidden);
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
            snapshot_id: snapshot.snapshot_id,
            document,
        })
    }
}

fn product_change(
    snapshot: &CatalogSnapshot,
    owner: &str,
    command: kaveon_sql::parser::ProductDmlCommand,
) -> Result<ProductStage, RegistryError> {
    use kaveon_sql::parser::ProductDmlCommand;
    match command {
        ProductDmlCommand::Create {
            kind,
            id,
            document_json,
        } => {
            let kind = record_kind(&kind)?;
            let (document, bytes) = product_document(kind, &id, 1, &document_json)?;
            Ok((
                CatalogChange::CreateProduct {
                    record: ProductRecordRef {
                        kind,
                        id,
                        revision: 1,
                        document,
                        unique_values: BTreeMap::from([("owner_principal".into(), owner.into())]),
                        references: BTreeSet::new(),
                    },
                },
                Some(bytes),
            ))
        }
        ProductDmlCommand::Update {
            kind,
            id,
            expected_revision,
            document_json,
        } => {
            let kind = record_kind(&kind)?;
            let current = snapshot
                .product_record(kind, &id)
                .map_err(|error| RegistryError::Invalid(error.to_string()))?
                .ok_or_else(|| RegistryError::Invalid("product record does not exist".into()))?;
            let revision = expected_revision
                .checked_add(1)
                .ok_or_else(|| RegistryError::Invalid("product revision overflow".into()))?;
            let (document, bytes) = product_document(kind, &id, revision, &document_json)?;
            Ok((
                CatalogChange::UpdateProduct {
                    expected_revision,
                    record: ProductRecordRef {
                        document,
                        revision,
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
        } => Ok((
            CatalogChange::DeleteProduct {
                kind: record_kind(&kind)?,
                id,
                expected_revision,
            },
            None,
        )),
    }
}

fn record_kind(kind: &str) -> Result<ProductRecordKind, RegistryError> {
    match kind {
        "dataset" => Ok(ProductRecordKind::Dataset),
        "chart" => Ok(ProductRecordKind::Chart),
        "dashboard" => Ok(ProductRecordKind::Dashboard),
        "saved_query" => Ok(ProductRecordKind::SavedQuery),
        "user_theme" => Ok(ProductRecordKind::UserTheme),
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
) -> Result<(ImmutableFileRef, (String, Vec<u8>)), RegistryError> {
    let value: serde_json::Value = serde_json::from_str(document_json)
        .map_err(|_| RegistryError::Invalid("document_json must be valid JSON".into()))?;
    if !value.is_object() {
        return Err(RegistryError::Invalid(
            "document_json must be a JSON object".into(),
        ));
    }
    let bytes = serde_json::to_vec(&value)
        .map_err(|_| RegistryError::Invalid("document_json cannot be encoded".into()))?;
    let sha256 = format!("{:x}", Sha256::digest(&bytes));
    let kind_name = match kind {
        ProductRecordKind::Dataset => "dataset",
        ProductRecordKind::Chart => "chart",
        ProductRecordKind::Dashboard => "dashboard",
        ProductRecordKind::SavedQuery => "saved_query",
        ProductRecordKind::UserTheme => "user_theme",
    };
    let path = format!("products/{kind_name}/{id}/{revision}-{sha256}.json");
    Ok((
        ImmutableFileRef {
            path: path.clone(),
            sha256,
        },
        (path, bytes),
    ))
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
        Some(session) if session.owner != owner => Err(RegistryError::Forbidden),
        Some(session) => Ok(session),
    }
}

#[derive(Debug, Serialize)]
struct BeginResponse {
    transaction_id: String,
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
struct SqlTransactionRequest {
    sql: String,
    #[serde(default)]
    transaction_id: Option<String>,
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

async fn execute_sql(
    State(state): State<Arc<AppState>>,
    Extension(identity): Extension<Identity>,
    Json(request): Json<SqlTransactionRequest>,
) -> Response {
    execute_sql_request(&state.product_transactions, &identity.principal, request).await
}

async fn execute_sql_request(
    registry: &TransactionRegistry,
    owner: &str,
    request: SqlTransactionRequest,
) -> Response {
    use kaveon_sql::parser::{
        NativeTransactionalStatement, adapt_product_dml, parse_native_transactional,
    };
    let parsed = match parse_native_transactional(&request.sql) {
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
        NativeTransactionalStatement::Dml(dml) => {
            let Some(id) = request.transaction_id else {
                return error_response(RegistryError::Invalid("transaction_id is required".into()));
            };
            let command = match adapt_product_dml(&dml) {
                Ok(command) => command,
                Err(error) => return error_response(RegistryError::Invalid(error.to_string())),
            };
            match registry.stage_product_command(owner, &id, command).await {
                Ok(snapshot) => Json(snapshot).into_response(),
                Err(error) => error_response(error),
            }
        }
    }
}

async fn commit_transaction(registry: &TransactionRegistry, owner: &str, id: &str) -> Response {
    let mut transaction = match registry.take(owner, id).await {
        Ok(transaction) => transaction,
        Err(error) => return error_response(error),
    };
    if let Err(error) = transaction.bind_request_digest(owner.as_bytes()) {
        return error_response(RegistryError::Invalid(error.to_string()));
    }
    match transaction.commit().await {
        Ok(CommitOutcome::Committed(snapshot) | CommitOutcome::Replayed(snapshot)) => Json(snapshot).into_response(),
        Ok(CommitOutcome::Conflict) => error_response(RegistryError::Conflict),
        Ok(CommitOutcome::Rejected) => error_response(RegistryError::Invalid("transaction rejected".into())),
        Ok(CommitOutcome::Indeterminate) => (StatusCode::SERVICE_UNAVAILABLE, Json(serde_json::json!({"error":"transaction outcome is indeterminate; resolve the operation before retrying"}))).into_response(),
        Err(error) => error_response(RegistryError::Invalid(error.to_string())),
    }
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
    match transaction.commit().await {
        Ok(CommitOutcome::Committed(snapshot) | CommitOutcome::Replayed(snapshot)) => Json(snapshot).into_response(),
        Ok(CommitOutcome::Conflict) => error_response(RegistryError::Conflict),
        Ok(CommitOutcome::Rejected) => error_response(RegistryError::Invalid("transaction rejected".into())),
        Ok(CommitOutcome::Indeterminate) => (StatusCode::SERVICE_UNAVAILABLE, Json(serde_json::json!({"error":"transaction outcome is indeterminate; resolve the operation before retrying"}))).into_response(),
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
        RegistryError::Forbidden => (
            StatusCode::FORBIDDEN,
            "transaction session belongs to another principal".to_owned(),
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
            RegistryError::Forbidden
        );
        assert_eq!(
            registry
                .take("bob", &begun.transaction_id)
                .await
                .err()
                .unwrap(),
            RegistryError::Forbidden
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

    #[tokio::test]
    async fn sql_endpoint_dispatches_product_dml_and_commit_to_owner_session() {
        let (registry, catalog) = registry().await;
        let begun = registry.begin("alice").await.unwrap();
        let response = execute_sql_request(&registry, "alice", SqlTransactionRequest {
            sql: r#"INSERT INTO kaveon.product.datasets (id, document_json) VALUES ('orders', '{"name":"Orders"}')"#.into(),
            transaction_id: Some(begun.transaction_id.clone()),
        }).await;
        assert_eq!(response.status(), StatusCode::OK);
        let response = execute_sql_request(
            &registry,
            "alice",
            SqlTransactionRequest {
                sql: "COMMIT".into(),
                transaction_id: Some(begun.transaction_id),
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
    async fn sql_endpoint_rejects_generic_row_dml_without_staging() {
        let (registry, _) = registry().await;
        let begun = registry.begin("alice").await.unwrap();
        let response = execute_sql_request(
            &registry,
            "alice",
            SqlTransactionRequest {
                sql: "DELETE FROM app.users WHERE id = 'alice'".into(),
                transaction_id: Some(begun.transaction_id.clone()),
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
            RegistryError::Forbidden
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
}
