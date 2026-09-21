use crate::AppState;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use crate::config::ServerConfig;

const HEARTBEAT_INTERVAL: Duration = Duration::from_secs(10);
const NODE_EXPIRY: Duration = Duration::from_secs(30);
/// A worker whose last heartbeat is older than this many intervals has
/// lapsed: with a probe of its `/v1/node` that does not answer, the
/// coordinator treats it as lost (`ClusterState::heartbeat_lapsed`).
const WORKER_LOSS_MISSED_HEARTBEATS: u32 = 2;
/// How long the coordinator gives a lost-worker probe.
pub const WORKER_PROBE_TIMEOUT: Duration = Duration::from_secs(5);
const KIBIBYTE_BYTES: u64 = 1024;
const MAX_CATALOG_REPLICA_BYTES: usize = 16 * 1024 * 1024;

#[derive(Deserialize)]
struct HeartbeatResponse {
    required_catalog_snapshot_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NodeInfo {
    pub node_id: String,
    pub role: NodeRole,
    pub address: String,
    pub environment: String,
    pub version: String,
    pub uptime_secs: u64,
    pub last_heartbeat: u64,
    #[serde(default)]
    pub memory_rss_bytes: u64,
    /// Live heap bytes by the node's own count, and the limit it runs under.
    #[serde(default)]
    pub memory_allocated_bytes: u64,
    #[serde(default)]
    pub memory_limit_bytes: Option<u64>,
    #[serde(default)]
    pub catalog_snapshot_id: Option<String>,
    /// The coordinator's result cache counters; workers carry none.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub result_cache: Option<crate::result_cache::ResultCacheStats>,
    /// Memory admission on this node: statements on the coordinator, tasks
    /// on a worker. Workers report theirs with every heartbeat.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub admission: Option<kaveon_core::AdmissionStats>,
    /// The coordinator's resource groups: each group's limits and its
    /// running, queued, admitted, rejected counters and wait percentiles.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resource_groups: Option<Vec<kaveon_core::AdmissionGroupStats>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum NodeRole {
    Coordinator,
    Worker,
}

pub struct ClusterState {
    pub this_node: NodeInfo,
    pub workers: HashMap<String, NodeInfo>,
    pub required_catalog_snapshot_id: Option<String>,
    /// The process limit reported with every heartbeat, when there is one.
    pub process_memory_limit_bytes: Option<u64>,
    started_at: u64,
}

impl ClusterState {
    pub fn new(config: &ServerConfig) -> Self {
        let now = now_epoch();
        Self {
            this_node: NodeInfo {
                node_id: config.node_id.clone(),
                role: if config.coordinator {
                    NodeRole::Coordinator
                } else {
                    NodeRole::Worker
                },
                address: config.advertised_uri.clone().unwrap_or_else(|| {
                    format!(
                        "{}://127.0.0.1:{}",
                        if config.tls_cert_path.is_some() {
                            "https"
                        } else {
                            "http"
                        },
                        config.http_port
                    )
                }),
                environment: config.environment.clone(),
                version: env!("CARGO_PKG_VERSION").to_owned(),
                uptime_secs: 0,
                last_heartbeat: now,
                memory_rss_bytes: process_memory_rss_bytes(),
                memory_allocated_bytes: kaveon_core::process_memory::allocated_bytes(),
                memory_limit_bytes: config.process_memory_limit_bytes,
                catalog_snapshot_id: None,
                result_cache: None,
                admission: None,
                resource_groups: None,
            },
            workers: HashMap::new(),
            process_memory_limit_bytes: config.process_memory_limit_bytes,
            required_catalog_snapshot_id: if config.coordinator {
                Some(String::new())
            } else {
                None
            },
            started_at: now,
        }
    }

    pub fn update_uptime(&mut self) {
        let now = now_epoch();
        self.this_node.uptime_secs = now.saturating_sub(self.started_at);
        self.this_node.last_heartbeat = now;
        self.this_node.memory_rss_bytes = process_memory_rss_bytes();
        self.this_node.memory_allocated_bytes = kaveon_core::process_memory::allocated_bytes();
        self.this_node.memory_limit_bytes = self.process_memory_limit_bytes;
    }

    pub fn register_worker(&mut self, info: NodeInfo) {
        self.workers.insert(info.node_id.clone(), info);
    }

    pub fn compatible_workers(&self, required_snapshot_id: &str) -> Vec<NodeInfo> {
        self.workers
            .values()
            .filter(|worker| worker.catalog_snapshot_id.as_deref() == Some(required_snapshot_id))
            .cloned()
            .collect()
    }

    pub fn remove_stale_workers(&mut self) {
        let cutoff = now_epoch().saturating_sub(NODE_EXPIRY.as_secs());
        self.workers.retain(|_, w| w.last_heartbeat >= cutoff);
    }

