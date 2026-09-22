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
//!
//! The demo posture lives here too: `demo.enabled` at the top level, and a
//! group's `rate` — a per-principal quota of live statements over a
//! rolling window, counted from the audit ledger's statement records so it
//! survives a restart, indexed in memory for the decision. A statement is
//! charged when it reaches the row path, after the result cache and the
//! statistics have declined it; the ledger's terminal line carries the
//! charge, and the index is rebuilt from those lines at start and after a
//! replacement. `GET /v1/quota` reports the caller's own standing.

use crate::AppState;
use crate::audit::{AuditFilter, AuditLedger, format_time, unix_ms};
use crate::config::ServerConfig;
use crate::security::{Identity, Role};
use axum::extract::State;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::{Extension, Json};
use kaveon_core::{AdmissionGroupPolicy, DEFAULT_ADMISSION_GROUP};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use std::collections::{HashMap, HashSet, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, RwLock};

const MAX_CONCURRENT_BOUND: usize = 10_000;
const MAX_QUEUED_BOUND: usize = 10_000;
const MAX_QUEUE_WAIT_SECONDS: u64 = 86_400;
const MAX_PRIORITY: u32 = 1_000;
const MAX_GROUPS: usize = 256;
const MAX_SELECTORS: usize = 4_096;
const MAX_RATE_STATEMENTS: u32 = 100_000;
/// The longest quota window: 31 days, the ledger's default retention
/// covers it three times over.
const MAX_RATE_WINDOW_SECONDS: u64 = 31 * 86_400;
pub const RATE_LIMITED: &str = "RATE_LIMITED";

/// What a group's `rate` counts.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RateCount {
    /// Statements that ran on the row path — distributed across the workers
    /// or on the coordinator. An answer from the result cache or from the
    /// statistics, and a statement refused before it ran, are not counted.
    #[default]
    Live,
}

/// A per-principal statement quota over a rolling window; the demo
/// posture's bound. Enforced on the coordinator for every client.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RateLimit {
    /// Statements a principal of the group may run in any window.
    pub max_statements: u32,
    /// The window, rolling: a charge leaves it this long after it was made.
    pub per_seconds: u64,
    #[serde(default)]
    pub count: RateCount,
}

impl RateLimit {
    pub fn window_ms(&self) -> u64 {
        self.per_seconds.saturating_mul(1_000)
    }

    /// `5 live queries per 6 hours`.
    pub fn describe(&self) -> String {
        let noun = if self.max_statements == 1 {
            "query"
        } else {
            "queries"
        };
        format!(
            "{} live {noun} per {}",
            self.max_statements,
            describe_seconds(self.per_seconds)
        )
    }
}

/// `6 hours`, `90 minutes`, `1 day`, `45 seconds`: the largest unit that
/// divides the count.
fn describe_seconds(seconds: u64) -> String {
    let (count, unit) = if seconds.is_multiple_of(86_400) {
        (seconds / 86_400, "day")
    } else if seconds.is_multiple_of(3_600) {
        (seconds / 3_600, "hour")
    } else if seconds.is_multiple_of(60) {
        (seconds / 60, "minute")
    } else {
        (seconds, "second")
    };
    if count == 1 {
        format!("1 {unit}")
    } else {
        format!("{count} {unit}s")
    }
}

