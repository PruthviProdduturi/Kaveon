//! Resource groups: named admission policies, and the ordered selectors
//! that map a principal to one at admission.
//!
//! A group bounds what its statements may hold of the coordinator's
//! admission pool, how many run at once, how many wait and for how long,
//! their aggregator threads, and the request settings they default to.
//! Statements are admitted through `kaveon_core::MemoryAdmissionController`
//! under the group's policy; the cross-group order is documented there and
//! in `docs/engine/governance.md`.
//!
//! The configuration comes, in order of precedence, from the file named by
//! `KAVEON_RESOURCE_GROUPS`, the coordinator's durable copy of the last
//! `PUT /v1/admin/resource-groups` (`<state dir>/resource-groups.json`),
//! the `[resource_groups]` section of the configuration file, the legacy
//! `security.resource_groups` list, and finally a built-in `default` group
//! whose `max_concurrent` is `KAVEON_PRINCIPAL_QUERY_LIMIT`.

use crate::AppState;
use crate::config::ServerConfig;
use crate::security::{Identity, Role};
use axum::extract::State;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::{Extension, Json};
use kaveon_core::{AdmissionGroupPolicy, DEFAULT_ADMISSION_GROUP};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use std::path::{Path, PathBuf};
use std::sync::{Arc, RwLock};

const MAX_CONCURRENT_BOUND: usize = 10_000;
const MAX_QUEUED_BOUND: usize = 10_000;
const MAX_QUEUE_WAIT_SECONDS: u64 = 86_400;
const MAX_PRIORITY: u32 = 1_000;
const MAX_GROUPS: usize = 256;
const MAX_SELECTORS: usize = 4_096;

/// One named policy.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ResourceGroup {
    pub name: String,
    /// The sum of the group's admitted query pools; absent, the whole
    /// admission pool. A statement whose pool exceeds it is refused on
    /// arrival.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_memory_bytes: Option<u64>,
    /// Statements of the group running at once.
    pub max_concurrent: usize,
    /// Statements of the group waiting at once; the node's
    /// `KAVEON_MEMORY_ADMISSION_QUEUE` bounds every group together.
    #[serde(default = "default_max_queued")]
    pub max_queued: usize,
    /// How long a statement of the group waits before HTTP 429; the
    /// node's `KAVEON_MEMORY_ADMISSION_WAIT_SECONDS` and the request's
    /// `admission_wait_seconds` can only shorten it.
    #[serde(default = "default_max_queue_wait_seconds")]
    pub max_queue_wait_seconds: u64,
    /// Caps the statement's `local_parallelism`; absent, the node's.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_local_parallelism: Option<usize>,
    /// The group's weight in the cross-group admission order; higher is
    /// entitled to a larger share of the pool under contention.
    #[serde(default = "default_priority")]
    pub priority: u32,
    /// Per-request settings applied when the request sets nothing for the
    /// key; validated like a request's own.
    #[serde(default, skip_serializing_if = "Map::is_empty")]
    pub default_settings: Map<String, Value>,
}

fn default_max_queued() -> usize {
    16
}
fn default_max_queue_wait_seconds() -> u64 {
    60
}
fn default_priority() -> u32 {
    1
}

/// One rule of the ordered selector list. Every matcher given must hold;
/// a selector with no matcher is the catch-all and must be last.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Selector {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub principal: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub principal_prefix: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub role: Option<Role>,
    /// Matches when the request's `client_tags` contains the tag.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub client_tag: Option<String>,
    pub group: String,
}

impl Selector {
    fn is_catch_all(&self) -> bool {
        self.principal.is_none()
            && self.principal_prefix.is_none()
            && self.role.is_none()
            && self.client_tag.is_none()
    }

    fn matches(&self, identity: &Identity, client_tags: &[String]) -> bool {
        self.principal
            .as_deref()
            .is_none_or(|principal| principal == identity.principal)
            && self
                .principal_prefix
                .as_deref()
                .is_none_or(|prefix| identity.principal.starts_with(prefix))
            && self.role.is_none_or(|role| role == identity.role)
            && self
                .client_tag
                .as_deref()
                .is_none_or(|tag| client_tags.iter().any(|given| given == tag))
    }
}

/// The whole configuration: every group and the ordered selectors.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ResourceGroups {
    #[serde(default)]
    pub groups: Vec<ResourceGroup>,
    #[serde(default)]
    pub selectors: Vec<Selector>,
}

