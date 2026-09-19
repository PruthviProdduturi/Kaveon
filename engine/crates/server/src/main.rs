mod api;
pub mod audit;
mod catalog_ddl;
pub mod cluster;
mod config;
#[cfg(test)]
mod differential_tests;
pub mod disk_exchange;
pub mod entra;
pub mod exchange;
pub mod fragment_exec;
pub mod lifecycle;
mod optimize;
pub mod orchestrator;
pub mod planner;
pub mod resource_groups;
pub mod result_cache;
pub mod results;
pub mod runtime;
pub mod scheduler;
pub mod security;
pub mod settings;
#[cfg(test)]
mod tpch_coverage;
mod transaction_api;
pub mod transport;
mod ui;

use std::net::SocketAddr;
use std::path::PathBuf;

/// Every allocation the node makes is counted, so the memory guard
/// answers to live bytes, not to estimates.
#[global_allocator]
static ALLOCATOR: kaveon_core::CountingAllocator = kaveon_core::CountingAllocator;
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
    /// Complete results of finished statements; only a coordinator keeps any.
    pub result_cache: result_cache::ResultCache,
    /// The resource groups in force and where they came from.
    pub governance: resource_groups::Governor,
    /// The audit ledger; disabled on workers.
    pub audit: audit::AuditLedger,
    pub config: ServerConfig,
    pub cluster: RwLock<ClusterState>,
    /// Published catalog view. Queries clone the `Arc` once and retain that
    /// immutable manager while a newer view may be published here.
    pub catalog: RwLock<Arc<PublishedCatalog>>,
    pub catalog_store: kaveon_catalog::CatalogStore,
    pub exchange_store: exchange::ExchangeStore,
    /// Shared transport for worker and exchange traffic. Reusing it preserves
    /// DNS resolution and idle connections across tasks in the same process.
    pub internal_http_client: reqwest::Client,
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

    let mut memory_admission =
        kaveon_core::MemoryAdmissionController::new(config.memory_admission_limit_bytes)
            .expect("validated memory admission configuration")
            .with_queue_limit(config.memory_admission_queue);
    if let Some(process) = config.process_memory() {
        println!(
            "Memory:      process limit {} MiB, {} MiB kept free; admission {} MiB; per query {} MiB",
            process.limit_bytes() >> 20,
            process.headroom_bytes() >> 20,
            config.memory_admission_limit_bytes >> 20,
            config.query_memory_limit_bytes >> 20,
        );
        memory_admission = memory_admission.with_process_memory(process);
    } else {
        println!(
            "Memory:      no process limit; admission {} MiB; per query {} MiB",
            config.memory_admission_limit_bytes >> 20,
            config.query_memory_limit_bytes >> 20,
        );
    }
    if config.memory_admission_queue == 0 {
        println!("Admission:   no queue; what does not fit on arrival is refused");
    } else if config.coordinator {
        println!(
            "Admission:   queue of {} statements, {} s wait",
            config.memory_admission_queue, config.memory_admission_wait_seconds
        );
    } else {
        println!(
            "Admission:   queue of {} tasks",
            config.memory_admission_queue
        );
    }
    let spools_exchanges = if config.coordinator {
        config.coordinator_exchange_spool
    } else {
        config.worker_exchange_spool
    };
    let disk_exchange_store = if spools_exchanges {
        Some(
            disk_exchange::DiskExchangeStore::with_query_limit(
                &config.exchange_spool_root,
                config.exchange_disk_limit_bytes,
                config.exchange_query_disk_limit_bytes,
            )
            .expect("exchange spool can be initialized"),
        )
    } else {
        None
    };
    let result_cache = result_cache::ResultCache::new(
        if config.coordinator {
            config.result_cache_bytes
        } else {
            0
        },
        std::time::Duration::from_secs(config.result_cache_ttl_seconds),
    );
    let governance = match resource_groups::load(&config, config.resource_groups_section.clone()) {
        Ok((groups, source, store_path)) => {
            if config.coordinator {
                if let Err(error) = memory_admission
                    .set_groups(groups.policies(config.memory_admission_limit_bytes))
                {
                    eprintln!("failed to apply resource groups: {error}");
                    std::process::exit(1);
                }
                println!(
                    "Governance:  {} resource group(s), {} selector(s), from {:?}",
                    groups.groups.len(),
                    groups.selectors.len(),
                    source
                );
            }
            resource_groups::Governor::new(groups, source, store_path)
        }
        Err(error) => {
            eprintln!("failed to load resource groups: {error}");
            std::process::exit(1);
        }
    };
    let audit = if config.coordinator && config.audit_retention_days > 0 {
        match audit::AuditLedger::open(
            &config.audit_dir,
            config.audit_segment_bytes,
            std::time::Duration::from_secs(config.audit_retention_days * 86_400),
        ) {
            Ok(ledger) => {
                ledger.skip_catalog_history(&catalog_store);
                println!(
                    "Audit:       {} ({} day retention)",
                    config.audit_dir.display(),
                    config.audit_retention_days
                );
                ledger
            }
            Err(error) => {
                eprintln!(
                    "failed to open audit ledger {}: {error}",
                    config.audit_dir.display()
                );
                std::process::exit(1);
            }
        }
    } else {
        audit::AuditLedger::disabled()
    };
    let state = Arc::new(AppState {
        disk_exchange_store,
        results: results::ResultStore::with_limits(
            config.result_query_disk_limit_bytes,
            config.result_disk_limit_bytes,
        ),
        result_cache,
        governance,
        audit,
        config,
        cluster: RwLock::new(cluster),
        catalog: RwLock::new(Arc::new(PublishedCatalog {
            manager: catalog,
            snapshot_id: catalog_snapshot_id,
        })),
        catalog_store,
        exchange_store: exchange::ExchangeStore::default(),
        internal_http_client: reqwest::Client::new(),
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
            if cleanup_state.audit.is_enabled() {
                let ledger = cleanup_state.audit.clone();
                let _ = tokio::task::spawn_blocking(move || ledger.enforce_retention()).await;
            }
        }
    });
    let app = api::build_router(Arc::clone(&state));

    // A clean stop: the listener closes, in-flight requests finish, and
    // the audit ledger writes what is enqueued before the process ends.
    if let (Some(cert), Some(key)) = (&state.config.tls_cert_path, &state.config.tls_key_path) {
        let tls = axum_server::tls_rustls::RustlsConfig::from_pem_file(cert, key)
            .await
            .expect("valid TLS certificate and private key");
        let handle = axum_server::Handle::new();
        let stopper = handle.clone();
        tokio::spawn(async move {
            shutdown_signal().await;
            stopper.graceful_shutdown(Some(std::time::Duration::from_secs(30)));
        });
        axum_server::bind_rustls(addr, tls)
            .handle(handle)
            .serve(app.into_make_service())
            .await
            .unwrap();
    } else {
        let listener = tokio::net::TcpListener::bind(addr).await.unwrap();
        axum::serve(listener, app)
            .with_graceful_shutdown(shutdown_signal())
            .await
            .unwrap();
    }
    let ledger = state.audit.clone();
    let _ = tokio::task::spawn_blocking(move || ledger.shutdown()).await;
}

/// Resolves on SIGTERM (the container runtime's stop) or Ctrl-C.
async fn shutdown_signal() {
    #[cfg(unix)]
    {
        let mut terminate =
            tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
                .expect("SIGTERM handler");
        tokio::select! {
            _ = tokio::signal::ctrl_c() => {}
            _ = terminate.recv() => {}
        }
    }
    #[cfg(not(unix))]
    {
        let _ = tokio::signal::ctrl_c().await;
    }
}
