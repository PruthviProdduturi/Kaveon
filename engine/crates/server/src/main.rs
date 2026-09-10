mod api;
pub mod cluster;
mod config;
pub mod disk_exchange;
pub mod entra;
pub mod exchange;
pub mod fragment_exec;
pub mod lifecycle;
pub mod orchestrator;
pub mod planner;
pub mod results;
pub mod runtime;
pub mod scheduler;
pub mod security;
mod transaction_api;
pub mod transport;
mod ui;

use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::RwLock;

use cluster::ClusterState;
use config::ServerConfig;

pub struct PublishedCatalog {
    pub manager: kaveon_core::CatalogManager,
    pub snapshot_id: String,
}

impl std::ops::Deref for PublishedCatalog {
    type Target = kaveon_core::CatalogManager;

    fn deref(&self) -> &Self::Target {
        &self.manager
    }
}

pub struct AppState {
    pub disk_exchange_store: Option<disk_exchange::DiskExchangeStore>,
    pub results: results::ResultStore,
    pub principal_admission: security::PrincipalAdmission,
    pub config: ServerConfig,
    pub cluster: RwLock<ClusterState>,
    /// Published catalog view. Queries clone the `Arc` once and retain that
    /// immutable manager while a newer view may be published here.
    pub catalog: RwLock<Arc<PublishedCatalog>>,
    pub catalog_store: kaveon_catalog::CatalogStore,
    pub exchange_store: exchange::ExchangeStore,
    pub lifecycle: lifecycle::WorkerLifecycle<transport::CachedTaskResult>,
    pub memory_admission: kaveon_core::MemoryAdmissionController,
    pub product_transactions: transaction_api::TransactionRegistry,
}

#[tokio::main]
async fn main() {
    let _ = rustls::crypto::ring::default_provider().install_default();
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

    let addr = SocketAddr::new(
        config.bind_host.parse().expect("validated bind address"),
        config.http_port,
    );
    let cluster = ClusterState::new(&config);
    let (catalog_store, catalog) = match config::open_catalog(&config) {
        Ok(catalog) => catalog,
        Err(error) => {
            eprintln!(
                "failed to open durable catalog {}: {error}",
                config.catalog_database_path.display()
            );
            std::process::exit(1);
        }
    };
    let catalog_snapshot_id = catalog_store
        .snapshot_identity()
        .expect("opened catalog has a durable snapshot identity");
    let product_catalog = match config::product_catalog_commit(&config) {
        Ok(value) => value,
        Err(error) => {
            eprintln!("failed to configure product transaction store: {error}");
            std::process::exit(1);
        }
    };
    if let Some(product_catalog) = &product_catalog {
        match product_catalog.read_current().await {
            Ok(_) => {}
            Err(kaveon_storage::CommitErrorKind::Missing) => {
                let genesis =
                    kaveon_catalog::product_manifest::CatalogSnapshot::empty("snapshot-genesis")
                        .expect("static genesis snapshot is valid");
                if !matches!(
                    product_catalog.initialize(genesis).await,
                    kaveon_catalog::product_commit::CommitOutcome::Committed(_)
                        | kaveon_catalog::product_commit::CommitOutcome::Replayed(_)
                ) {
                    eprintln!("failed to initialize product transaction store");
                    std::process::exit(1);
                }
            }
            Err(error) => {
                eprintln!("failed to open product transaction store: {error:?}");
                std::process::exit(1);
            }
        }
    }

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
    let scheme = if config.tls_cert_path.is_some() {
        "https"
    } else {
        "http"
    };
    println!("Listening:   {scheme}://{addr}");
    println!("Catalog DB:  {}", config.catalog_database_path.display());
    if !config.coordinator {
        println!("Coordinator: {}", config.discovery_uri);
    }
    println!();

    let memory_admission =
        kaveon_core::MemoryAdmissionController::new(config.memory_admission_limit_bytes)
            .expect("validated memory admission configuration");
    let disk_exchange_store = if config.coordinator && config.coordinator_exchange_spool {
        Some(
            disk_exchange::DiskExchangeStore::new(
                &config.exchange_spool_root,
                config.exchange_disk_limit_bytes,
            )
            .expect("exchange spool can be initialized"),
        )
    } else {
        None
    };
    let state = Arc::new(AppState {
        disk_exchange_store,
        results: results::ResultStore::default(),
        principal_admission: security::PrincipalAdmission::default(),
        config,
        cluster: RwLock::new(cluster),
        catalog: RwLock::new(Arc::new(PublishedCatalog {
            manager: catalog,
            snapshot_id: catalog_snapshot_id,
        })),
        catalog_store,
        exchange_store: exchange::ExchangeStore::default(),
        lifecycle: lifecycle::WorkerLifecycle::default(),
        memory_admission,
        // Product transactions fail closed until an ADLS-backed product store
        // is supplied by deployment configuration.
        product_transactions: product_catalog
            .map(transaction_api::TransactionRegistry::enabled)
            .unwrap_or_else(transaction_api::TransactionRegistry::disabled),
    });

    if !state.config.coordinator {
        let s = Arc::clone(&state);
        tokio::spawn(async move {
            cluster::worker_heartbeat_loop(s).await;
        });
    }

    let cleanup_state = state.clone();
    tokio::spawn(async move {
        loop {
            tokio::time::sleep(std::time::Duration::from_secs(30)).await;
            cleanup_state.results.cleanup();
            if let Some(store) = &cleanup_state.disk_exchange_store {
                store.cleanup();
            }
        }
    });
    let app = api::build_router(Arc::clone(&state));

    if let (Some(cert), Some(key)) = (&state.config.tls_cert_path, &state.config.tls_key_path) {
        let tls = axum_server::tls_rustls::RustlsConfig::from_pem_file(cert, key)
            .await
            .expect("valid TLS certificate and private key");
        axum_server::bind_rustls(addr, tls)
            .serve(app.into_make_service())
            .await
            .unwrap();
    } else {
        let listener = tokio::net::TcpListener::bind(addr).await.unwrap();
        axum::serve(listener, app).await.unwrap();
    }
}
