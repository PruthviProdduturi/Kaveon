use crate::AppState;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use crate::config::ServerConfig;

const HEARTBEAT_INTERVAL: Duration = Duration::from_secs(10);
const NODE_EXPIRY: Duration = Duration::from_secs(30);
const KIBIBYTE_BYTES: u64 = 1024;

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
                address: config
                    .advertised_uri
                    .clone()
                    .unwrap_or_else(|| format!("http://127.0.0.1:{}", config.http_port)),
                environment: config.environment.clone(),
                version: env!("CARGO_PKG_VERSION").to_owned(),
                uptime_secs: 0,
                last_heartbeat: now,
                memory_rss_bytes: process_memory_rss_bytes(),
            },
            workers: HashMap::new(),
            started_at: now,
        }
    }

    pub fn update_uptime(&mut self) {
        let now = now_epoch();
        self.this_node.uptime_secs = now.saturating_sub(self.started_at);
        self.this_node.last_heartbeat = now;
        self.this_node.memory_rss_bytes = process_memory_rss_bytes();
    }

    pub fn register_worker(&mut self, info: NodeInfo) {
        self.workers.insert(info.node_id.clone(), info);
    }

    pub fn remove_stale_workers(&mut self) {
        let cutoff = now_epoch().saturating_sub(NODE_EXPIRY.as_secs());
        self.workers.retain(|_, w| w.last_heartbeat >= cutoff);
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
            let mut cluster = state.cluster.write().await;
            cluster.update_uptime();
            cluster.this_node.clone()
        };

        let url = format!("{}/v1/node/heartbeat", state.config.discovery_uri);
        match client.post(&url).json(&info).send().await {
            Ok(resp) if resp.status().is_success() => {}
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

fn now_epoch() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}