/// The demo posture switch of the coordinator: on, the groups' `rate`
/// quotas are enforced and reported; off (the default), a `rate` is
/// refused at validation so a self-hosted install cannot carry one
/// unknowingly.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Demo {
    #[serde(default)]
    pub enabled: bool,
}

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
    /// The per-principal quota of live statements over a rolling window;
    /// needs `demo.enabled`. Admins are exempt.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rate: Option<RateLimit>,
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
    /// The demo posture; off unless the document says so.
    #[serde(default)]
    pub demo: Demo,
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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rate: Option<RateLimit>,
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
            rate: group.rate.clone(),
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
                rate: None,
                default_settings: Map::new(),
            }],
            selectors: Vec::new(),
            demo: Demo::default(),
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
                rate: None,
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
        Self {
            groups,
            selectors,
            demo: Demo::default(),
        }
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
            if let Some(rate) = &group.rate {
                anyhow::ensure!(
                    self.demo.enabled,
                    "resource group '{name}': rate is the demo posture's quota and needs demo.enabled = true"
                );
                anyhow::ensure!(
                    (1..=MAX_RATE_STATEMENTS).contains(&rate.max_statements),
                    "resource group '{name}': rate.max_statements must be between 1 and {MAX_RATE_STATEMENTS}"
                );
                anyhow::ensure!(
                    (1..=MAX_RATE_WINDOW_SECONDS).contains(&rate.per_seconds),
                    "resource group '{name}': rate.per_seconds must be between 1 and {MAX_RATE_WINDOW_SECONDS}"
                );
                anyhow::ensure!(
                    rate.per_seconds <= config.audit_retention_days.saturating_mul(86_400),
                    "resource group '{name}': rate.per_seconds exceeds the audit ledger's retention of {} day(s), which the quota is counted from across restarts",
                    config.audit_retention_days
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

    /// The widest quota window any group carries, in milliseconds; zero
    /// when none does.
    fn widest_rate_window_ms(&self) -> u64 {
        self.groups
            .iter()
            .filter_map(|group| group.rate.as_ref())
            .map(RateLimit::window_ms)
            .max()
            .unwrap_or(0)
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
    /// The quota charges in every group's window, by principal.
    quota: QuotaIndex,
}

impl Governor {
    pub fn new(groups: ResourceGroups, source: Source, store_path: PathBuf) -> Self {
        Self {
            current: RwLock::new(Arc::new(groups)),
            source: RwLock::new(source),
            store_path,
            quota: QuotaIndex::default(),
        }
    }

    /// The standing of `identity` under `group`'s quota at `now_ms`, or
    /// `None` when no quota applies: the group has no `rate`, or the
    /// principal is an admin.
    pub fn quota(
        &self,
        ledger: &AuditLedger,
        group: &EffectiveGroup,
        identity: &Identity,
        now_ms: u64,
    ) -> Option<QuotaStatus> {
        let rate = group.rate.as_ref()?;
        if identity.role == Role::Admin {
            return None;
        }
        self.quota
            .prime(ledger, self.current().widest_rate_window_ms(), now_ms);
        Some(
            self.quota
                .status(rate, &group.name, &identity.principal, now_ms),
        )
    }

    /// Charges one live statement of `identity` against `group`'s quota:
    /// `Ok(None)` when no quota applies, `Ok(Some(charge))` with the
    /// standing after the charge, `Err` — nothing charged — when the window
    /// already holds `max_statements`.
    pub fn charge(
        &self,
        ledger: &AuditLedger,
        group: &EffectiveGroup,
        identity: &Identity,
        query_id: &str,
        now_ms: u64,
    ) -> Result<Option<QuotaCharge>, QuotaRefusal> {
        let Some(rate) = group.rate.as_ref() else {
            return Ok(None);
        };
        if identity.role == Role::Admin {
            return Ok(None);
        }
        self.quota
            .prime(ledger, self.current().widest_rate_window_ms(), now_ms);
        self.quota
            .charge(rate, &group.name, &identity.principal, query_id, now_ms)
            .map(Some)
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
        // A wider window than before needs older charges from the ledger;
        // the next decision reads them in.
        self.quota.primed.store(false, Ordering::Release);
        Ok(())
    }
}

/// One statement's charge against its group's quota, as the query record
/// and the ledger carry it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct QuotaCharge {
    /// When the statement entered the row path and was charged.
    pub charged_at_ms: u64,
    /// Charges in the window after this one, and what remains.
    pub used: u32,
    pub remaining: u32,
    pub max_statements: u32,
    pub per_seconds: u64,
}

/// A principal's standing under a group's quota, as `GET /v1/quota`
/// reports it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct QuotaStatus {
    pub max_statements: u32,
    pub per_seconds: u64,
    pub count: RateCount,
    pub resource_group: String,
    /// Charges in the window now.
    pub used: u32,
    pub remaining: u32,
    /// When the oldest charge leaves the window — the next time `remaining`
    /// grows; absent when nothing is charged.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub resets_at: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub resets_at_ms: Option<u64>,
    /// When the next statement is allowed: now when `remaining` is above
    /// zero, else `resets_at`.
    pub next_allowed_at: String,
    pub next_allowed_at_ms: u64,
}

/// A statement refused because its principal's window is full.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct QuotaRefusal {
    pub limit: RateLimit,
    pub resource_group: String,
    pub next_allowed_at_ms: u64,
}