/// Where the configuration in force came from.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Source {
    /// The file named by `KAVEON_RESOURCE_GROUPS`.
    Environment,
    /// The coordinator's durable copy of the last runtime replacement.
    Runtime,
    /// The `[resource_groups]` section of the configuration file.
    ConfigFile,
    /// `security.resource_groups`, translated.
    LegacySecurity,
    /// The built-in `default` group.
    Builtin,
}

/// A group's limits as the query record carries them.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct EffectiveGroup {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_memory_bytes: Option<u64>,
    pub max_concurrent: usize,
    pub max_queued: usize,
    pub max_queue_wait_seconds: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_local_parallelism: Option<usize>,
    pub priority: u32,
}

impl EffectiveGroup {
    /// The built-in `default` group as a test context carries it.
    #[cfg(test)]
    pub(crate) fn for_tests() -> Self {
        Self::from(&ResourceGroups::builtin(&ServerConfig::default()).groups[0])
    }
}

impl From<&ResourceGroup> for EffectiveGroup {
    fn from(group: &ResourceGroup) -> Self {
        Self {
            name: group.name.clone(),
            max_memory_bytes: group.max_memory_bytes,
            max_concurrent: group.max_concurrent,
            max_queued: group.max_queued,
            max_queue_wait_seconds: group.max_queue_wait_seconds,
            max_local_parallelism: group.max_local_parallelism,
            priority: group.priority,
        }
    }
}

impl ResourceGroups {
    /// The built-in configuration: one `default` group with
    /// `KAVEON_PRINCIPAL_QUERY_LIMIT` running slots, the node's queue and
    /// wait, no memory share, priority 1.
    pub fn builtin(config: &ServerConfig) -> Self {
        Self {
            groups: vec![ResourceGroup {
                name: DEFAULT_ADMISSION_GROUP.to_owned(),
                max_memory_bytes: None,
                max_concurrent: config.principal_query_limit,
                max_queued: config.memory_admission_queue.max(1),
                max_queue_wait_seconds: config.memory_admission_wait_seconds.max(1),
                max_local_parallelism: None,
                priority: default_priority(),
                default_settings: Map::new(),
            }],
            selectors: Vec::new(),
        }
    }

    /// The legacy `security.resource_groups` list as groups and exact
    /// selectors; a `*` member is the catch-all. A `default` group is
    /// added from the built-in configuration when the list names none.
    pub fn from_legacy(
        legacy: &[crate::security::ResourceGroupConfig],
        config: &ServerConfig,
    ) -> Self {
        let mut groups = Vec::with_capacity(legacy.len() + 1);
        let mut selectors = Vec::new();
        let mut catch_all = None;
        for group in legacy {
            groups.push(ResourceGroup {
                name: group.name.clone(),
                max_memory_bytes: None,
                max_concurrent: group.max_running,
                max_queued: group.max_queued,
                max_queue_wait_seconds: group.queue_timeout_ms.div_ceil(1_000).max(1),
                max_local_parallelism: None,
                priority: default_priority(),
                default_settings: Map::new(),
            });
            for principal in &group.principals {
                if principal == "*" {
                    catch_all = Some(Selector {
                        group: group.name.clone(),
                        ..Selector::default()
                    });
                } else {
                    selectors.push(Selector {
                        principal: Some(principal.clone()),
                        group: group.name.clone(),
                        ..Selector::default()
                    });
                }
            }
        }
        if !groups
            .iter()
            .any(|group| group.name == DEFAULT_ADMISSION_GROUP)
        {
            groups.extend(Self::builtin(config).groups);
        }
        selectors.extend(catch_all);
        Self { groups, selectors }
    }

