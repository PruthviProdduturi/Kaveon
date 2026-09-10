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
    routing::post,
};
use kaveon_catalog::{
    product_commit::{CommitOutcome, ProductCatalogCommit},
    product_manifest::{CatalogChange, CatalogSnapshot},
    product_transaction::ProductTransaction,
};
use serde::{Deserialize, Serialize};
use tokio::sync::Mutex;
use uuid::Uuid;

use crate::{AppState, security::Identity};

const MAX_SESSIONS: usize = 64;
const MAX_SESSIONS_PER_PRINCIPAL: usize = 8;
const MAX_SESSION_DOCUMENT_BYTES: usize = 16 * 1024 * 1024;
const SESSION_TTL: Duration = Duration::from_secs(5 * 60);

pub fn routes() -> Router<Arc<AppState>> {
    Router::new()
        .route("/v1/transaction", post(begin))
        .route("/v1/transaction/{transaction_id}/stage", post(stage))
        .route("/v1/transaction/{transaction_id}/commit", post(commit))
        .route("/v1/transaction/{transaction_id}/rollback", post(rollback))
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
    Forbidden,
    Conflict,
    Invalid(String),
}

impl TransactionRegistry {
    pub fn disabled() -> Self {
        Self {
            catalog: None,
            sessions: Arc::new(Mutex::new(HashMap::new())),
            ttl: SESSION_TTL,
        }
    }

    #[cfg(test)]
    fn enabled(catalog: ProductCatalogCommit) -> Self {
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
        RegistryError::Forbidden => (
            StatusCode::FORBIDDEN,
            "transaction session belongs to another principal".to_owned(),
        ),
        RegistryError::Conflict => (StatusCode::CONFLICT, "transaction conflict".to_owned()),
        RegistryError::Invalid(message) => (StatusCode::BAD_REQUEST, message),
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
}
