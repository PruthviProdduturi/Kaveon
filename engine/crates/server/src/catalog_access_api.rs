//! The catalog access routes: administration of the grants family, the
//! effective view per principal, the reconciliation of an open deployment
//! and a caller's own standing.
//!
//! Every route reads the identity the security layer attached; nothing in
//! a request body names the actor or their role.
use std::{collections::BTreeMap, sync::Arc};

use axum::{
    Extension, Json,
    extract::{Path, State},
    http::StatusCode,
    response::{IntoResponse, Response},
};
use serde::Deserialize;
use serde_json::{Value, json};

use crate::{
    AppState,
    audit::{AuditFilter, KIND_STATEMENT_SUBMITTED, MAX_PAGE},
    catalog_access::{
        Access, AccessError, GrantRequest, RESERVED_CATALOG, is_reserved, kaveondb_views_available,
        role_ceiling,
    },
    security::{Identity, Role},
};

fn forbidden() -> Response {
    (
        StatusCode::FORBIDDEN,
        Json(json!({
            "error": "catalog access is managed by administrators",
            "code": "FORBIDDEN"
        })),
    )
        .into_response()
}

fn not_coordinator() -> Response {
    (
        StatusCode::BAD_REQUEST,
        Json(json!({
            "error": "catalog access is managed on the coordinator",
            "code": "NOT_COORDINATOR"
        })),
    )
        .into_response()
}

fn admin_on_coordinator(state: &AppState, identity: &Identity) -> Result<(), Box<Response>> {
    if identity.role != Role::Admin {
        return Err(Box::new(forbidden()));
    }
    if !state.config.coordinator {
        return Err(Box::new(not_coordinator()));
    }
    Ok(())
}

fn error_response(error: AccessError) -> Response {
    let (status, code) = match &error {
        AccessError::Disabled => (StatusCode::SERVICE_UNAVAILABLE, "ACCESS_STORE_DISABLED"),
        AccessError::Reserved => (StatusCode::BAD_REQUEST, "RESERVED_CATALOG"),
        AccessError::Invalid(_) => (StatusCode::BAD_REQUEST, "INVALID_GRANT"),
        AccessError::Conflict(_) => (StatusCode::CONFLICT, "REVISION_CONFLICT"),
        AccessError::Unavailable(_) => {
            (StatusCode::SERVICE_UNAVAILABLE, "ACCESS_STORE_UNAVAILABLE")
        }
        AccessError::Indeterminate => (
            StatusCode::SERVICE_UNAVAILABLE,
            "ACCESS_OUTCOME_INDETERMINATE",
        ),
    };
    (
        status,
        Json(json!({ "error": error.to_string(), "code": code })),
    )
        .into_response()
}

/// The registered catalogs a grant may name: everything published except
/// the reserved authority.
async fn grantable_catalogs(state: &AppState) -> Vec<String> {
    let mut names: Vec<String> = state
        .catalog
        .read()
        .await
        .registered_catalog_names()
        .into_iter()
        .filter(|name| !is_reserved(name))
        .collect();
    names.sort();
    names
}

fn role_ceilings() -> Value {
    json!({
        "reader": role_ceiling(Role::Reader),
        "analyst": role_ceiling(Role::Analyst),
        "admin": "all",
    })
}

fn reserved() -> Value {
    json!([{
        "name": RESERVED_CATALOG,
        "grantable": false,
        "visible_to": if kaveondb_views_available() { "admin" } else { "none" },
        "reason": if kaveondb_views_available() {
            "read-only product and catalog views for administrators; system is administrators only"
        } else {
            "its read-only views are not available yet; hidden from every role until they are"
        },
    }])
}

/// The effective level of each Engine role on a grant of `access`.
fn effective_by_role(access: Access) -> Value {
    json!({
        "reader": role_ceiling(Role::Reader).map(|ceiling| access.min(ceiling)),
        "analyst": role_ceiling(Role::Analyst).map(|ceiling| access.min(ceiling)),
        "admin": "all",
    })
}

async fn document(state: &AppState) -> Value {
    let grants = state.catalog_access.current();
    json!({
        "store": {
            "enabled": state.catalog_access.is_enabled(),
            "generation": grants.generation,
            "snapshot_id": grants.snapshot_id,
        },
        "catalogs": grantable_catalogs(state).await,
        "reserved": reserved(),
        "roles": role_ceilings(),
        "grants": grants.iter().collect::<Vec<_>>(),
    })
}

/// `GET /v1/admin/catalog-access`: the store's standing, the grantable
/// catalogs, the reserved authority, the role ceilings and every grant.
pub async fn get_catalog_access(
    State(state): State<Arc<AppState>>,
    Extension(identity): Extension<Identity>,
) -> Response {
    if let Err(response) = admin_on_coordinator(&state, &identity) {
        return *response;
    }
    Json(document(&state).await).into_response()
}