    /// Refuses what the admission controller or the statement path could
    /// not honour; the message names the group or selector.
    pub fn validate(&self, config: &ServerConfig) -> anyhow::Result<()> {
        anyhow::ensure!(
            !self.groups.is_empty() && self.groups.len() <= MAX_GROUPS,
            "resource groups must name between 1 and {MAX_GROUPS} groups"
        );
        anyhow::ensure!(
            self.selectors.len() <= MAX_SELECTORS,
            "resource groups accept at most {MAX_SELECTORS} selectors"
        );
        let mut names = std::collections::HashSet::new();
        for group in &self.groups {
            let name = &group.name;
            anyhow::ensure!(
                !name.trim().is_empty() && name == name.trim() && name.len() <= 128,
                "resource group names must be nonempty, trimmed and at most 128 characters"
            );
            anyhow::ensure!(
                names.insert(name.as_str()),
                "resource group '{name}' is named twice"
            );
            anyhow::ensure!(
                (1..=MAX_CONCURRENT_BOUND).contains(&group.max_concurrent),
                "resource group '{name}': max_concurrent must be between 1 and {MAX_CONCURRENT_BOUND}"
            );
            anyhow::ensure!(
                group.max_queued <= MAX_QUEUED_BOUND,
                "resource group '{name}': max_queued must be at most {MAX_QUEUED_BOUND}"
            );
            anyhow::ensure!(
                (1..=MAX_QUEUE_WAIT_SECONDS).contains(&group.max_queue_wait_seconds),
                "resource group '{name}': max_queue_wait_seconds must be between 1 and {MAX_QUEUE_WAIT_SECONDS}"
            );
            anyhow::ensure!(
                (1..=MAX_PRIORITY).contains(&group.priority),
                "resource group '{name}': priority must be between 1 and {MAX_PRIORITY}"
            );
            if let Some(share) = group.max_memory_bytes {
                anyhow::ensure!(
                    share >= 1 && share <= config.memory_admission_limit_bytes,
                    "resource group '{name}': max_memory_bytes must be between 1 and the admission limit of {} bytes",
                    config.memory_admission_limit_bytes
                );
            }
            if let Some(threads) = group.max_local_parallelism {
                anyhow::ensure!(
                    threads >= 1,
                    "resource group '{name}': max_local_parallelism must be at least 1"
                );
            }
            crate::settings::QuerySettings::from_request(&group.default_settings, config)
                .map_err(|error| anyhow::anyhow!("resource group '{name}': default {error}"))?;
        }
        anyhow::ensure!(
            names.contains(DEFAULT_ADMISSION_GROUP),
            "resource groups must include a '{DEFAULT_ADMISSION_GROUP}' group"
        );
        for (index, selector) in self.selectors.iter().enumerate() {
            anyhow::ensure!(
                names.contains(selector.group.as_str()),
                "selector {index} names resource group '{}', which does not exist",
                selector.group
            );
            for (field, value) in [
                ("principal", &selector.principal),
                ("principal_prefix", &selector.principal_prefix),
                ("client_tag", &selector.client_tag),
            ] {
                if let Some(value) = value {
                    anyhow::ensure!(
                        !value.trim().is_empty() && value == value.trim(),
                        "selector {index}: {field} must be nonempty and trimmed"
                    );
                }
            }
            anyhow::ensure!(
                !selector.is_catch_all() || index + 1 == self.selectors.len(),
                "selector {index} matches everything, so the selectors after it are unreachable"
            );
        }
        Ok(())
    }

    /// The group the ordered selectors pick for a request; `default` when
    /// none matches.
    pub fn select(&self, identity: &Identity, client_tags: &[String]) -> &ResourceGroup {
        let name = self
            .selectors
            .iter()
            .find(|selector| selector.matches(identity, client_tags))
            .map_or(DEFAULT_ADMISSION_GROUP, |selector| selector.group.as_str());
        self.group(name)
            .or_else(|| self.group(DEFAULT_ADMISSION_GROUP))
            .expect("a validated configuration has a default group")
    }

    pub fn group(&self, name: &str) -> Option<&ResourceGroup> {
        self.groups.iter().find(|group| group.name == name)
    }

    /// The controller's policies for these groups over `pool_bytes`.
    pub fn policies(&self, pool_bytes: u64) -> Vec<AdmissionGroupPolicy> {
        self.groups
            .iter()
            .map(|group| AdmissionGroupPolicy {
                name: group.name.clone(),
                limit_bytes: group.max_memory_bytes.unwrap_or(pool_bytes).min(pool_bytes),
                max_concurrent: group.max_concurrent,
                max_queued: group.max_queued,
                weight: group.priority,
            })
            .collect()
    }
}

/// Parses a JSON or TOML document (by the path's extension; anything but
/// `.toml` is JSON).
pub fn parse_file(path: &Path, content: &str) -> anyhow::Result<ResourceGroups> {
    if path.extension().is_some_and(|ext| ext == "toml") {
        Ok(toml::from_str(content)?)
    } else {
        Ok(serde_json::from_str(content)?)
    }
}

/// The configuration in force on the coordinator, replaceable at runtime.
pub struct Governor {
    current: RwLock<Arc<ResourceGroups>>,
    source: RwLock<Source>,
    /// Where a runtime replacement is written so it survives a restart.
    store_path: PathBuf,
}

