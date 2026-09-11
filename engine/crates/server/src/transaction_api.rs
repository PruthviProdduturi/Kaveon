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
        ProductRecordReference,
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
    }
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
        return Err(RegistryError::Forbidden);
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
        Err(RegistryError::Forbidden)
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
    let references = if matches!(kind,ProductRecordKind::ChatSession|ProductRecordKind::ChatMessage) {
        let object=value.as_object().expect("object checked above");
        let owner=object.get("user_email").and_then(serde_json::Value::as_str).filter(|v|!v.is_empty()).ok_or_else(||RegistryError::Invalid("chat owner is invalid".into()))?;
        if object.get("id").and_then(serde_json::Value::as_str)!=Some(id) { return Err(RegistryError::Invalid("chat record ID is invalid".into())); }
        derived_values.insert("chat_owner".into(),owner.into());
        if kind==ProductRecordKind::ChatSession {
            const ALLOWED:[&str;5]=["id","user_email","title","created_at","updated_at"];
            if object.keys().any(|k|!ALLOWED.contains(&k.as_str())) || object.get("title").and_then(serde_json::Value::as_str).is_none() { return Err(RegistryError::Invalid("chat session document is invalid".into())); } BTreeSet::new()
        } else {
            const ALLOWED:[&str;10]=["id","session_id","user_email","role","content","sql_query","chart_type","data","route","created_at"];
            if object.keys().any(|k|!ALLOWED.contains(&k.as_str())) || !matches!(object.get("role").and_then(serde_json::Value::as_str),Some("user"|"assistant")) || object.get("content").and_then(serde_json::Value::as_str).is_none() { return Err(RegistryError::Invalid("chat message document is invalid".into())); }
            let session=object.get("session_id").and_then(serde_json::Value::as_str).filter(|v|!v.is_empty()).ok_or_else(||RegistryError::Invalid("chat session reference is invalid".into()))?;BTreeSet::from([ProductRecordReference{kind:ProductRecordKind::ChatSession,id:session.into()}])
        }
    } else if kind == ProductRecordKind::Activity {
        let object=value.as_object().expect("object checked above");
        const ALLOWED:[&str;8]=["id","action","object_type","object_id","object_name","timestamp","user_email","details"];
        if object.keys().any(|key|!ALLOWED.contains(&key.as_str())) || object.get("id").and_then(serde_json::Value::as_str)!=Some(id) { return Err(RegistryError::Invalid("activity document schema is invalid".into())); }
        let actor=object.get("user_email").and_then(serde_json::Value::as_str).filter(|v|!v.is_empty()).ok_or_else(||RegistryError::Invalid("activity actor is invalid".into()))?;
        for field in ["action","object_type","object_id","object_name","timestamp"] { if object.get(field).and_then(serde_json::Value::as_str).is_none_or(|v|v.is_empty()) { return Err(RegistryError::Invalid("activity document is invalid".into())); } }
        if object.get("details").is_some_and(|v|!v.is_null()&&!v.is_object()) { return Err(RegistryError::Invalid("activity details must be an object".into())); }
        derived_values.insert("activity_actor".into(),actor.into());BTreeSet::new()
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
            id: item.into(),
        }])
    } else if kind == ProductRecordKind::Source {
        let object = value.as_object().expect("object checked above");
        const ALLOWED: [&str; 12] = [
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
            Err(RegistryError::Forbidden)
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
            Err(RegistryError::Forbidden)
        );
    }

    #[test]
    fn query_history_is_owner_isolated_and_binds_optional_dataset() {
        let (_,_,references,values)=product_document(ProductRecordKind::QueryHistory,"q1",1,
            r#"{"id":"q1","sql_text":"SELECT 1","database_name":"db","executed_at":"2026-01-01T00:00:00","execution_time":1,"row_count":1,"status":"success","error_message":null,"user_email":"alice","trigger_source":"lab","dataset_id":"ds","tables_used":"[]"}"#).unwrap();
        assert_eq!(values["query_owner"],"alice");
        assert_eq!(references,BTreeSet::from([ProductRecordReference{kind:ProductRecordKind::Dataset,id:"ds".into()}]));
        assert!(validate_favorite_owner(ProductRecordKind::QueryHistory,"alice",&values).is_ok());
        assert_eq!(validate_favorite_owner(ProductRecordKind::QueryHistory,"bob",&values),Err(RegistryError::Forbidden));
    }

    #[test]
    fn activity_is_actor_isolated_and_rejects_unstructured_details() {
        let (_,_,references,values)=product_document(ProductRecordKind::Activity,"a1",1,
            r#"{"id":"a1","action":"created","object_type":"catalog_source","object_id":"c1","object_name":"Lake","timestamp":"2026-01-01T00:00:00","user_email":"alice","details":{"storage_type":"adls_gen2"}}"#).unwrap();
        assert!(references.is_empty());assert_eq!(values["activity_actor"],"alice");
        assert!(validate_favorite_owner(ProductRecordKind::Activity,"alice",&values).is_ok());
        assert_eq!(validate_favorite_owner(ProductRecordKind::Activity,"bob",&values),Err(RegistryError::Forbidden));
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
            Err(RegistryError::Forbidden)
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
            Err(RegistryError::Forbidden)
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
            Err(RegistryError::Forbidden)
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
                RegistryError::Forbidden
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
