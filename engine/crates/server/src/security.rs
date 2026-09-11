//! Authentication at the Engine boundary. Forwarded identity is accepted only
//! from the separately authenticated platform bridge, never from a user token.
use crate::AppState;
use axum::{
    extract::{Request, State},
    http::{HeaderMap, Method, StatusCode},
    middleware::Next,
    response::{IntoResponse, Response},
};
use serde::Deserialize;
use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
};

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SecurityConfig {
    pub entra: Option<crate::entra::EntraConfig>,
    /// Absolute HTTPS origin for the Studio front door. Never includes a path.
    pub studio_url: Option<String>,
    #[serde(default)]
    pub insecure_development: bool,
    #[serde(default)]
    pub principals: Vec<PrincipalCredential>,
    pub bridge_token: Option<String>,
    #[serde(default)]
    pub resource_groups: Vec<ResourceGroupConfig>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ResourceGroupConfig {
    pub name: String,
    pub principals: Vec<String>,
    pub max_running: usize,
    pub max_queued: usize,
    pub queue_timeout_ms: u64,
}
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PrincipalCredential {
    pub token: String,
    pub principal: String,
    pub role: Role,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    Reader,
    Analyst,
    Admin,
}
#[derive(Clone, Debug)]
pub struct Identity {
    pub principal: String,
    /// Display-only identity from a validated authentication source. Never use for authorization.
    pub display_identity: Option<String>,
    pub role: Role,
}
impl Identity {
    pub fn display_name(&self) -> &str {
        self.display_identity.as_deref().unwrap_or(&self.principal)
    }
    pub fn can_view(&self, owner: Option<&str>) -> bool {
        self.role == Role::Admin || owner == Some(self.principal.as_str())
    }
}
pub fn bearer(headers: &HeaderMap) -> Option<&str> {
    headers
        .get("authorization")?
        .to_str()
        .ok()?
        .strip_prefix("Bearer ")
}
pub fn token_matches(actual: Option<&str>, expected: Option<&str>) -> bool {
    let Some(expected) = expected.filter(|v| !v.is_empty()) else {
        return false;
    };
    let Some(actual) = actual else {
        return false;
    };
    let mut diff = actual.len() ^ expected.len();
    for (a, b) in actual.bytes().zip(expected.bytes()) {
        diff |= usize::from(a ^ b);
    }
    diff == 0
}
impl SecurityConfig {
    pub fn validate(&self) -> anyhow::Result<()> {
        if let Some(entra) = &self.entra {
            entra.validate()?;
        }
        if let Some(studio_url) = &self.studio_url {
            let parsed = reqwest::Url::parse(studio_url)
                .map_err(|_| anyhow::anyhow!("studio_url must be an absolute HTTPS origin"))?;
            anyhow::ensure!(
                parsed.scheme() == "https"
                    && parsed.host_str().is_some()
                    && parsed.username().is_empty()
                    && parsed.password().is_none()
                    && parsed.path() == "/"
                    && parsed.query().is_none()
                    && parsed.fragment().is_none(),
                "studio_url must be an absolute HTTPS origin with no path, query, fragment, or credentials"
            );
        }
        let mut group_names = std::collections::HashSet::new();
        let mut grouped_principals = std::collections::HashSet::new();
        let mut wildcard_group = None;
        for group in &self.resource_groups {
            anyhow::ensure!(
                !group.name.trim().is_empty() && group_names.insert(&group.name),
                "resource group names must be nonempty and unique"
            );
            anyhow::ensure!(
                group.max_running > 0
                    && group.max_running <= 10000
                    && group.max_queued <= 10000
                    && group.queue_timeout_ms > 0
                    && group.queue_timeout_ms <= 300000,
                "invalid resource group bounds"
            );
            for principal in &group.principals {
                anyhow::ensure!(
                    !principal.trim().is_empty()
                        && principal == principal.trim()
                        && grouped_principals.insert(principal),
                    "each principal may belong to only one resource group"
                );
                if principal == "*" {
                    anyhow::ensure!(
                        wildcard_group.replace(group.name.as_str()).is_none(),
                        "only one wildcard resource group is allowed"
                    );
                }
            }
        }
        let mut tokens = std::collections::HashSet::new();
        for credential in &self.principals {
            anyhow::ensure!(
                credential.token.len() >= 32 && !credential.principal.trim().is_empty(),
                "security principals require nonempty names and tokens of at least 32 bytes"
            );
            anyhow::ensure!(
                tokens.insert(credential.token.as_str()),
                "duplicate security token"
            );
        }
        if let Some(token) = &self.bridge_token {
            anyhow::ensure!(
                token.len() >= 32 && tokens.insert(token),
                "bridge token must be distinct and at least 32 bytes"
            );
        }
        Ok(())
    }
    pub fn authenticate(&self, headers: &HeaderMap) -> Result<Identity, StatusCode> {
        let token = bearer(headers);
        if token_matches(token, self.bridge_token.as_deref()) {
            let principal = headers
                .get("x-kaveon-principal")
                .and_then(|v| v.to_str().ok())
                .filter(|v| !v.trim().is_empty())
                .ok_or(StatusCode::UNAUTHORIZED)?;
            let role = match headers.get("x-kaveon-role").and_then(|v| v.to_str().ok()) {
                Some("reader") => Role::Reader,
                Some("analyst") => Role::Analyst,
                Some("admin") => Role::Admin,
                _ => return Err(StatusCode::FORBIDDEN),
            };
            return Ok(Identity {
                principal: principal.into(),
                display_identity: None,
                role,
            });
        }
        for credential in &self.principals {
            if token_matches(token, Some(&credential.token)) {
                return Ok(Identity {
                    principal: credential.principal.clone(),
                    display_identity: None,
                    role: credential.role,
                });
            }
        }
        if self.insecure_development && token.is_none() {
            return Ok(Identity {
                principal: "development".into(),
                display_identity: None,
                role: Role::Admin,
            });
        }
        Err(StatusCode::UNAUTHORIZED)
    }
}

pub async fn authorize(
    State(state): State<Arc<AppState>>,
    mut request: Request,
    next: Next,
) -> Response {
    let path = request.uri().path();
    // Only the static login shell is public; dashboard data stays authenticated.
    if matches!(path, "/health" | "/ready")
        || (matches!(path, "/ui" | "/ui/msal-browser.min.js" | "/v1/auth/config")
            && request.method() == Method::GET)
    {
        return next.run(request).await;
    }
    let internal = path == "/v1/task"
        || path == "/v1/node/heartbeat"
        || path.starts_with("/v1/internal/")
        || path.starts_with("/v1/exchange")
        || (!state.config.coordinator
            && path.starts_with("/v1/query/")
            && request.method() == Method::DELETE);
    if internal {
        if !token_matches(
            bearer(request.headers()),
            state.config.exchange_token.as_deref(),
        ) {
            return StatusCode::UNAUTHORIZED.into_response();
        }
        request.extensions_mut().insert(Identity {
            principal: "internal".into(),
            display_identity: None,
            role: Role::Admin,
        });
        return next.run(request).await;
    }
    // The catalog service credential is confined to the metadata API.
    if path.starts_with("/v1/catalog/")
        && token_matches(
            bearer(request.headers()),
            state.config.catalog_admin_token.as_deref(),
        )
    {
        return next.run(request).await;
    }
    let identity = match state.config.security.authenticate(request.headers()) {
        Ok(identity) => identity,
        Err(status) => {
            let Some(entra) = &state.config.security.entra else {
                return status.into_response();
            };
            let Some(token) = bearer(request.headers()) else {
                return status.into_response();
            };
            match entra.authenticate(token).await {
                Ok(identity) => identity,
                Err(status) => return status.into_response(),
            }
        }
    };
    if (path == "/v1/statement" || path.starts_with("/v1/transaction"))
        && identity.role == Role::Reader
    {
        return StatusCode::FORBIDDEN.into_response();
    }
    request.extensions_mut().insert(identity);
    next.run(request).await
}

#[derive(Default)]
pub struct PrincipalAdmission {
    active: Arc<Mutex<HashMap<String, usize>>>,
    groups: Mutex<HashMap<String, Arc<GroupGate>>>,
}
struct GroupGate {
    running: Arc<tokio::sync::Semaphore>,
    queued: Arc<std::sync::atomic::AtomicUsize>,
}
struct QueueGuard(Arc<std::sync::atomic::AtomicUsize>);
impl Drop for QueueGuard {
    fn drop(&mut self) {
        self.0.fetch_sub(1, std::sync::atomic::Ordering::AcqRel);
    }
}
pub struct PrincipalPermit {
    active: Arc<Mutex<HashMap<String, usize>>>,
    principal: String,
}
impl PrincipalAdmission {
    pub async fn admit_group(
        &self,
        principal: &str,
        config: &SecurityConfig,
    ) -> Result<Option<tokio::sync::OwnedSemaphorePermit>, StatusCode> {
        let Some(group) = config
            .resource_groups
            .iter()
            .find(|group| group.principals.iter().any(|member| member == principal))
            .or_else(|| {
                config
                    .resource_groups
                    .iter()
                    .find(|group| group.principals.iter().any(|member| member == "*"))
            })
        else {
            return Ok(None);
        };
        let gate = {
            let mut gates = self
                .groups
                .lock()
                .map_err(|_| StatusCode::SERVICE_UNAVAILABLE)?;
            gates
                .entry(group.name.clone())
                .or_insert_with(|| {
                    Arc::new(GroupGate {
                        running: Arc::new(tokio::sync::Semaphore::new(group.max_running)),
                        queued: Arc::new(std::sync::atomic::AtomicUsize::new(0)),
                    })
                })
                .clone()
        };
        if let Ok(permit) = gate.running.clone().try_acquire_owned() {
            return Ok(Some(permit));
        }
        gate.queued
            .fetch_update(
                std::sync::atomic::Ordering::AcqRel,
                std::sync::atomic::Ordering::Acquire,
                |queued| (queued < group.max_queued).then_some(queued + 1),
            )
            .map_err(|_| StatusCode::TOO_MANY_REQUESTS)?;
        let _waiting = QueueGuard(gate.queued.clone());
        match tokio::time::timeout(
            std::time::Duration::from_millis(group.queue_timeout_ms),
            gate.running.clone().acquire_owned(),
        )
        .await
        {
            Ok(Ok(permit)) => Ok(Some(permit)),
            Ok(Err(_)) => Err(StatusCode::SERVICE_UNAVAILABLE),
            Err(_) => Err(StatusCode::TOO_MANY_REQUESTS),
        }
    }
    pub fn admit(&self, principal: &str, limit: usize) -> Result<PrincipalPermit, StatusCode> {
        let mut active = self
            .active
            .lock()
            .map_err(|_| StatusCode::SERVICE_UNAVAILABLE)?;
        let count = active.entry(principal.into()).or_default();
        if *count >= limit {
            return Err(StatusCode::TOO_MANY_REQUESTS);
        }
        *count += 1;
        Ok(PrincipalPermit {
            active: self.active.clone(),
            principal: principal.into(),
        })
    }
}
impl Drop for PrincipalPermit {
    fn drop(&mut self) {
        if let Ok(mut active) = self.active.lock()
            && let Some(count) = active.get_mut(&self.principal)
        {
            *count -= 1;
            if *count == 0 {
                active.remove(&self.principal);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn dashboard_shell_is_public_but_data_requires_authentication() {
        let config = crate::config::ServerConfig {
            security: SecurityConfig {
                entra: Some(
                    serde_json::from_value(serde_json::json!({
                        "tenant_id": "11111111-1111-1111-1111-111111111111",
                        "client_id": "22222222-2222-2222-2222-222222222222",
                        "required_scope": "access_as_user",
                        "principals": {"33333333-3333-3333-3333-333333333333": "analyst"}
                    }))
                    .unwrap(),
                ),
                studio_url: Some("https://studio.kaveon.example".into()),
                principals: vec![
                    PrincipalCredential {
                        token: "a".repeat(32),
                        principal: "alice".into(),
                        role: Role::Analyst,
                    },
                    PrincipalCredential {
                        token: "r".repeat(32),
                        principal: "reader".into(),
                        role: Role::Reader,
                    },
                ],
                ..Default::default()
            },
            ..Default::default()
        };
        let catalog_store = kaveon_catalog::CatalogStore::open_in_memory().unwrap();
        let snapshot_id = catalog_store.snapshot_identity().unwrap();
        let state = Arc::new(crate::AppState {
            disk_exchange_store: None,
            results: crate::results::ResultStore::default(),
            principal_admission: PrincipalAdmission::default(),
            cluster: tokio::sync::RwLock::new(crate::cluster::ClusterState::new(&config)),
            catalog: tokio::sync::RwLock::new(Arc::new(crate::PublishedCatalog {
                manager: kaveon_core::CatalogManager::new("kaveon", "default"),
                snapshot_id,
            })),
            catalog_store,
            exchange_store: crate::exchange::ExchangeStore::default(),
            lifecycle: crate::lifecycle::WorkerLifecycle::default(),
            memory_admission: kaveon_core::MemoryAdmissionController::new(
                config.memory_admission_limit_bytes,
            )
            .unwrap(),
            product_transactions: crate::transaction_api::TransactionRegistry::disabled(),
            config,
        });
        let app = axum::Router::new()
            .route("/ui", axum::routing::get(crate::ui::dashboard))
            .route(
                "/ui/msal-browser.min.js",
                axum::routing::get(crate::ui::msal_script),
            )
            .route(
                "/v1/auth/config",
                axum::routing::get(crate::entra::public_config),
            )
            .route("/v1/cluster", axum::routing::get(|| async { "protected" }))
            .route("/v1/query", axum::routing::get(|| async { "protected" }))
            .route("/v1/transaction", axum::routing::post(|| async { "write" }))
            .layer(axum::middleware::from_fn_with_state(
                state.clone(),
                authorize,
            ))
            .with_state(state);
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let client = reqwest::Client::new();
        let shell = client
            .get(format!("http://{address}/ui"))
            .send()
            .await
            .unwrap();
        assert_eq!(shell.status(), StatusCode::OK);
        assert!(shell.text().await.unwrap().contains("id=\"auth-token\""));
        let auth = client
            .get(format!("http://{address}/v1/auth/config"))
            .send()
            .await
            .unwrap();
        assert_eq!(auth.status(), StatusCode::OK);
        let auth: serde_json::Value = auth.json().await.unwrap();
        assert_eq!(
            auth["entra"]["scope"],
            "api://22222222-2222-2222-2222-222222222222/access_as_user"
        );
        assert_eq!(auth["entra"].as_object().unwrap().len(), 3);
        assert_eq!(auth["studio_url"], "https://studio.kaveon.example");
        assert_eq!(
            client
                .get(format!("http://{address}/ui/msal-browser.min.js"))
                .send()
                .await
                .unwrap()
                .status(),
            StatusCode::OK
        );
        for path in ["/v1/cluster", "/v1/query"] {
            let url = format!("http://{address}{path}");
            assert_eq!(
                client.get(&url).send().await.unwrap().status(),
                StatusCode::UNAUTHORIZED
            );
            assert_eq!(
                client
                    .get(&url)
                    .bearer_auth("wrong")
                    .send()
                    .await
                    .unwrap()
                    .status(),
                StatusCode::UNAUTHORIZED
            );
            assert_eq!(
                client
                    .get(&url)
                    .bearer_auth("a".repeat(32))
                    .send()
                    .await
                    .unwrap()
                    .status(),
                StatusCode::OK
            );
        }
        assert_eq!(
            client
                .post(format!("http://{address}/ui"))
                .send()
                .await
                .unwrap()
                .status(),
            StatusCode::UNAUTHORIZED
        );
        assert_eq!(
            client
                .post(format!("http://{address}/v1/transaction"))
                .bearer_auth("r".repeat(32))
                .send()
                .await
                .unwrap()
                .status(),
            StatusCode::FORBIDDEN
        );
        server.abort();
    }

    #[test]
    fn rejects_missing_wrong_and_untrusted_forwarded_identity() {
        let config = SecurityConfig {
            principals: vec![PrincipalCredential {
                token: "a".repeat(32),
                principal: "alice".into(),
                role: Role::Analyst,
            }],
            ..Default::default()
        };
        let mut headers = HeaderMap::new();
        headers.insert("x-kaveon-principal", "admin".parse().unwrap());
        assert_eq!(
            config.authenticate(&headers).unwrap_err(),
            StatusCode::UNAUTHORIZED
        );
        headers.insert(
            "authorization",
            format!("Bearer {}", "a".repeat(32)).parse().unwrap(),
        );
        let identity = config.authenticate(&headers).unwrap();
        assert_eq!(identity.principal, "alice");
        assert!(!identity.can_view(Some("bob")));
        assert!(identity.can_view(Some("alice")));
    }
    #[test]
    fn bridge_requires_identity_and_known_role() {
        let config = SecurityConfig {
            bridge_token: Some("b".repeat(32)),
            ..Default::default()
        };
        let mut headers = HeaderMap::new();
        headers.insert(
            "authorization",
            format!("Bearer {}", "b".repeat(32)).parse().unwrap(),
        );
        assert!(config.authenticate(&headers).is_err());
        headers.insert("x-kaveon-principal", "alice".parse().unwrap());
        headers.insert("x-kaveon-role", "owner".parse().unwrap());
        assert!(config.authenticate(&headers).is_err());
        headers.insert("x-kaveon-role", "analyst".parse().unwrap());
        assert_eq!(config.authenticate(&headers).unwrap().role, Role::Analyst);
    }
    #[test]
    fn quota_is_per_principal_and_releases() {
        let admission = PrincipalAdmission::default();
        let alice = admission.admit("alice", 1).unwrap();
        assert!(admission.admit("alice", 1).is_err());
        let _bob = admission.admit("bob", 1).unwrap();
        drop(alice);
        assert!(admission.admit("alice", 1).is_ok());
    }

    #[tokio::test]
    async fn resource_group_queue_is_bounded_and_recovers_after_timeout() {
        let admission = PrincipalAdmission::default();
        let config = SecurityConfig {
            resource_groups: vec![ResourceGroupConfig {
                name: "interactive".into(),
                principals: vec!["alice".into(), "bob".into()],
                max_running: 1,
                max_queued: 1,
                queue_timeout_ms: 10,
            }],
            ..Default::default()
        };
        config.validate().unwrap();
        let first = admission.admit_group("alice", &config).await.unwrap();
        assert_eq!(
            admission.admit_group("bob", &config).await.unwrap_err(),
            StatusCode::TOO_MANY_REQUESTS
        );
        drop(first);
        assert!(admission.admit_group("bob", &config).await.is_ok());
        assert_eq!(
            admission.groups.lock().unwrap()["interactive"]
                .queued
                .load(std::sync::atomic::Ordering::Acquire),
            0
        );
    }

    #[tokio::test]
    async fn queued_group_request_runs_when_slot_releases() {
        let admission = Arc::new(PrincipalAdmission::default());
        let config = Arc::new(SecurityConfig {
            resource_groups: vec![ResourceGroupConfig {
                name: "interactive".into(),
                principals: vec!["alice".into()],
                max_running: 1,
                max_queued: 1,
                queue_timeout_ms: 1000,
            }],
            ..Default::default()
        });
        let first = admission.admit_group("alice", &config).await.unwrap();
        let waiter_admission = admission.clone();
        let waiter_config = config.clone();
        let waiting =
            tokio::spawn(
                async move { waiter_admission.admit_group("alice", &waiter_config).await },
            );
        tokio::task::yield_now().await;
        assert_eq!(
            admission.admit_group("alice", &config).await.unwrap_err(),
            StatusCode::TOO_MANY_REQUESTS
        );
        drop(first);
        assert!(waiting.await.unwrap().is_ok());
    }

    #[tokio::test]
    async fn wildcard_group_admits_unlisted_principals_and_exact_group_wins() {
        let admission = PrincipalAdmission::default();
        let config = SecurityConfig {
            resource_groups: vec![
                ResourceGroupConfig {
                    name: "interactive".into(),
                    principals: vec!["alice".into()],
                    max_running: 1,
                    max_queued: 0,
                    queue_timeout_ms: 100,
                },
                ResourceGroupConfig {
                    name: "default".into(),
                    principals: vec!["*".into()],
                    max_running: 1,
                    max_queued: 0,
                    queue_timeout_ms: 100,
                },
            ],
            ..Default::default()
        };
        config.validate().unwrap();

        let alice = admission.admit_group("alice", &config).await.unwrap();
        let bob = admission.admit_group("bob", &config).await.unwrap();
        assert!(alice.is_some());
        assert!(bob.is_some());
        assert!(admission.groups.lock().unwrap().contains_key("interactive"));
        assert!(admission.groups.lock().unwrap().contains_key("default"));
        assert_eq!(admission.groups.lock().unwrap().len(), 2);
    }

    #[test]
    fn resource_groups_reject_multiple_wildcards_and_whitespace_principals() {
        let group = |name: &str, principal: &str| ResourceGroupConfig {
            name: name.into(),
            principals: vec![principal.into()],
            max_running: 1,
            max_queued: 0,
            queue_timeout_ms: 100,
        };
        let duplicate_default = SecurityConfig {
            resource_groups: vec![group("one", "*"), group("two", "*")],
            ..Default::default()
        };
        assert!(duplicate_default.validate().is_err());

        let whitespace = SecurityConfig {
            resource_groups: vec![group("one", " alice ")],
            ..Default::default()
        };
        assert!(whitespace.validate().is_err());
    }

    #[test]
    fn studio_url_accepts_only_an_https_origin() {
        for accepted in ["https://studio.kaveon.example", "https://localhost:3000"] {
            SecurityConfig {
                studio_url: Some(accepted.into()),
                ..Default::default()
            }
            .validate()
            .unwrap();
        }
        for rejected in [
            "http://studio.kaveon.example",
            "https://user@studio.kaveon.example",
            "https://studio.kaveon.example/engine",
            "https://studio.kaveon.example?next=/engine",
            "not-a-url",
        ] {
            assert!(
                SecurityConfig {
                    studio_url: Some(rejected.into()),
                    ..Default::default()
                }
                .validate()
                .is_err(),
                "unexpectedly accepted {rejected}"
            );
        }
    }
}