impl QuotaRefusal {
    /// `5 live queries per 6 hours in this demo; the next is allowed at
    /// 2026-09-22T14:20:00Z`.
    pub fn message(&self) -> String {
        format!(
            "{} in this demo; the next is allowed at {}",
            self.limit.describe(),
            format_time(self.next_allowed_at_ms)
        )
    }

    pub fn retry_after_seconds(&self, now_ms: u64) -> u64 {
        self.next_allowed_at_ms
            .saturating_sub(now_ms)
            .div_ceil(1_000)
    }

    /// The HTTP 429 the coordinator answers with, and the `Retry-After`.
    pub fn response(&self, now_ms: u64) -> Response {
        let retry_after = self.retry_after_seconds(now_ms);
        let message = self.message();
        let body = serde_json::json!({
            "error": message,
            "code": RATE_LIMITED,
            "message": message,
            "retry_after_seconds": retry_after,
            "next_allowed_at": format_time(self.next_allowed_at_ms),
            "next_allowed_at_ms": self.next_allowed_at_ms,
            "resource_group": self.resource_group,
            "limit": self.limit,
        });
        (
            StatusCode::TOO_MANY_REQUESTS,
            [(axum::http::header::RETRY_AFTER, retry_after.to_string())],
            Json(body),
        )
            .into_response()
    }
}

/// One charge in the index: when, and the statement it was for, so a
/// rebuild from the ledger never counts a statement twice.
#[derive(Clone, Debug)]
struct Charge {
    at_ms: u64,
    query_id: String,
}

/// The in-memory window index over the ledger's charges: per principal,
/// oldest first. The ledger is the durable record — every terminal
/// statement line carries `quota_charged_at_ms` when the statement was
/// charged — and this is read from it once at the first decision after a
/// start or a replacement, then kept up by the charges themselves.
#[derive(Default)]
struct QuotaIndex {
    charges: Mutex<HashMap<String, VecDeque<Charge>>>,
    primed: AtomicBool,
}

impl QuotaIndex {
    /// Reads the ledger's charges of the last `window_ms` into the index,
    /// once per priming. Charges already indexed (a statement in flight
    /// across a replacement) are kept.
    fn prime(&self, ledger: &AuditLedger, window_ms: u64, now_ms: u64) {
        if self.primed.swap(true, Ordering::AcqRel) {
            return;
        }
        let mut charges = self
            .charges
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let known: HashSet<String> = charges
            .values()
            .flat_map(|queue| queue.iter().map(|charge| charge.query_id.clone()))
            .collect();
        // A charge is made when the statement starts; its line lands when
        // it ends. A line since `now - window` holds every charge still in
        // any window, and the index prunes what is older at the decision.
        let mut filter = AuditFilter {
            since_ms: Some(now_ms.saturating_sub(window_ms)),
            kinds: vec![
                crate::audit::KIND_STATEMENT_FINISHED.to_owned(),
                crate::audit::KIND_STATEMENT_FAILED.to_owned(),
                crate::audit::KIND_STATEMENT_CANCELED.to_owned(),
            ],
            ..AuditFilter::default()
        };
        let mut read: Vec<(String, Charge)> = Vec::new();
        while let Ok(page) = ledger.query(&filter, crate::audit::MAX_PAGE) {
            for record in &page.records {
                let (Some(at_ms), Some(principal), Some(query_id)) = (
                    record.quota_charged_at_ms,
                    record.principal.as_deref(),
                    record.query_id.as_deref(),
                ) else {
                    continue;
                };
                if known.contains(query_id) {
                    continue;
                }
                read.push((
                    principal.to_owned(),
                    Charge {
                        at_ms,
                        query_id: query_id.to_owned(),
                    },
                ));
            }
            match page.next_cursor {
                Some(cursor) => filter.after_seq = Some(cursor),
                None => break,
            }
        }
        for (principal, charge) in read {
            charges.entry(principal).or_default().push_back(charge);
        }
        for queue in charges.values_mut() {
            queue.make_contiguous().sort_by_key(|charge| charge.at_ms);
        }
    }

    fn status(&self, rate: &RateLimit, group: &str, principal: &str, now_ms: u64) -> QuotaStatus {
        let mut charges = self
            .charges
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let queue = charges.entry(principal.to_owned()).or_default();
        prune(queue, rate, now_ms);
        let used = queue.len().min(rate.max_statements as usize) as u32;
        let remaining = rate.max_statements - used;
        let resets_at_ms = queue.front().map(|oldest| oldest.at_ms + rate.window_ms());
        let next_allowed_at_ms = if remaining > 0 {
            now_ms
        } else {
            resets_at_ms.unwrap_or(now_ms)
        };
        QuotaStatus {
            max_statements: rate.max_statements,
            per_seconds: rate.per_seconds,
            count: rate.count,
            resource_group: group.to_owned(),
            used,
            remaining,
            resets_at: resets_at_ms.map(format_time),
            resets_at_ms,
            next_allowed_at: format_time(next_allowed_at_ms),
            next_allowed_at_ms,
        }
    }