    /// Whether `node_id` has missed `WORKER_LOSS_MISSED_HEARTBEATS`
    /// heartbeats — or is no longer registered at all. The loss rule a
    /// running query applies is: a connection to the worker refused or
    /// reset, or a lapsed heartbeat together with a probe that does not
    /// answer; a worker's own task error is never a loss.
    pub fn heartbeat_lapsed(&self, node_id: &str) -> bool {
        let Some(worker) = self.workers.get(node_id) else {
            return true;
        };
        let lapse = HEARTBEAT_INTERVAL.as_secs() * u64::from(WORKER_LOSS_MISSED_HEARTBEATS);
        worker.last_heartbeat.saturating_add(lapse) < now_epoch()
    }

    pub fn active_worker_count(&self) -> usize {
        self.workers.len()
    }

    pub fn all_nodes(&mut self) -> Vec<NodeInfo> {
        self.update_uptime();
        self.remove_stale_workers();
        let mut nodes = vec![self.this_node.clone()];
        nodes.extend(self.workers.values().cloned());
        nodes
    }
}

fn process_memory_rss_bytes() -> u64 {
    std::fs::read_to_string("/proc/self/status")
        .ok()
        .and_then(|status| {
            status.lines().find_map(|line| {
                line.strip_prefix("VmRSS:")?
                    .split_whitespace()
                    .next()?
                    .parse::<u64>()
                    .ok()
            })
        })
        .unwrap_or_default()
        .saturating_mul(KIBIBYTE_BYTES)
}

pub async fn worker_heartbeat_loop(state: Arc<AppState>) {
    let client = reqwest::Client::new();
    loop {
        let info = {
            let snapshot_id = state.catalog.read().await.snapshot_id.clone();
            let mut cluster = state.cluster.write().await;
            cluster.update_uptime();
            cluster.this_node.catalog_snapshot_id = Some(snapshot_id);
            cluster.this_node.admission = Some(state.memory_admission.stats());
            cluster.this_node.clone()
        };

        let url = format!("{}/v1/node/heartbeat", state.config.discovery_uri);
        let mut request = client.post(&url).json(&info);
        if let Some(token) = &state.config.exchange_token {
            request = request.bearer_auth(token);
        }
        match request.send().await {
            Ok(resp) if resp.status().is_success() => {
                match resp.json::<HeartbeatResponse>().await {
                    Ok(reply)
                        if info.catalog_snapshot_id.as_deref()
                            != Some(reply.required_catalog_snapshot_id.as_str()) =>
                    {
                        state.cluster.write().await.required_catalog_snapshot_id =
                            Some(reply.required_catalog_snapshot_id.clone());
                        if let Err(error) = synchronize_catalog(
                            &client,
                            &state,
                            &reply.required_catalog_snapshot_id,
                        )
                        .await
                        {
                            eprintln!("catalog synchronization failed: {error}");
                        }
                    }
                    Ok(reply) => {
                        state.cluster.write().await.required_catalog_snapshot_id =
                            Some(reply.required_catalog_snapshot_id);
                    }
                    Err(error) => eprintln!("heartbeat response was invalid: {error}"),
                }
            }
            Ok(resp) => {
                eprintln!("heartbeat failed: coordinator returned {}", resp.status());
            }
            Err(e) => {
                eprintln!("heartbeat failed: {e}");
            }
        }

        tokio::time::sleep(HEARTBEAT_INTERVAL).await;
    }
}

async fn synchronize_catalog(
    client: &reqwest::Client,
    state: &AppState,
    required_identity: &str,
) -> Result<(), String> {
    let token = state
        .config
        .exchange_token
        .as_deref()
        .ok_or_else(|| "exchange credential is unavailable".to_owned())?;
    let url = format!(
        "{}/v1/internal/catalog/snapshot",
        state.config.discovery_uri.trim_end_matches('/')
    );
    let response = client
        .get(url)
        .bearer_auth(token)
        .send()
        .await
        .map_err(|error| format!("snapshot request failed: {error}"))?;
    if !response.status().is_success() {
        return Err(format!("coordinator returned {}", response.status()));
    }
    if response
        .content_length()
        .is_some_and(|length| length > MAX_CATALOG_REPLICA_BYTES as u64)
    {
        return Err("snapshot exceeds 16 MiB".into());
    }
    let bytes = response
        .bytes()
        .await
        .map_err(|error| format!("snapshot receive failed: {error}"))?;
    if bytes.len() > MAX_CATALOG_REPLICA_BYTES {
        return Err("snapshot exceeds 16 MiB".into());
    }
    let snapshot: kaveon_catalog::CatalogReplicaSnapshot = serde_json::from_slice(&bytes)
        .map_err(|error| format!("snapshot JSON is invalid: {error}"))?;
    if snapshot.identity != required_identity {
        return Err("heartbeat and snapshot identities differ".into());
    }
    state
        .catalog_store
        .install_replica_snapshot(&snapshot)
        .map_err(|error| error.to_string())?;
    crate::api::refresh_catalog_snapshot(state)
        .await
        .map_err(|_| "cannot publish installed catalog snapshot".to_owned())?;
    Ok(())
}

fn now_epoch() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}