impl Governor {
    pub fn new(groups: ResourceGroups, source: Source, store_path: PathBuf) -> Self {
        Self {
            current: RwLock::new(Arc::new(groups)),
            source: RwLock::new(source),
            store_path,
        }
    }

    pub fn current(&self) -> Arc<ResourceGroups> {
        Arc::clone(
            &self
                .current
                .read()
                .unwrap_or_else(|poisoned| poisoned.into_inner()),
        )
    }

    pub fn source(&self) -> Source {
        *self
            .source
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    pub fn store_path(&self) -> &Path {
        &self.store_path
    }

    /// Applies a validated replacement: the controller's policies first
    /// (so an admission decision never runs under groups the controller
    /// does not know), then the durable copy, then the selectors.
    pub fn replace(&self, state: &AppState, groups: ResourceGroups) -> anyhow::Result<()> {
        groups.validate(&state.config)?;
        state
            .memory_admission
            .set_groups(groups.policies(state.config.memory_admission_limit_bytes))
            .map_err(|error| anyhow::anyhow!("{error}"))?;
        write_durable(&self.store_path, &groups)?;
        *self
            .current
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = Arc::new(groups);
        *self
            .source
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = Source::Runtime;
        Ok(())
    }
}

/// Writes the configuration whole: a temporary file, synced, then renamed
/// over the target, so a crash leaves the previous copy intact.
fn write_durable(path: &Path, groups: &ResourceGroups) -> anyhow::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let content = if path.extension().is_some_and(|ext| ext == "toml") {
        toml::to_string_pretty(groups)?
    } else {
        serde_json::to_string_pretty(groups)?
    };
    let temporary = path.with_extension("tmp");
    {
        let mut file = std::fs::File::create(&temporary)?;
        std::io::Write::write_all(&mut file, content.as_bytes())?;
        file.sync_all()?;
    }
    std::fs::rename(&temporary, path)?;
    Ok(())
}

/// The loaded configuration and where it came from, by precedence.
pub fn load(
    config: &ServerConfig,
    section: Option<ResourceGroups>,
) -> anyhow::Result<(ResourceGroups, Source, PathBuf)> {
    let runtime_path = config.state_dir.join("resource-groups.json");
    let (groups, source, store_path) = if let Some(path) = &config.resource_groups_path {
        let content = std::fs::read_to_string(path).map_err(|error| {
            anyhow::anyhow!("KAVEON_RESOURCE_GROUPS {}: {error}", path.display())
        })?;
        let groups = parse_file(path, &content).map_err(|error| {
            anyhow::anyhow!("KAVEON_RESOURCE_GROUPS {}: {error}", path.display())
        })?;
        (groups, Source::Environment, path.clone())
    } else if runtime_path.is_file() {
        let content = std::fs::read_to_string(&runtime_path)?;
        let groups = parse_file(&runtime_path, &content).map_err(|error| {
            anyhow::anyhow!(
                "runtime resource groups {}: {error}",
                runtime_path.display()
            )
        })?;
        (groups, Source::Runtime, runtime_path)
    } else if let Some(section) = section {
        (section, Source::ConfigFile, runtime_path)
    } else if !config.security.resource_groups.is_empty() {
        (
            ResourceGroups::from_legacy(&config.security.resource_groups, config),
            Source::LegacySecurity,
            runtime_path,
        )
    } else {
        (
            ResourceGroups::builtin(config),
            Source::Builtin,
            runtime_path,
        )
    };
    groups
        .validate(config)
        .map_err(|error| anyhow::anyhow!("resource groups ({source:?}): {error}"))?;
    Ok((groups, source, store_path))
}

fn forbidden() -> Response {
    (
        StatusCode::FORBIDDEN,
        Json(serde_json::json!({
            "error": "resource groups require admin role",
            "code": "FORBIDDEN"
        })),
    )
        .into_response()
}

fn not_coordinator() -> Response {
    (
        StatusCode::BAD_REQUEST,
        Json(serde_json::json!({
            "error": "resource groups are managed on the coordinator",
            "code": "NOT_COORDINATOR"
        })),
    )
        .into_response()
}

fn document(state: &AppState) -> Value {
    let groups = state.governance.current();
    serde_json::json!({
        "source": state.governance.source(),
        "store_path": state.governance.store_path().display().to_string(),
        "admission_limit_bytes": state.config.memory_admission_limit_bytes,
        "groups": groups.groups,
        "selectors": groups.selectors,
        "counters": state.memory_admission.group_stats(),
    })
}