    fn charge(
        &self,
        rate: &RateLimit,
        group: &str,
        principal: &str,
        query_id: &str,
        now_ms: u64,
    ) -> Result<QuotaCharge, QuotaRefusal> {
        let mut charges = self
            .charges
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let queue = charges.entry(principal.to_owned()).or_default();
        prune(queue, rate, now_ms);
        if queue.len() >= rate.max_statements as usize {
            let oldest = queue.front().map_or(now_ms, |charge| charge.at_ms);
            return Err(QuotaRefusal {
                limit: rate.clone(),
                resource_group: group.to_owned(),
                next_allowed_at_ms: oldest + rate.window_ms(),
            });
        }
        queue.push_back(Charge {
            at_ms: now_ms,
            query_id: query_id.to_owned(),
        });
        let used = queue.len() as u32;
        Ok(QuotaCharge {
            charged_at_ms: now_ms,
            used,
            remaining: rate.max_statements - used,
            max_statements: rate.max_statements,
            per_seconds: rate.per_seconds,
        })
    }
}

/// Drops the charges that have left the window.
fn prune(queue: &mut VecDeque<Charge>, rate: &RateLimit, now_ms: u64) {
    let cutoff = now_ms.saturating_sub(rate.window_ms());
    while queue.front().is_some_and(|charge| charge.at_ms <= cutoff) {
        queue.pop_front();
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
        "demo": groups.demo,
        "groups": groups.groups,
        "selectors": groups.selectors,
        "counters": state.memory_admission.group_stats(),
    })
}