/// `PUT /v1/admin/catalog-access/grants`: create a grant, or change one at
/// the revision the body names.
pub async fn put_grant(
    State(state): State<Arc<AppState>>,
    Extension(identity): Extension<Identity>,
    body: axum::body::Bytes,
) -> Response {
    if let Err(response) = admin_on_coordinator(&state, &identity) {
        return *response;
    }
    let request: GrantRequest = match serde_json::from_slice(&body) {
        Ok(request) => request,
        Err(error) => {
            return (
                StatusCode::BAD_REQUEST,
                Json(json!({
                    "error": format!("catalog grant: {error}"),
                    "code": "INVALID_GRANT"
                })),
            )
                .into_response();
        }
    };
    if is_reserved(&request.catalog) {
        return error_response(AccessError::Reserved);
    }
    if !grantable_catalogs(&state)
        .await
        .iter()
        .any(|name| name == &request.catalog)
    {
        return (
            StatusCode::NOT_FOUND,
            Json(json!({
                "error": format!("catalog '{}' not found", request.catalog),
                "code": "CATALOG_NOT_FOUND"
            })),
        )
            .into_response();
    }
    match state
        .catalog_access
        .grant(&state.audit, &identity, request)
        .await
    {
        Ok(outcome) => Json(json!({
            "grant": outcome.grant,
            "revision_before": outcome.revision_before,
            "generation": outcome.generation,
            "effective": effective_by_role(outcome.grant.access),
        }))
        .into_response(),
        Err(error) => error_response(error),
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RevokeRequest {
    pub principal: String,
    pub catalog: String,
    pub revision: u64,
}

/// `DELETE /v1/admin/catalog-access/grants`: remove a grant at the
/// revision the body names.
pub async fn delete_grant(
    State(state): State<Arc<AppState>>,
    Extension(identity): Extension<Identity>,
    body: axum::body::Bytes,
) -> Response {
    if let Err(response) = admin_on_coordinator(&state, &identity) {
        return *response;
    }
    let request: RevokeRequest = match serde_json::from_slice(&body) {
        Ok(request) => request,
        Err(error) => {
            return (
                StatusCode::BAD_REQUEST,
                Json(json!({
                    "error": format!("catalog revoke: {error}"),
                    "code": "INVALID_GRANT"
                })),
            )
                .into_response();
        }
    };
    match state
        .catalog_access
        .revoke(
            &state.audit,
            &identity,
            &request.principal,
            &request.catalog,
            request.revision,
        )
        .await
    {
        Ok(revoked) => Json(json!({
            "revoked": revoked,
            "generation": state.catalog_access.current().generation,
        }))
        .into_response(),
        Err(error) => error_response(error),
    }
}

/// `GET /v1/admin/catalog-access/effective/{principal}`: what each Engine
/// role would reach on the principal's grants. Roles are not stored with
/// the grants — the identity provider assigns them — so the view is given
/// per role.
pub async fn get_effective(
    State(state): State<Arc<AppState>>,
    Extension(identity): Extension<Identity>,
    Path(principal): Path<String>,
) -> Response {
    if let Err(response) = admin_on_coordinator(&state, &identity) {
        return *response;
    }
    let grants = state.catalog_access.current();
    let registered = grantable_catalogs(&state).await;
    let rows: Vec<Value> = grants
        .for_principal(&principal)
        .map(|grant| {
            json!({
                "catalog": grant.catalog,
                "access": grant.access,
                "revision": grant.revision,
                "granted_by": grant.granted_by,
                "granted_at_ms": grant.granted_at_ms,
                "registered": registered.contains(&grant.catalog),
                "effective": effective_by_role(grant.access),
            })
        })
        .collect();
    Json(json!({
        "principal": principal,
        "store_enabled": state.catalog_access.is_enabled(),
        "grants": rows,
        "ungranted": registered
            .iter()
            .filter(|name| grants.get(&principal, name).is_none())
            .collect::<Vec<_>>(),
        "reserved": reserved(),
        "roles": role_ceilings(),
    }))
    .into_response()
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ImportRequest {
    /// The policy to record. Only `open` exists: what every principal had
    /// before grants, every catalog at their role's ceiling.
    pub source: String,
    /// Record the proposal; without it the response is the proposal only.
    #[serde(default)]
    pub apply: bool,
}

/// `POST /v1/admin/catalog-access/import`: the reconciliation of a
/// deployment that ran open. The audit ledger's submitted statements name
/// every principal that used the coordinator and the role they held; the
/// open policy for them is every grantable catalog at that role's ceiling.
/// The proposal is returned as-is; it is recorded only with `apply`, and
/// then only for pairs with no grant yet. Admins are never proposed: their
/// access is the role's. A ledger that is off proposes nothing.
pub async fn import_open_policy(
    State(state): State<Arc<AppState>>,
    Extension(identity): Extension<Identity>,
    body: axum::body::Bytes,
) -> Response {
    if let Err(response) = admin_on_coordinator(&state, &identity) {
        return *response;
    }
    let request: ImportRequest = match serde_json::from_slice(&body) {
        Ok(request) => request,
        Err(error) => {
            return (
                StatusCode::BAD_REQUEST,
                Json(json!({
                    "error": format!("catalog access import: {error}"),
                    "code": "INVALID_IMPORT"
                })),
            )
                .into_response();
        }
    };
    if request.source != "open" {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({
                "error": "the only import source is 'open': the policy every principal had before grants",
                "code": "INVALID_IMPORT"
            })),
        )
            .into_response();
    }
    // Every principal the ledger saw submit a statement, with the highest
    // non-admin role it held.
    let mut seen: BTreeMap<String, Role> = BTreeMap::new();
    let mut filter = AuditFilter {
        kinds: vec![KIND_STATEMENT_SUBMITTED.into()],
        ..Default::default()
    };
    loop {
        let page = match state.audit.query(&filter, MAX_PAGE) {
            Ok(page) => page,
            Err(error) => {
                return (
                    StatusCode::SERVICE_UNAVAILABLE,
                    Json(json!({
                        "error": format!("cannot read the audit ledger: {error}"),
                        "code": "AUDIT_UNAVAILABLE"
                    })),
                )
                    .into_response();
            }
        };
        for record in &page.records {
            let (Some(principal), Some(role)) = (&record.principal, record.role) else {
                continue;
            };
            if role == Role::Admin || principal == "internal" {
                continue;
            }
            let entry = seen.entry(principal.clone()).or_insert(role);
            if role_ceiling(role) > role_ceiling(*entry) {
                *entry = role;
            }
        }
        match page.next_cursor {
            Some(cursor) => filter.after_seq = Some(cursor),
            None => break,
        }
    }
    let catalogs = grantable_catalogs(&state).await;
    let existing = state.catalog_access.current();
    let proposals: Vec<(String, Role, String, Access)> = seen
        .iter()
        .flat_map(|(principal, role)| {
            let access = role_ceiling(*role).unwrap_or(Access::Browse);
            catalogs
                .iter()
                .filter(|catalog| existing.get(principal, catalog).is_none())
                .map(move |catalog| (principal.clone(), *role, catalog.clone(), access))
        })
        .collect();
    let proposed: Vec<Value> = proposals
        .iter()
        .map(|(principal, role, catalog, access)| {
            json!({
                "principal": principal,
                "role_seen": role,
                "catalog": catalog,
                "access": access,
            })
        })
        .collect();
    let mut response = json!({
        "source": "open",
        "ledger_enabled": state.audit.is_enabled(),
        "principals_seen": seen.len(),
        "catalogs": catalogs,
        "proposed": proposed,
        "applied": false,
        "recorded": [],
    });
    if !request.apply {
        return Json(response).into_response();
    }
    let requests = proposals
        .into_iter()
        .map(|(principal, _, catalog, access)| GrantRequest {
            principal,
            catalog,
            access,
            revision: None,
        })
        .collect();
    match state
        .catalog_access
        .grant_many(&state.audit, &identity, requests)
        .await
    {
        Ok(recorded) => {
            response["applied"] = json!(true);
            response["recorded"] = json!(recorded);
            response["generation"] = json!(state.catalog_access.current().generation);
            Json(response).into_response()
        }
        Err(error) => error_response(error),
    }
}

/// `GET /v1/catalog-access/me`: the caller's own standing — the catalogs
/// they may reach and at what level. Informational; nothing here grants.
pub async fn get_my_access(
    State(state): State<Arc<AppState>>,
    Extension(identity): Extension<Identity>,
) -> Response {
    let scope = state.catalog_access.evaluate(&identity);
    let registered = state.catalog.read().await.registered_catalog_names();
    let mut catalogs: Vec<Value> = registered
        .iter()
        .filter_map(|name| {
            scope
                .level(name)
                .map(|level| json!({ "catalog": name, "access": level }))
        })
        .collect();
    catalogs.sort_by(|a, b| a["catalog"].as_str().cmp(&b["catalog"].as_str()));
    Json(json!({
        "principal": identity.principal,
        "role": identity.role,
        "store_enabled": state.catalog_access.is_enabled(),
        "catalogs": catalogs,
    }))
    .into_response()
}

#[cfg(test)]
mod tests {
    //! Every enforcement point, through the real router: identity comes
    //! from the bearer token the security layer resolves, never from a
    //! body, and the same policy answers the catalog list, discovery, the
    //! definitions API, the metadata statements and a hand-written
    //! statement.
    use std::sync::Arc;

    use arrow::array::Int64Array;
    use arrow::datatypes::{DataType, Field, Schema};
    use arrow::record_batch::RecordBatch;
    use axum::http::StatusCode;
    use serde_json::{Value, json};

    use crate::catalog_access::{CatalogAccess, tests::memory_store};
    use crate::security::{PrincipalCredential, Role};

    const ADMIN: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
    const ANALYST: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";
    const READER: &str = "cccccccccccccccccccccccccccccccc";
    const BROWSER: &str = "dddddddddddddddddddddddddddddddd";
    const SERVICE: &str = "admin-token";

    struct Engine {
        url: String,
        state: Arc<crate::AppState>,
        client: reqwest::Client,
        directory: std::path::PathBuf,
        server: tokio::task::JoinHandle<()>,
    }

    impl Engine {
        /// A coordinator with two Parquet catalogs, `lake` and `secret`,
        /// each holding `sales.orders`; an in-memory grants authority; an
        /// audit ledger; four static principals. Nothing is granted yet.
        async fn start() -> Engine {
            let directory = std::env::temp_dir()
                .join(format!("kaveon-catalog-access-{}", uuid::Uuid::new_v4()));
            std::fs::create_dir_all(&directory).unwrap();
            let schema = Arc::new(Schema::new(vec![Field::new("id", DataType::Int64, false)]));
            let batch = RecordBatch::try_new(
                schema.clone(),
                vec![Arc::new(Int64Array::from(vec![1, 2, 3]))],
            )
            .unwrap();
            let mut writer = parquet::arrow::ArrowWriter::try_new(
                std::fs::File::create(directory.join("orders.parquet")).unwrap(),
                schema,
                None,
            )
            .unwrap();
            writer.write(&batch).unwrap();
            writer.close().unwrap();

            let mut state = crate::api::catalog_test_state();
            let credential = |token: &str, principal: &str, role: Role| PrincipalCredential {
                token: token.into(),
                principal: principal.into(),
                role,
            };
            state.config.security.principals = vec![
                credential(ADMIN, "root", Role::Admin),
                credential(ANALYST, "ana", Role::Analyst),
                credential(READER, "ray", Role::Reader),
                credential(BROWSER, "bea", Role::Analyst),
            ];
            state.audit = crate::audit::AuditLedger::open(
                &directory.join("audit"),
                1 << 20,
                std::time::Duration::from_secs(60),
            )
            .unwrap();
            state.catalog_access = CatalogAccess::open(memory_store().await).await.unwrap();
            for name in ["lake", "secret"] {
                let catalog = kaveon_core::CatalogDefinition::new(
                    kaveon_core::CatalogId::new(format!("catalog:{name}")).unwrap(),
                    name,
                    kaveon_core::CatalogAdapter::Native,
                    kaveon_core::StorageType::Local {
                        base_path: directory.clone(),
                    },
                )
                .unwrap()
                .transition(kaveon_core::CatalogLifecycle::Active)
                .unwrap();
                state
                    .catalog_store
                    .create_catalog("test", &catalog)
                    .unwrap();
                let schema = kaveon_core::SchemaDefinition::new(
                    kaveon_core::SchemaId::new(format!("schema:{name}:sales")).unwrap(),
                    kaveon_core::CatalogId::new(format!("catalog:{name}")).unwrap(),
                    "sales",
                )
                .unwrap()
                .transition(kaveon_core::CatalogLifecycle::Active)
                .unwrap();
                state.catalog_store.create_schema("test", &schema).unwrap();
                let table = kaveon_core::TableDefinition::new(
                    kaveon_core::TableId::new(format!("table:{name}:sales:orders")).unwrap(),
                    kaveon_core::SchemaId::new(format!("schema:{name}:sales")).unwrap(),
                    "orders",
                    "orders.parquet",
                    kaveon_core::AccessPattern::Shortcut,
                    kaveon_core::DataFormat::Parquet,
                    vec![kaveon_core::ColumnDefinition::new("id", DataType::Int64, false).unwrap()],
                )
                .unwrap()
                .transition(kaveon_core::CatalogLifecycle::Active)
                .unwrap();
                state.catalog_store.create_table("test", &table).unwrap();
            }
            crate::api::publish_catalog_snapshot(&state).await.unwrap();
            let state = Arc::new(state);
            let app = crate::api::build_router(Arc::clone(&state));
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let url = format!("http://{}", listener.local_addr().unwrap());
            let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
            Engine {
                url,
                state,
                client: reqwest::Client::new(),
                directory,
                server,
            }
        }

        async fn get(&self, token: &str, path: &str) -> (StatusCode, Value) {
            let response = self
                .client
                .get(format!("{}{path}", self.url))
                .bearer_auth(token)
                .send()
                .await
                .unwrap();
            let status = response.status();
            let body = response.text().await.unwrap();
            (status, serde_json::from_str(&body).unwrap_or(Value::Null))
        }

        async fn send(
            &self,
            method: reqwest::Method,
            token: &str,
            path: &str,
            body: Value,
        ) -> (StatusCode, Value) {
            let response = self
                .client
                .request(method, format!("{}{path}", self.url))
                .bearer_auth(token)
                .json(&body)
                .send()
                .await
                .unwrap();
            let status = response.status();
            let body = response.text().await.unwrap();
            (status, serde_json::from_str(&body).unwrap_or(Value::Null))
        }

        /// A statement in the session catalog `catalog`.
        async fn statement(&self, token: &str, catalog: &str, sql: &str) -> (StatusCode, Value) {
            self.send(
                reqwest::Method::POST,
                token,
                "/v1/statement",
                json!({ "query": sql, "catalog": catalog, "schema": "sales" }),
            )
            .await
        }

        async fn grant(
            &self,
            token: &str,
            principal: &str,
            catalog: &str,
            access: &str,
            revision: Option<u64>,
        ) -> (StatusCode, Value) {
            let mut body = json!({ "principal": principal, "catalog": catalog, "access": access });
            if let Some(revision) = revision {
                body["revision"] = json!(revision);
            }
            self.send(
                reqwest::Method::PUT,
                token,
                "/v1/admin/catalog-access/grants",
                body,
            )
            .await
        }

        async fn revoke(
            &self,
            token: &str,
            principal: &str,
            catalog: &str,
            revision: u64,
        ) -> (StatusCode, Value) {
            self.send(
                reqwest::Method::DELETE,
                token,
                "/v1/admin/catalog-access/grants",
                json!({ "principal": principal, "catalog": catalog, "revision": revision }),
            )
            .await
        }

        fn stop(self) {
            self.server.abort();
            self.state.audit.shutdown();
            let _ = std::fs::remove_dir_all(self.directory);
        }
    }

    fn names(body: &Value, key: &str) -> Vec<String> {
        let mut names: Vec<String> = body[key]
            .as_array()
            .unwrap_or(&Vec::new())
            .iter()
            .map(|value| value.as_str().unwrap().to_owned())
            .collect();
        names.sort();
        names
    }

    fn definition_names(body: &Value) -> Vec<String> {
        let mut names: Vec<String> = body
            .as_array()
            .unwrap_or(&Vec::new())
            .iter()
            .map(|value| value["name"].as_str().unwrap().to_owned())
            .collect();
        names.sort();
        names
    }

    #[tokio::test]
    async fn default_deny_then_grants_open_exactly_the_granted_catalog_at_every_point() {
        let engine = Engine::start().await;

        // Nothing granted: the Admin sees both catalogs, nobody else sees
        // one — on the list, on discovery, in the definitions API, in the
        // metadata statements and in a statement.
        let (_, body) = engine.get(ADMIN, "/v1/catalog").await;
        assert_eq!(names(&body, "catalogs"), ["lake", "secret"]);
        for token in [ANALYST, READER] {
            let (status, body) = engine.get(token, "/v1/catalog").await;
            assert_eq!(status, StatusCode::OK);
            assert!(names(&body, "catalogs").is_empty(), "{body}");
            let (status, body) = engine.get(token, "/v1/catalog/definitions").await;
            assert_eq!(status, StatusCode::OK);
            assert!(definition_names(&body).is_empty(), "{body}");
            let (status, _) = engine
                .get(token, "/v1/catalog/definitions/catalog:lake")
                .await;
            assert_eq!(status, StatusCode::NOT_FOUND);
            let (status, _) = engine
                .get(token, "/v1/catalog/definitions/catalog:lake/schemas")
                .await;
            assert_eq!(status, StatusCode::NOT_FOUND);
            let (status, _) = engine
                .get(token, "/v1/catalog/schemas/schema:lake:sales/tables")
                .await;
            assert_eq!(status, StatusCode::NOT_FOUND);
            let (status, _) = engine
                .get(token, "/v1/catalog/tables/table:lake:sales:orders")
                .await;
            assert_eq!(status, StatusCode::NOT_FOUND);
        }
        // The catalog service credential is the platform's own and is not a
        // principal: it keeps the whole view.
        let (status, body) = engine.get(SERVICE, "/v1/catalog/definitions").await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(definition_names(&body), ["lake", "secret"]);

        // Grant the analyst `lake`: it appears everywhere; `secret` does not.
        let (status, body) = engine.grant(ADMIN, "ana", "lake", "query", None).await;
        assert_eq!(status, StatusCode::OK, "{body}");
        assert_eq!(body["grant"]["revision"], 1);
        assert_eq!(body["effective"]["analyst"], "query");
        assert_eq!(body["effective"]["reader"], "browse");
        let (_, body) = engine.get(ANALYST, "/v1/catalog").await;
        assert_eq!(names(&body, "catalogs"), ["lake"]);
        let (status, body) = engine.get(ANALYST, "/v1/catalog/lake/schema").await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(names(&body, "schemas"), ["sales"]);
        let (status, body) = engine
            .get(ANALYST, "/v1/catalog/lake/schema/sales/table")
            .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(names(&body, "tables"), ["orders"]);
        let (status, body) = engine.get(ANALYST, "/v1/catalog/definitions").await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(definition_names(&body), ["lake"]);
        assert_eq!(body[0]["access"], "query");
        let (status, _) = engine
            .get(ANALYST, "/v1/catalog/tables/table:lake:sales:orders")
            .await;
        assert_eq!(status, StatusCode::OK);
        let (status, body) = engine.statement(ANALYST, "lake", "SHOW CATALOGS").await;
        assert_eq!(status, StatusCode::OK, "{body}");
        assert_eq!(body["data"], json!([["lake"]]));
        let (status, body) = engine
            .statement(ANALYST, "lake", "SELECT id FROM orders ORDER BY id")
            .await;
        assert_eq!(status, StatusCode::OK, "{body}");
        assert_eq!(body["data"], json!([[1], [2], [3]]));
        let (status, body) = engine.get(ANALYST, "/v1/catalog-access/me").await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(
            body["catalogs"],
            json!([{ "catalog": "lake", "access": "query" }])
        );

        // A reader with the same grant browses: discovery, not statements.
        engine.grant(ADMIN, "ray", "lake", "manage", None).await;
        let (_, body) = engine.get(READER, "/v1/catalog").await;
        assert_eq!(names(&body, "catalogs"), ["lake"]);
        let (status, _) = engine.statement(READER, "lake", "SELECT 1").await;
        assert_eq!(status, StatusCode::FORBIDDEN);
        engine.stop();
    }

    #[tokio::test]
    async fn an_ungranted_catalog_is_indistinguishable_from_an_absent_one() {
        let engine = Engine::start().await;
        engine.grant(ADMIN, "ana", "lake", "query", None).await;

        // Discovery: the hidden catalog and a catalog that does not exist
        // answer with the same status, code and text.
        let (hidden_status, hidden) = engine.get(ANALYST, "/v1/catalog/secret/schema").await;
        let (absent_status, absent) = engine.get(ANALYST, "/v1/catalog/nowhere/schema").await;
        assert_eq!(hidden_status, StatusCode::NOT_FOUND);
        assert_eq!(absent_status, StatusCode::NOT_FOUND);
        assert_eq!(hidden["code"], "CATALOG_NOT_FOUND");
        assert_eq!(
            hidden["error"].as_str().unwrap().replace("secret", "X"),
            absent["error"].as_str().unwrap().replace("nowhere", "X")
        );
        let (hidden_status, _) = engine
            .get(ANALYST, "/v1/catalog/secret/schema/sales/table")
            .await;
        assert_eq!(hidden_status, StatusCode::NOT_FOUND);

        // The metadata statements.
        for (hidden_sql, absent_sql) in [
            ("SHOW SCHEMAS FROM secret", "SHOW SCHEMAS FROM nowhere"),
            (
                "SHOW TABLES FROM secret.sales",
                "SHOW TABLES FROM nowhere.sales",
            ),
            (
                "DESCRIBE secret.sales.orders",
                "DESCRIBE nowhere.sales.orders",
            ),
            (
                "SHOW CREATE TABLE secret.sales.orders",
                "SHOW CREATE TABLE nowhere.sales.orders",
            ),
            ("CREATE SCHEMA secret.raw", "CREATE SCHEMA nowhere.raw"),
        ] {
            let (hidden_status, hidden) = engine.statement(ANALYST, "lake", hidden_sql).await;
            let (absent_status, absent) = engine.statement(ANALYST, "lake", absent_sql).await;
            assert_eq!(
                hidden_status, absent_status,
                "{hidden_sql}: {hidden} / {absent}"
            );
            assert_eq!(hidden["code"], absent["code"], "{hidden_sql}");
            assert_eq!(
                hidden["error"].as_str().unwrap().replace("secret", "X"),
                absent["error"].as_str().unwrap().replace("nowhere", "X"),
                "{hidden_sql}"
            );
        }
        let (status, body) = engine.statement(ANALYST, "lake", "SHOW CATALOGS").await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["data"], json!([["lake"]]));

        // A hand-written statement naming the hidden catalog fails at bind
        // with the text an absent catalog gets; so does a session opened
        // in it.
        let (hidden_status, hidden) = engine
            .statement(ANALYST, "lake", "SELECT id FROM secret.sales.orders")
            .await;
        let (absent_status, absent) = engine
            .statement(ANALYST, "lake", "SELECT id FROM nowhere.sales.orders")
            .await;
        assert_eq!(hidden_status, StatusCode::BAD_REQUEST, "{hidden}");
        assert_eq!(absent_status, StatusCode::BAD_REQUEST);
        assert_eq!(hidden["code"], "ANALYSIS_ERROR");
        assert_eq!(
            hidden["error"].as_str().unwrap().replace("secret", "X"),
            absent["error"].as_str().unwrap().replace("nowhere", "X")
        );
        let (hidden_status, hidden) = engine
            .statement(ANALYST, "secret", "SELECT id FROM orders")
            .await;
        let (absent_status, absent) = engine
            .statement(ANALYST, "nowhere", "SELECT id FROM orders")
            .await;
        assert_eq!(hidden_status, absent_status);
        assert_eq!(hidden["code"], "CATALOG_NOT_FOUND");
        assert_eq!(
            hidden["error"].as_str().unwrap().replace("secret", "X"),
            absent["error"].as_str().unwrap().replace("nowhere", "X")
        );
        // A join across the granted and the hidden catalog fails the same
        // way: every scan is checked.
        let (status, body) = engine
            .statement(
                ANALYST,
                "lake",
                "SELECT a.id FROM orders a JOIN secret.sales.orders b ON a.id = b.id",
            )
            .await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert!(
            body["error"]
                .as_str()
                .unwrap()
                .contains("catalog 'secret' not found"),
            "{body}"
        );
        // The definitions API for the hidden catalog's objects: 404, as for
        // an id that does not exist.
        for path in [
            "/v1/catalog/definitions/catalog:secret",
            "/v1/catalog/definitions/catalog:secret/schemas",
            "/v1/catalog/schemas/schema:secret:sales",
            "/v1/catalog/schemas/schema:secret:sales/tables",
            "/v1/catalog/tables/table:secret:sales:orders",
            "/v1/catalog/tables/table:secret:sales:orders/statistics",
            "/v1/catalog/tables/table:secret:sales:orders/version",
            "/v1/catalog/definitions/catalog:nowhere",
            "/v1/catalog/tables/table:nowhere:sales:orders",
        ] {
            let (status, _) = engine.get(ANALYST, path).await;
            assert_eq!(status, StatusCode::NOT_FOUND, "{path}");
        }
        engine.stop();
    }

    #[tokio::test]
    async fn levels_gate_statements_and_ddl_and_revokes_take_effect_at_once() {
        let engine = Engine::start().await;
        // `bea` browses `lake`: discovery and DESCRIBE, no reads, no DDL.
        let (status, body) = engine.grant(ADMIN, "bea", "lake", "browse", None).await;
        assert_eq!(status, StatusCode::OK, "{body}");
        let (status, body) = engine.statement(BROWSER, "lake", "DESCRIBE orders").await;
        assert_eq!(status, StatusCode::OK, "{body}");
        let (status, body) = engine
            .statement(BROWSER, "lake", "SELECT id FROM orders")
            .await;
        assert_eq!(status, StatusCode::FORBIDDEN, "{body}");
        assert_eq!(body["code"], "ACCESS_DENIED");
        let (status, body) = engine
            .statement(BROWSER, "lake", "CREATE SCHEMA lake.raw")
            .await;
        assert_eq!(status, StatusCode::FORBIDDEN, "{body}");
        assert!(body["error"].as_str().unwrap().contains("manage"), "{body}");
        // Raised to `query`: reads run, DDL is still refused; the change
        // needs the current revision.
        let (status, body) = engine.grant(ADMIN, "bea", "lake", "query", None).await;
        assert_eq!(status, StatusCode::CONFLICT, "{body}");
        assert_eq!(body["code"], "REVISION_CONFLICT");
        let (status, body) = engine.grant(ADMIN, "bea", "lake", "query", Some(9)).await;
        assert_eq!(status, StatusCode::CONFLICT, "{body}");
        let (status, body) = engine.grant(ADMIN, "bea", "lake", "query", Some(1)).await;
        assert_eq!(status, StatusCode::OK, "{body}");
        assert_eq!(body["grant"]["revision"], 2);
        assert_eq!(body["revision_before"], 1);
        let (status, _) = engine
            .statement(BROWSER, "lake", "SELECT id FROM orders")
            .await;
        assert_eq!(status, StatusCode::OK);
        let (status, _) = engine
            .statement(BROWSER, "lake", "CREATE SCHEMA lake.raw")
            .await;
        assert_eq!(status, StatusCode::FORBIDDEN);
        // `manage`: DDL inside the catalog; CREATE CATALOG stays the Admin's.
        engine.grant(ADMIN, "bea", "lake", "manage", Some(2)).await;
        let (status, body) = engine
            .statement(BROWSER, "lake", "CREATE SCHEMA lake.raw")
            .await;
        assert_eq!(status, StatusCode::OK, "{body}");
        let (status, _) = engine
            .statement(
                BROWSER,
                "lake",
                "CREATE CATALOG other WITH (storage = 'local', base_path = '/tmp')",
            )
            .await;
        assert_eq!(status, StatusCode::FORBIDDEN);
        // Revoked: gone from the list and refused at the Engine, even with
        // a hand-written statement, on the next request.
        let (status, body) = engine.revoke(ADMIN, "bea", "lake", 2).await;
        assert_eq!(status, StatusCode::CONFLICT, "{body}");
        let (status, body) = engine.revoke(ADMIN, "bea", "lake", 3).await;
        assert_eq!(status, StatusCode::OK, "{body}");
        assert_eq!(body["revoked"]["principal"], "bea");
        let (_, body) = engine.get(BROWSER, "/v1/catalog").await;
        assert!(names(&body, "catalogs").is_empty());
        let (status, body) = engine
            .statement(BROWSER, "lake", "SELECT id FROM lake.sales.orders")
            .await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
        assert!(
            body["error"]
                .as_str()
                .unwrap()
                .contains("catalog 'lake' not found")
        );
        let (status, _) = engine.revoke(ADMIN, "bea", "lake", 3).await;
        assert_eq!(status, StatusCode::CONFLICT);

        // The ledger names the actor for every grant, change and revoke.
        let (status, body) = engine
            .get(ADMIN, "/v1/audit?kind=catalog_access&limit=100")
            .await;
        assert_eq!(status, StatusCode::OK, "{body}");
        let records = body["records"].as_array().unwrap();
        let kinds: Vec<&str> = records
            .iter()
            .map(|record| record["kind"].as_str().unwrap())
            .collect();
        assert_eq!(
            kinds,
            [
                "catalog_access.grant",
                "catalog_access.grant",
                "catalog_access.grant",
                "catalog_access.revoke"
            ]
        );
        for record in records {
            assert_eq!(record["principal"], "root");
            assert_eq!(record["role"], "admin");
            assert_eq!(record["object_type"], "catalog_grant");
            assert_eq!(record["object_id"], "lake/bea");
            assert_eq!(record["catalog"], "lake");
            assert_eq!(record["details"]["principal"], "bea");
        }
        assert_eq!(records[0]["revision_after"], 1);
        assert!(records[0].get("revision_before").is_none());
        assert_eq!(records[1]["revision_before"], 1);
        assert_eq!(records[1]["revision_after"], 2);
        assert_eq!(records[1]["details"]["access_before"], "browse");
        assert_eq!(records[3]["revision_before"], 3);
        assert!(records[3].get("revision_after").is_none());
        engine.stop();
    }

    #[tokio::test]
    async fn only_admins_manage_grants_and_bodies_never_name_the_actor() {
        let engine = Engine::start().await;
        for token in [ANALYST, READER] {
            let (status, _) = engine.get(token, "/v1/admin/catalog-access").await;
            assert_eq!(status, StatusCode::FORBIDDEN);
            let (status, _) = engine.grant(token, "ana", "lake", "manage", None).await;
            assert_eq!(status, StatusCode::FORBIDDEN);
            let (status, _) = engine.revoke(token, "ana", "lake", 1).await;
            assert_eq!(status, StatusCode::FORBIDDEN);
            let (status, _) = engine
                .send(
                    reqwest::Method::POST,
                    token,
                    "/v1/admin/catalog-access/import",
                    json!({ "source": "open", "apply": true }),
                )
                .await;
            assert_eq!(status, StatusCode::FORBIDDEN);
        }
        // A request cannot smuggle an actor or a role; unknown fields are
        // refused, and the recorded grantor is the authenticated Admin.
        let (status, body) = engine
            .send(
                reqwest::Method::PUT,
                ADMIN,
                "/v1/admin/catalog-access/grants",
                json!({ "principal": "ana", "catalog": "lake", "access": "query", "granted_by": "ana" }),
            )
            .await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
        let (_, body) = engine.grant(ADMIN, "ana", "lake", "query", None).await;
        assert_eq!(body["grant"]["granted_by"], "root");
        // The reserved authority is never grantable; an unknown catalog is
        // not found; the analyst's transaction session cannot stage a row
        // into the family.
        let (status, body) = engine.grant(ADMIN, "ana", "KaveonDB", "browse", None).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
        assert_eq!(body["code"], "RESERVED_CATALOG");
        let (status, body) = engine.grant(ADMIN, "ana", "kaveondb", "browse", None).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
        let (status, body) = engine.grant(ADMIN, "ana", "nowhere", "browse", None).await;
        assert_eq!(status, StatusCode::NOT_FOUND, "{body}");
        let (_, body) = engine.get(ADMIN, "/v1/admin/catalog-access").await;
        assert_eq!(body["catalogs"], json!(["lake", "secret"]));
        assert_eq!(body["reserved"][0]["name"], "KaveonDB");
        assert_eq!(body["reserved"][0]["visible_to"], "admin");
        assert_eq!(body["roles"]["reader"], "browse");
        assert_eq!(body["roles"]["analyst"], "manage");
        assert_eq!(body["store"]["enabled"], true);
        assert_eq!(body["grants"].as_array().unwrap().len(), 1);
        let (status, body) = engine
            .get(ADMIN, "/v1/admin/catalog-access/effective/ana")
            .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["grants"][0]["catalog"], "lake");
        assert_eq!(body["grants"][0]["effective"]["reader"], "browse");
        assert_eq!(body["grants"][0]["effective"]["analyst"], "query");
        assert_eq!(body["ungranted"], json!(["secret"]));
        engine.stop();
    }

    #[tokio::test]
    async fn the_open_policy_import_proposes_from_the_ledger_and_records_only_on_apply() {
        let engine = Engine::start().await;
        // The ledger sees the analyst and the reader submit (the reader is
        // refused, and refusals are not submissions); the Admin too.
        engine.grant(ADMIN, "ana", "lake", "query", None).await;
        engine
            .statement(ANALYST, "lake", "SELECT id FROM orders")
            .await;
        engine
            .statement(ADMIN, "secret", "SELECT id FROM orders")
            .await;
        engine.statement(READER, "lake", "SELECT 1").await;
        let (status, body) = engine
            .send(
                reqwest::Method::POST,
                ADMIN,
                "/v1/admin/catalog-access/import",
                json!({ "source": "open" }),
            )
            .await;
        assert_eq!(status, StatusCode::OK, "{body}");
        assert_eq!(body["applied"], false);
        assert_eq!(body["principals_seen"], 1);
        // Only the pair with no grant yet is proposed, at the role's
        // ceiling; the Admin is never proposed.
        assert_eq!(
            body["proposed"],
            json!([{ "principal": "ana", "role_seen": "analyst", "catalog": "secret", "access": "manage" }])
        );
        assert_eq!(body["recorded"], json!([]));
        let (_, current) = engine.get(ADMIN, "/v1/admin/catalog-access").await;
        assert_eq!(current["grants"].as_array().unwrap().len(), 1);
        let (status, body) = engine
            .send(
                reqwest::Method::POST,
                ADMIN,
                "/v1/admin/catalog-access/import",
                json!({ "source": "open", "apply": true }),
            )
            .await;
        assert_eq!(status, StatusCode::OK, "{body}");
        assert_eq!(body["applied"], true);
        assert_eq!(body["recorded"].as_array().unwrap().len(), 1);
        assert_eq!(body["recorded"][0]["catalog"], "secret");
        assert_eq!(body["recorded"][0]["granted_by"], "root");
        let (_, body) = engine.get(ANALYST, "/v1/catalog").await;
        assert_eq!(names(&body, "catalogs"), ["lake", "secret"]);
        let (status, body) = engine
            .send(
                reqwest::Method::POST,
                ADMIN,
                "/v1/admin/catalog-access/import",
                json!({ "source": "everything" }),
            )
            .await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
        engine.stop();
    }
}