/// `GET /v1/admin/resource-groups`: the configuration in force, its
/// source, and each group's counters.
pub async fn get_resource_groups(
    State(state): State<Arc<AppState>>,
    Extension(identity): Extension<Identity>,
) -> Response {
    if identity.role != Role::Admin {
        return forbidden();
    }
    if !state.config.coordinator {
        return not_coordinator();
    }
    Json(document(&state)).into_response()
}

/// `PUT /v1/admin/resource-groups`: replaces every group and selector at
/// once after validation; takes effect for the next admission decision,
/// waiting statements included, and survives a restart.
pub async fn put_resource_groups(
    State(state): State<Arc<AppState>>,
    Extension(identity): Extension<Identity>,
    body: axum::body::Bytes,
) -> Response {
    if identity.role != Role::Admin {
        return forbidden();
    }
    if !state.config.coordinator {
        return not_coordinator();
    }
    let groups: ResourceGroups = match serde_json::from_slice(&body) {
        Ok(groups) => groups,
        Err(error) => {
            return (
                StatusCode::BAD_REQUEST,
                Json(serde_json::json!({
                    "error": format!("resource groups: {error}"),
                    "code": "INVALID_RESOURCE_GROUPS"
                })),
            )
                .into_response();
        }
    };
    let before = state.governance.current();
    if let Err(error) = state.governance.replace(&state, groups) {
        return (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({
                "error": error.to_string(),
                "code": "INVALID_RESOURCE_GROUPS"
            })),
        )
            .into_response();
    }
    let after = state.governance.current();
    state.audit.record(crate::audit::AuditRecord::settings(
        &identity,
        crate::audit::KIND_SETTINGS_RESOURCE_GROUPS,
        serde_json::json!({
            "groups_before": before.groups.iter().map(|group| &group.name).collect::<Vec<_>>(),
            "groups_after": after.groups.iter().map(|group| &group.name).collect::<Vec<_>>(),
            "selectors_before": before.selectors.len(),
            "selectors_after": after.selectors.len(),
        }),
    ));
    Json(document(&state)).into_response()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn identity(principal: &str, role: Role) -> Identity {
        Identity {
            principal: principal.into(),
            display_identity: None,
            role,
        }
    }

    fn group(name: &str) -> ResourceGroup {
        ResourceGroup {
            name: name.into(),
            max_memory_bytes: None,
            max_concurrent: 2,
            max_queued: 4,
            max_queue_wait_seconds: 5,
            max_local_parallelism: None,
            priority: 1,
            default_settings: Map::new(),
        }
    }

    #[test]
    fn selectors_are_evaluated_in_order_every_matcher_must_hold_and_default_catches_the_rest() {
        let config = ServerConfig::default();
        let groups = ResourceGroups {
            groups: vec![
                group("default"),
                group("interactive"),
                group("etl"),
                group("admins"),
            ],
            selectors: vec![
                Selector {
                    principal: Some("alice".into()),
                    group: "interactive".into(),
                    ..Selector::default()
                },
                Selector {
                    principal_prefix: Some("svc-".into()),
                    client_tag: Some("etl".into()),
                    group: "etl".into(),
                    ..Selector::default()
                },
                Selector {
                    role: Some(Role::Admin),
                    group: "admins".into(),
                    ..Selector::default()
                },
            ],
        };
        groups.validate(&config).unwrap();
        let none: &[String] = &[];
        assert_eq!(
            groups.select(&identity("alice", Role::Admin), none).name,
            "interactive"
        );
        assert_eq!(
            groups
                .select(&identity("svc-loader", Role::Analyst), none)
                .name,
            "default"
        );
        assert_eq!(
            groups
                .select(&identity("svc-loader", Role::Analyst), &["etl".to_owned()])
                .name,
            "etl"
        );
        assert_eq!(
            groups.select(&identity("bob", Role::Admin), none).name,
            "admins"
        );
        assert_eq!(
            groups.select(&identity("bob", Role::Reader), none).name,
            "default"
        );
    }

    #[test]
    fn validation_refuses_what_the_controller_or_the_statement_path_cannot_honour() {
        let config = ServerConfig::default();
        let valid = ResourceGroups {
            groups: vec![group("default")],
            selectors: vec![],
        };
        valid.validate(&config).unwrap();
        let without_default = ResourceGroups {
            groups: vec![group("other")],
            selectors: vec![],
        };
        assert!(
            without_default
                .validate(&config)
                .unwrap_err()
                .to_string()
                .contains("default")
        );
        let over_pool = ResourceGroups {
            groups: vec![ResourceGroup {
                max_memory_bytes: Some(config.memory_admission_limit_bytes + 1),
                ..group("default")
            }],
            selectors: vec![],
        };
        assert!(
            over_pool
                .validate(&config)
                .unwrap_err()
                .to_string()
                .contains("max_memory_bytes")
        );
        let bad_setting = ResourceGroups {
            groups: vec![ResourceGroup {
                default_settings: [("result_cache".to_owned(), Value::from("maybe"))]
                    .into_iter()
                    .collect(),
                ..group("default")
            }],
            selectors: vec![],
        };
        assert!(
            bad_setting
                .validate(&config)
                .unwrap_err()
                .to_string()
                .contains("result_cache")
        );
        let unknown_group = ResourceGroups {
            groups: vec![group("default")],
            selectors: vec![Selector {
                principal: Some("alice".into()),
                group: "missing".into(),
                ..Selector::default()
            }],
        };
        assert!(
            unknown_group
                .validate(&config)
                .unwrap_err()
                .to_string()
                .contains("missing")
        );
        let unreachable = ResourceGroups {
            groups: vec![group("default")],
            selectors: vec![
                Selector {
                    group: "default".into(),
                    ..Selector::default()
                },
                Selector {
                    principal: Some("alice".into()),
                    group: "default".into(),
                    ..Selector::default()
                },
            ],
        };
        assert!(
            unreachable
                .validate(&config)
                .unwrap_err()
                .to_string()
                .contains("unreachable")
        );
        let duplicate = ResourceGroups {
            groups: vec![group("default"), group("default")],
            selectors: vec![],
        };
        assert!(duplicate.validate(&config).is_err());
        let zero_priority = ResourceGroups {
            groups: vec![ResourceGroup {
                priority: 0,
                ..group("default")
            }],
            selectors: vec![],
        };
        assert!(zero_priority.validate(&config).is_err());
    }

    #[test]
    fn the_legacy_security_list_and_the_builtin_translate_to_groups() {
        let config = ServerConfig {
            principal_query_limit: 3,
            ..ServerConfig::default()
        };
        let builtin = ResourceGroups::builtin(&config);
        builtin.validate(&config).unwrap();
        assert_eq!(builtin.groups[0].max_concurrent, 3);
        let legacy = vec![
            crate::security::ResourceGroupConfig {
                name: "interactive".into(),
                principals: vec!["alice".into()],
                max_running: 1,
                max_queued: 2,
                queue_timeout_ms: 1_500,
            },
            crate::security::ResourceGroupConfig {
                name: "everyone".into(),
                principals: vec!["*".into()],
                max_running: 4,
                max_queued: 8,
                queue_timeout_ms: 60_000,
            },
        ];
        let translated = ResourceGroups::from_legacy(&legacy, &config);
        translated.validate(&config).unwrap();
        assert_eq!(translated.groups.len(), 3);
        assert_eq!(translated.groups[0].max_queue_wait_seconds, 2);
        let none: &[String] = &[];
        assert_eq!(
            translated
                .select(&identity("alice", Role::Analyst), none)
                .name,
            "interactive"
        );
        assert_eq!(
            translated
                .select(&identity("bob", Role::Analyst), none)
                .name,
            "everyone"
        );
        let policies = translated.policies(1 << 30);
        assert_eq!(policies.len(), 3);
        assert!(policies.iter().all(|policy| policy.limit_bytes == 1 << 30));
    }

    #[test]
    fn a_toml_and_a_json_document_parse_to_the_same_configuration() {
        let json = r#"{"groups":[{"name":"default","max_concurrent":4}],"selectors":[{"role":"admin","group":"default"}]}"#;
        let toml = "[[groups]]\nname = \"default\"\nmax_concurrent = 4\n\n[[selectors]]\nrole = \"admin\"\ngroup = \"default\"\n";
        let from_json = parse_file(Path::new("groups.json"), json).unwrap();
        let from_toml = parse_file(Path::new("groups.toml"), toml).unwrap();
        assert_eq!(from_json, from_toml);
        assert_eq!(from_json.groups[0].max_queued, 16);
        assert_eq!(from_json.groups[0].priority, 1);
    }
}