/// `GET /v1/quota`: the caller's own standing under the demo quota —
/// `demo.enabled`, the group the selectors pick for them (with no client
/// tags), whether they are exempt, and `quota` (null when none applies).
pub async fn get_quota(
    State(state): State<Arc<AppState>>,
    Extension(identity): Extension<Identity>,
) -> Response {
    if !state.config.coordinator {
        return not_coordinator();
    }
    let groups = state.governance.current();
    let group = EffectiveGroup::from(groups.select(&identity, &[]));
    let exempt = identity.role == Role::Admin && group.rate.is_some();
    let quota = state
        .governance
        .quota(&state.audit, &group, &identity, unix_ms());
    Json(serde_json::json!({
        "demo": groups.demo,
        "principal": identity.principal,
        "resource_group": group.name,
        "exempt": exempt,
        "quota": quota,
    }))
    .into_response()
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
            rate: None,
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
            demo: Demo::default(),
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
            demo: Demo::default(),
        };
        valid.validate(&config).unwrap();
        let without_default = ResourceGroups {
            groups: vec![group("other")],
            selectors: vec![],
            demo: Demo::default(),
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
            demo: Demo::default(),
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
            demo: Demo::default(),
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
            demo: Demo::default(),
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
            demo: Demo::default(),
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
            demo: Demo::default(),
        };
        assert!(duplicate.validate(&config).is_err());
        let zero_priority = ResourceGroups {
            groups: vec![ResourceGroup {
                priority: 0,
                ..group("default")
            }],
            selectors: vec![],
            demo: Demo::default(),
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
        assert_eq!(from_json.demo, Demo::default());
        assert!(from_json.groups[0].rate.is_none());
    }

    fn rate(max_statements: u32, per_seconds: u64) -> RateLimit {
        RateLimit {
            max_statements,
            per_seconds,
            count: RateCount::Live,
        }
    }

    fn demo_groups(rate: Option<RateLimit>, enabled: bool) -> ResourceGroups {
        ResourceGroups {
            groups: vec![
                group("default"),
                ResourceGroup {
                    rate,
                    ..group("demo")
                },
            ],
            selectors: vec![Selector {
                group: "demo".into(),
                ..Selector::default()
            }],
            demo: Demo { enabled },
        }
    }

    #[test]
    fn a_rate_needs_the_demo_posture_and_lies_within_the_ledger_retention() {
        let config = ServerConfig::default();
        demo_groups(Some(rate(5, 21_600)), true)
            .validate(&config)
            .unwrap();
        demo_groups(None, true).validate(&config).unwrap();
        demo_groups(None, false).validate(&config).unwrap();
        let off = demo_groups(Some(rate(5, 21_600)), false)
            .validate(&config)
            .unwrap_err()
            .to_string();
        assert!(off.contains("demo.enabled"), "{off}");
        for (limit, field) in [
            (rate(0, 21_600), "max_statements"),
            (rate(5, 0), "per_seconds"),
            (rate(5, MAX_RATE_WINDOW_SECONDS + 1), "per_seconds"),
        ] {
            let error = demo_groups(Some(limit), true)
                .validate(&config)
                .unwrap_err()
                .to_string();
            assert!(error.contains(field), "{error}");
        }
        let short_ledger = ServerConfig {
            audit_retention_days: 0,
            ..ServerConfig::default()
        };
        let error = demo_groups(Some(rate(5, 21_600)), true)
            .validate(&short_ledger)
            .unwrap_err()
            .to_string();
        assert!(error.contains("retention"), "{error}");
        let json = r#"{"demo":{"enabled":true},"groups":[{"name":"default","max_concurrent":4,"rate":{"max_statements":5,"per_seconds":21600,"count":"live"}}]}"#;
        let parsed = parse_file(Path::new("groups.json"), json).unwrap();
        assert_eq!(parsed.groups[0].rate, Some(rate(5, 21_600)));
        assert!(parsed.demo.enabled);
        assert!(
            parse_file(
                Path::new("groups.json"),
                r#"{"groups":[{"name":"default","max_concurrent":4,"rate":{"max_statements":5,"per_seconds":21600,"count":"all"}}]}"#
            )
            .is_err()
        );
        let effective = EffectiveGroup::from(&parsed.groups[0]);
        assert_eq!(effective.rate, Some(rate(5, 21_600)));
        assert_eq!(rate(5, 21_600).describe(), "5 live queries per 6 hours");
        assert_eq!(rate(1, 60).describe(), "1 live query per 1 minute");
        assert_eq!(rate(3, 172_800).describe(), "3 live queries per 2 days");
        assert_eq!(rate(2, 90).describe(), "2 live queries per 90 seconds");
    }

    #[test]
    fn the_window_admits_max_statements_refuses_the_next_with_the_reset_time_and_rolls() {
        let index = QuotaIndex::default();
        index.primed.store(true, Ordering::Release);
        let limit = rate(5, 21_600);
        let start = 1_789_776_000_000;
        for n in 1..=5 {
            let charge = index
                .charge(&limit, "demo", "alice", &format!("q{n}"), start + n * 1_000)
                .unwrap();
            assert_eq!((charge.used, charge.remaining), (n as u32, 5 - n as u32));
            assert_eq!(charge.charged_at_ms, start + n * 1_000);
        }
        let refusal = index
            .charge(&limit, "demo", "alice", "q6", start + 6_000)
            .unwrap_err();
        assert_eq!(refusal.next_allowed_at_ms, start + 1_000 + 21_600_000);
        assert_eq!(refusal.retry_after_seconds(start + 6_000), 21_595);
        assert_eq!(
            refusal.message(),
            "5 live queries per 6 hours in this demo; the next is allowed at 2026-09-19T06:00:01Z"
        );
        // Another principal has its own window.
        assert!(
            index
                .charge(&limit, "demo", "bob", "b1", start + 6_000)
                .is_ok()
        );
        let standing = index.status(&limit, "demo", "alice", start + 6_000);
        assert_eq!((standing.used, standing.remaining), (5, 0));
        assert_eq!(standing.resets_at_ms, Some(start + 1_000 + 21_600_000));
        assert_eq!(standing.next_allowed_at_ms, start + 1_000 + 21_600_000);
        // The oldest charge leaves the window and one slot returns.
        let later = start + 1_000 + 21_600_000;
        let standing = index.status(&limit, "demo", "alice", later);
        assert_eq!((standing.used, standing.remaining), (4, 1));
        assert_eq!(standing.next_allowed_at_ms, later);
        assert_eq!(standing.resets_at_ms, Some(start + 2_000 + 21_600_000));
        assert!(index.charge(&limit, "demo", "alice", "q7", later).is_ok());
        assert!(index.charge(&limit, "demo", "alice", "q8", later).is_err());
        let nothing = index.status(&limit, "demo", "carol", later);
        assert_eq!((nothing.used, nothing.remaining), (0, 5));
        assert!(nothing.resets_at.is_none());
        assert_eq!(nothing.next_allowed_at_ms, later);
    }

    #[test]
    fn the_index_is_rebuilt_from_the_ledger_without_counting_a_statement_twice() {
        let directory = std::env::temp_dir().join(format!("kaveon-quota-{}", uuid::Uuid::new_v4()));
        let ledger =
            AuditLedger::open(&directory, 1 << 20, std::time::Duration::from_secs(86_400)).unwrap();
        let now = unix_ms();
        let line = |kind: &str, principal: &str, query_id: &str, charged: Option<u64>| {
            crate::audit::AuditRecord {
                principal: Some(principal.into()),
                query_id: Some(query_id.into()),
                mode: charged.map(|_| "distributed".to_owned()),
                quota_charged_at_ms: charged,
                ..crate::audit::AuditRecord::new(kind)
            }
        };
        // Two charged statements of alice inside the window, one outside it,
        // one answered from the cache, one refused before it ran, and bob's.
        ledger.record(line(
            crate::audit::KIND_STATEMENT_FINISHED,
            "alice",
            "a1",
            Some(now - 3_600_000),
        ));
        ledger.record(line(
            crate::audit::KIND_STATEMENT_FAILED,
            "alice",
            "a2",
            Some(now - 1_800_000),
        ));
        ledger.record(line(
            crate::audit::KIND_STATEMENT_CANCELED,
            "alice",
            "a0",
            Some(now - 30_000_000),
        ));
        ledger.record(line(
            crate::audit::KIND_STATEMENT_FINISHED,
            "alice",
            "a3",
            None,
        ));
        ledger.record(line(
            crate::audit::KIND_STATEMENT_REJECTED,
            "alice",
            "a4",
            None,
        ));
        ledger.record(line(
            crate::audit::KIND_STATEMENT_FINISHED,
            "bob",
            "b1",
            Some(now - 60_000),
        ));
        let limit = rate(3, 21_600);
        let index = QuotaIndex::default();
        // A statement in flight across the priming keeps its charge and is
        // not counted again when its line is read.
        index.primed.store(true, Ordering::Release);
        index
            .charge(&limit, "demo", "alice", "a2", now - 1_800_000)
            .unwrap();
        index.primed.store(false, Ordering::Release);
        index.prime(&ledger, limit.window_ms(), now);
        let alice = index.status(&limit, "demo", "alice", now);
        assert_eq!((alice.used, alice.remaining), (2, 1));
        assert_eq!(alice.resets_at_ms, Some(now - 3_600_000 + 21_600_000));
        let bob = index.status(&limit, "demo", "bob", now);
        assert_eq!((bob.used, bob.remaining), (1, 2));
        // Priming again is a no-op until a replacement asks for it.
        index.prime(&ledger, limit.window_ms(), now);
        assert_eq!(index.status(&limit, "demo", "alice", now).used, 2);
        // Through the governor: the ledger primes the first decision, an
        // admin is exempt, a group without a rate has no quota.
        let governor = Governor::new(
            demo_groups(Some(limit.clone()), true),
            Source::ConfigFile,
            directory.join("resource-groups.json"),
        );
        let current = governor.current();
        let alice_identity = identity("alice", Role::Analyst);
        let group = EffectiveGroup::from(current.select(&alice_identity, &[]));
        let standing = governor
            .quota(&ledger, &group, &alice_identity, now)
            .unwrap();
        assert_eq!((standing.used, standing.remaining), (2, 1));
        assert!(
            governor
                .quota(&ledger, &group, &identity("alice", Role::Admin), now)
                .is_none()
        );
        assert!(
            governor
                .charge(&ledger, &group, &identity("alice", Role::Admin), "x", now)
                .unwrap()
                .is_none()
        );
        let plain = EffectiveGroup::from(current.group("default").unwrap());
        assert!(
            governor
                .quota(&ledger, &plain, &alice_identity, now)
                .is_none()
        );
        let charge = governor
            .charge(&ledger, &group, &alice_identity, "a5", now)
            .unwrap()
            .unwrap();
        assert_eq!((charge.used, charge.remaining), (3, 0));
        assert!(
            governor
                .charge(&ledger, &group, &alice_identity, "a6", now)
                .is_err()
        );
        ledger.shutdown();
        std::fs::remove_dir_all(directory).unwrap();
    }
}
