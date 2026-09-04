mod api;
mod cluster;
mod config;
pub mod exchange;
pub mod planner;
mod scheduler;
mod ui;

use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::RwLock;

use cluster::ClusterState;
use config::ServerConfig;

pub struct AppState {
    pub config: ServerConfig,
    pub cluster: RwLock<ClusterState>,
    pub catalog: RwLock<kaveon_core::CatalogManager>,
}

#[tokio::main]
async fn main() {
    let config_path = std::env::args()
        .nth(1)
        .map(PathBuf::from)
        .unwrap_or_else(config::default_config_path);

    let config = match config::load_server_config(&config_path) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("failed to load config from {}: {e}", config_path.display());
            eprintln!("usage: kaveon-server [config.toml]");
            std::process::exit(1);
        }
    };

    let addr: SocketAddr = format!("0.0.0.0:{}", config.http_port).parse().unwrap();
    let cluster = ClusterState::new(&config);
    let catalog = config::build_catalog_manager(&config);

    println!("Kaveon Engine v{}", env!("CARGO_PKG_VERSION"));
    println!("Node:        {}", config.node_id);
    println!(
        "Role:        {}",
        if config.coordinator {
            "coordinator"
        } else {
            "worker"
        }
    );
    println!("Environment: {}", config.environment);
    println!("Listening:   http://{addr}");
    if !config.coordinator {
        println!("Coordinator: {}", config.discovery_uri);
    }
    println!();

    let state = Arc::new(AppState {
        config,
        cluster: RwLock::new(cluster),
        catalog: RwLock::new(catalog),
    });

    if !state.config.coordinator {
        let s = Arc::clone(&state);
        tokio::spawn(async move {
            cluster::worker_heartbeat_loop(s).await;
        });
    }

    let app = api::build_router(Arc::clone(&state));

    let listener = tokio::net::TcpListener::bind(addr).await.unwrap();
    axum::serve(listener, app).await.unwrap();
}
