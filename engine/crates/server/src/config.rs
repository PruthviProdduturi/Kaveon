use kaveon_catalog::CatalogStore;
use kaveon_core::{
    AccessPattern, CatalogAdapter, CatalogDefinition, CatalogId, CatalogLifecycle, CatalogManager,
    CatalogProvider, ColumnDefinition, DataFormat, MemoryCatalog, SchemaDefinition, SchemaId,
    StorageType, TableDefinition, TableId, TableMeta,
};
use kaveon_storage::{DeltaTableReader, ParquetReader};
use serde::Deserialize;
use std::path::{Path, PathBuf};
use std::sync::Arc;

const DEFAULT_QUERY_MEMORY_LIMIT_BYTES: u64 = 512 * 1_024 * 1_024;
const DEFAULT_MEMORY_ADMISSION_LIMIT_BYTES: u64 = 4 * 1_024 * 1_024 * 1_024;
const DEFAULT_MEMORY_ADMISSION_QUEUE: usize = 64;
const DEFAULT_MEMORY_ADMISSION_WAIT_SECONDS: u64 = 60;
const DEFAULT_RESULT_CACHE_BYTES: u64 = 256 * 1_024 * 1_024;
const DEFAULT_RESULT_CACHE_TTL_SECONDS: u64 = 600;

#[derive(Debug, Clone)]
pub struct ServerConfig {
    pub security: crate::security::SecurityConfig,
    pub bind_host: String,
    pub tls_cert_path: Option<PathBuf>,
    pub tls_key_path: Option<PathBuf>,
    /// The `default` resource group's `max_concurrent`
    /// (`KAVEON_PRINCIPAL_QUERY_LIMIT`, kept as the alias).
    pub principal_query_limit: usize,
    /// Where the coordinator keeps what must survive a restart and is not
    /// the catalog store: the runtime resource-group copy and the audit
    /// ledger. Defaults to the catalog store's directory.
    pub state_dir: PathBuf,
    /// The JSON or TOML file named by `KAVEON_RESOURCE_GROUPS`.
    pub resource_groups_path: Option<PathBuf>,
    /// The `[resource_groups]` section of the configuration file.
    pub resource_groups_section: Option<crate::resource_groups::ResourceGroups>,
    /// The audit ledger's directory; defaults to `<state dir>/audit`.
    pub audit_dir: PathBuf,
    /// How long ledger segments are kept; zero turns the ledger off.
    pub audit_retention_days: u64,
    /// The size at which a ledger segment is rotated.
    pub audit_segment_bytes: u64,
    pub coordinator_exchange_spool: bool,
    /// Workers keep the exchange partitions addressed to them on their own
    /// disk, so producers upload straight to the consuming worker and the
    /// coordinator carries no exchange traffic. Off, the coordinator's
    /// spool (when on) is the hub, else workers hold payloads in memory.
    pub worker_exchange_spool: bool,
    pub exchange_spool_root: PathBuf,
    pub exchange_disk_limit_bytes: u64,
    /// One query's share of the exchange spool.
    pub exchange_query_disk_limit_bytes: u64,
    pub node_id: String,
    pub environment: String,
    pub coordinator: bool,
    pub http_port: u16,
    pub discovery_uri: String,
    pub advertised_uri: Option<String>,
    pub data_dir: Option<PathBuf>,
    pub catalog_dir: Option<PathBuf>,
    pub catalog_database_path: PathBuf,
    pub catalog_admin_token: Option<String>,
    pub exchange_token: Option<String>,
    pub query_memory_limit_bytes: u64,
    pub memory_admission_limit_bytes: u64,
    /// How many arrivals may wait for admission at once; zero refuses
    /// whatever does not fit on arrival.
    pub memory_admission_queue: usize,
    /// How long a statement waits for admission on the coordinator before
    /// it is refused; the ceiling of the per-request setting.
    pub memory_admission_wait_seconds: u64,
    /// How many times one stage's finished output may be produced again
    /// for one query after worker losses (`KAVEON_STAGE_RETRY_LIMIT`).
    pub stage_retry_limit: u32,
    /// The process's memory limit: the container's cgroup limit unless
    /// `KAVEON_PROCESS_MEMORY_LIMIT_BYTES` says otherwise; None when the
    /// process is not limited.
    pub process_memory_limit_bytes: Option<u64>,
    /// The coordinator's result cache budget; zero disables it.
    pub result_cache_bytes: u64,
    /// One paged result's disk, and every paged result's together.
    pub result_query_disk_limit_bytes: u64,
    pub result_disk_limit_bytes: u64,
    /// How long a cached result may be served.
    pub result_cache_ttl_seconds: u64,
    /// Whether the coordinator refreshes a table's statistics on its own
    /// when planning observes a source version newer than the one on
    /// record: added files folded in, a removal recomputed at the
    /// document's depth. Off, statistics change only through `ANALYZE`.
    pub statistics_auto_refresh: bool,
    /// The most cells a table's cube may hold (`KAVEON_CUBE_MAX_CELLS`): a
    /// declared shape whose planned cells exceed it is refused, and a
    /// build or refresh that exceeds it fails. The encoded document is
    /// bounded with it (see `kaveon_core::cube`).
    pub cube_max_cells: u64,
    pub product_transactions: ProductTransactionsConfig,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProductTransactionsConfig {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub account: String,
    #[serde(default)]
    pub container: String,
    #[serde(default = "default_product_prefix")]
    pub prefix: String,
    #[serde(default = "default_product_storage_mode")]
    pub storage_mode: String,
    #[serde(default = "default_product_local_path")]
    pub local_path: PathBuf,
}

fn default_product_prefix() -> String {
    "kaveon/product-catalog".to_owned()
}

fn default_product_storage_mode() -> String {
    "adls".to_owned()
}

fn default_product_local_path() -> PathBuf {
    PathBuf::from("/var/lib/kaveon/product-transactions")
}

impl Default for ProductTransactionsConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            account: String::new(),
            container: String::new(),
            prefix: default_product_prefix(),
            storage_mode: default_product_storage_mode(),
            local_path: default_product_local_path(),
        }
    }
}

impl Default for ServerConfig {
    fn default() -> Self {
        Self {
            security: crate::security::SecurityConfig::default(),
            bind_host: "127.0.0.1".into(),
            tls_cert_path: None,
            tls_key_path: None,
            principal_query_limit: 4,
            state_dir: PathBuf::from("."),
            resource_groups_path: None,
            resource_groups_section: None,
            audit_dir: PathBuf::from("./audit"),
            audit_retention_days: crate::audit::DEFAULT_RETENTION_DAYS,
            audit_segment_bytes: crate::audit::DEFAULT_SEGMENT_BYTES,
            coordinator_exchange_spool: true,
            worker_exchange_spool: false,
            exchange_spool_root: std::env::temp_dir(),
            exchange_disk_limit_bytes: 10 * 1024 * 1024 * 1024,
            exchange_query_disk_limit_bytes: 8 * 1024 * 1024 * 1024,
            node_id: uuid::Uuid::new_v4().to_string(),
            environment: "production".into(),
            coordinator: true,
            http_port: 8080,
            discovery_uri: "http://localhost:8080".into(),
            advertised_uri: None,
            data_dir: None,
            catalog_dir: None,
            catalog_database_path: PathBuf::from("kaveon-catalog.db"),
            catalog_admin_token: None,
            exchange_token: None,
            query_memory_limit_bytes: DEFAULT_QUERY_MEMORY_LIMIT_BYTES,
            memory_admission_limit_bytes: DEFAULT_MEMORY_ADMISSION_LIMIT_BYTES,
            memory_admission_queue: DEFAULT_MEMORY_ADMISSION_QUEUE,
            memory_admission_wait_seconds: DEFAULT_MEMORY_ADMISSION_WAIT_SECONDS,
            stage_retry_limit: crate::orchestrator::DEFAULT_STAGE_RETRY_LIMIT,
            process_memory_limit_bytes: None,
            result_cache_bytes: DEFAULT_RESULT_CACHE_BYTES,
            result_query_disk_limit_bytes: crate::results::DEFAULT_QUERY_BYTES,
            result_disk_limit_bytes: crate::results::DEFAULT_PROCESS_BYTES,
            result_cache_ttl_seconds: DEFAULT_RESULT_CACHE_TTL_SECONDS,
            statistics_auto_refresh: true,
            cube_max_cells: kaveon_core::shape::DEFAULT_CUBE_MAX_CELLS,
            product_transactions: ProductTransactionsConfig::default(),
        }
    }
}

impl ServerConfig {
    /// The guard over the process's real memory, when it is limited.
    pub fn process_memory(&self) -> Option<kaveon_core::ProcessMemory> {
        self.process_memory_limit_bytes
            .map(kaveon_core::ProcessMemory::new)
    }
}

/// The admission limit a limited process defaults to: everything below the
/// guard's headroom. Set explicitly, the limit is taken as given and only
/// warned about when the process could never honour it.
fn default_admission_for_process(limit_bytes: u64) -> u64 {
    limit_bytes
        .saturating_sub(kaveon_core::process_memory::default_headroom_bytes(
            limit_bytes,
        ))
        .max(1)
}

#[derive(Deserialize)]
struct RawConfig {
    node: Option<NodeConfig>,
    http: Option<HttpConfig>,
    discovery: Option<DiscoveryConfig>,
    storage: Option<StorageConfig>,
    exchange: Option<ExchangeConfig>,
    memory: Option<MemoryConfig>,
    result_cache: Option<ResultCacheConfig>,
    catalog: Option<NativeCatalogConfig>,
    product_transactions: Option<ProductTransactionsConfig>,
    resource_groups: Option<crate::resource_groups::ResourceGroups>,
    audit: Option<AuditConfig>,
}

#[derive(Deserialize)]
struct AuditConfig {
    dir: Option<String>,
    retention_days: Option<u64>,
    segment_bytes: Option<u64>,
}

#[derive(Deserialize)]
struct NodeConfig {
    id: Option<String>,
    environment: Option<String>,
    coordinator: Option<bool>,
    state_dir: Option<String>,
}

#[derive(Deserialize)]
struct HttpConfig {
    port: Option<u16>,
}

#[derive(Deserialize)]
struct DiscoveryConfig {
    uri: Option<String>,
    advertised_uri: Option<String>,
}

#[derive(Deserialize)]
struct StorageConfig {
    data_dir: Option<String>,
    catalog_dir: Option<String>,
    catalog_database_path: Option<String>,
}

#[derive(Deserialize)]
struct ExchangeConfig {
    token: Option<String>,
}

#[derive(Deserialize)]
struct MemoryConfig {
    query_limit_bytes: Option<u64>,
    admission_limit_bytes: Option<u64>,
    admission_queue: Option<usize>,
    admission_wait_seconds: Option<u64>,
}

#[derive(Deserialize)]
struct ResultCacheConfig {
    bytes: Option<u64>,
    ttl_seconds: Option<u64>,
}

#[derive(Deserialize)]
struct NativeCatalogConfig {
    database_path: Option<String>,
    admin_token: Option<String>,
}

pub fn default_config_path() -> PathBuf {
    PathBuf::from("/etc/kaveon/config.toml")
}

fn config_env_fallback_enabled() -> bool {
    std::env::var("KAVEON_CONFIG_ENV_FALLBACK").as_deref() == Ok("true")
        && (std::env::var("KAVEON_INSECURE_DEVELOPMENT").as_deref() == Ok("true")
            || std::env::var("KAVEON_TLS_PROXY_BOUNDARY").as_deref() == Ok("true"))
}

fn parse_raw_config(content: &str, allow_env_fallback: bool) -> anyhow::Result<RawConfig> {
    let parsed: anyhow::Result<RawConfig> = if content.trim_start().starts_with('{') {
        serde_json::from_str(content).map_err(Into::into)
    } else {
        toml::from_str(content).map_err(Into::into)
    };
    match parsed {
        Ok(raw) => Ok(raw),
        Err(_error) if allow_env_fallback => Ok(RawConfig {
            node: None,
            http: None,
            discovery: None,
            storage: None,
            exchange: None,
            memory: None,
            result_cache: None,
            catalog: None,
            product_transactions: None,
            resource_groups: None,
            audit: None,
        }),
        Err(error) => Err(error),
    }
}

pub fn load_server_config(path: &Path) -> anyhow::Result<ServerConfig> {
    let mut config = ServerConfig::default();
    let mut config_sets_admission = false;
    let mut state_dir_from_config = None;
    let mut audit_dir_from_config = None;

    if path.exists() {
        let content = std::fs::read_to_string(path)?;
        // Older ACA revisions mounted the rendered configuration as JSON even
        // though the file was named `config.toml`. Accept that representation
        // only when the document is explicitly a JSON object; malformed JSON
        // still fails closed and TOML keeps its normal parser/validation path.
        let raw = parse_raw_config(&content, config_env_fallback_enabled())?;

        if let Some(node) = raw.node {
            if let Some(id) = node.id {
                config.node_id = id;
            }
            if let Some(env) = node.environment {
                config.environment = env;
            }
            if let Some(coord) = node.coordinator {
                config.coordinator = coord;
            }
            if let Some(dir) = node.state_dir {
                state_dir_from_config = Some(PathBuf::from(dir));
            }
        }
        if let Some(http) = raw.http
            && let Some(port) = http.port
        {
            config.http_port = port;
        }
        if let Some(disc) = raw.discovery {
            if let Some(uri) = disc.uri {
                config.discovery_uri = uri;
            }
            config.advertised_uri = disc.advertised_uri;
        }
        if let Some(storage) = raw.storage {
            if let Some(dir) = storage.data_dir {
                config.data_dir = Some(PathBuf::from(dir));
            }
            if let Some(dir) = storage.catalog_dir {
                config.catalog_dir = Some(PathBuf::from(dir));
            }
            if let Some(path) = storage.catalog_database_path {
                config.catalog_database_path = PathBuf::from(path);
            }
        }
        if let Some(exchange) = raw.exchange {
            config.exchange_token = exchange.token;
        }
        if let Some(memory) = raw.memory {
            if let Some(limit) = memory.query_limit_bytes {
                config.query_memory_limit_bytes = limit;
            }
            if let Some(limit) = memory.admission_limit_bytes {
                config.memory_admission_limit_bytes = limit;
                config_sets_admission = true;
            }
            if let Some(queue) = memory.admission_queue {
                config.memory_admission_queue = queue;
            }
            if let Some(seconds) = memory.admission_wait_seconds {
                config.memory_admission_wait_seconds = seconds;
            }
        }
        if let Some(cache) = raw.result_cache {
            if let Some(bytes) = cache.bytes {
                config.result_cache_bytes = bytes;
            }
            if let Some(seconds) = cache.ttl_seconds {
                config.result_cache_ttl_seconds = seconds;
            }
        }
        if let Some(catalog) = raw.catalog {
            if let Some(path) = catalog.database_path {
                config.catalog_database_path = PathBuf::from(path);
            }
            config.catalog_admin_token = catalog.admin_token;
        }
        if let Some(product_transactions) = raw.product_transactions {
            config.product_transactions = product_transactions;
        }
        config.resource_groups_section = raw.resource_groups;
        if let Some(audit) = raw.audit {
            audit_dir_from_config = audit.dir.map(PathBuf::from);
            if let Some(days) = audit.retention_days {
                config.audit_retention_days = days;
            }
            if let Some(bytes) = audit.segment_bytes {
                config.audit_segment_bytes = bytes;
            }
        }
    }

    if let Ok(v) = std::env::var("KAVEON_NODE_ID") {
        config.node_id = v;
    }
    if let Ok(v) = std::env::var("KAVEON_ENVIRONMENT") {
        config.environment = v;
    }
    if let Ok(v) = std::env::var("KAVEON_COORDINATOR") {
        config.coordinator = v == "true";
    }
    if let Ok(v) = std::env::var("KAVEON_HTTP_PORT")
        && let Ok(port) = v.parse()
    {
        config.http_port = port;
    }
    if let Ok(v) = std::env::var("KAVEON_DISCOVERY_URI") {
        config.discovery_uri = v;
    }
    if let Ok(v) = std::env::var("KAVEON_ADVERTISED_URI") {
        config.advertised_uri = Some(v);
    }
    if let Ok(v) = std::env::var("KAVEON_DATA_DIR") {
        config.data_dir = Some(PathBuf::from(v));
    }
    if let Ok(v) = std::env::var("KAVEON_CATALOG_DIR") {
        config.catalog_dir = Some(PathBuf::from(v));
    }
    if let Ok(v) = std::env::var("KAVEON_CATALOG_DATABASE_PATH") {
        config.catalog_database_path = PathBuf::from(v);
    }
    if let Ok(v) = std::env::var("KAVEON_CATALOG_ADMIN_TOKEN") {
        config.catalog_admin_token = Some(v);
    }
    if let Ok(v) = std::env::var("KAVEON_EXCHANGE_TOKEN") {
        config.exchange_token = Some(v);
    }
    if let Ok(value) = std::env::var("KAVEON_PRODUCT_TRANSACTIONS_ENABLED") {
        anyhow::ensure!(
            matches!(value.as_str(), "true" | "false"),
            "KAVEON_PRODUCT_TRANSACTIONS_ENABLED must be true or false"
        );
        config.product_transactions.enabled = value == "true";
    }
    if let Ok(value) = std::env::var("KAVEON_PRODUCT_ADLS_ACCOUNT") {
        config.product_transactions.account = value;
    }
    if let Ok(value) = std::env::var("KAVEON_PRODUCT_ADLS_CONTAINER") {
        config.product_transactions.container = value;
    }
    if let Ok(value) = std::env::var("KAVEON_PRODUCT_ADLS_PREFIX") {
        config.product_transactions.prefix = value;
    }
    if let Ok(value) = std::env::var("KAVEON_PRODUCT_STORAGE_MODE") {
        config.product_transactions.storage_mode = value;
    }
    if let Ok(value) = std::env::var("KAVEON_PRODUCT_LOCAL_PATH") {
        config.product_transactions.local_path = PathBuf::from(value);
    }
    // The process limit: the cgroup's, or an explicit override (0 disables).
    config.process_memory_limit_bytes = match std::env::var("KAVEON_PROCESS_MEMORY_LIMIT_BYTES") {
        Ok(v) => {
            let limit: u64 = v.parse().map_err(|_| {
                anyhow::anyhow!("KAVEON_PROCESS_MEMORY_LIMIT_BYTES must be an unsigned integer")
            })?;
            (limit != 0).then_some(limit)
        }
        Err(_) => kaveon_core::process_memory::cgroup_memory_limit_bytes(),
    };
    // A limited process that is not told its admission limit takes what
    // the guard leaves: the container's limit less the headroom.
    let admission_from_env = std::env::var("KAVEON_MEMORY_ADMISSION_LIMIT_BYTES").is_ok();
    if let Ok(v) = std::env::var("KAVEON_QUERY_MEMORY_LIMIT_BYTES") {
        config.query_memory_limit_bytes = v.parse().map_err(|_| {
            anyhow::anyhow!("KAVEON_QUERY_MEMORY_LIMIT_BYTES must be an unsigned integer")
        })?;
    }
    if let Ok(v) = std::env::var("KAVEON_MEMORY_ADMISSION_LIMIT_BYTES") {
        config.memory_admission_limit_bytes = v.parse().map_err(|_| {
            anyhow::anyhow!("KAVEON_MEMORY_ADMISSION_LIMIT_BYTES must be an unsigned integer")
        })?;
    }
    if let Some(limit) = config.process_memory_limit_bytes
        && !admission_from_env
        && !config_sets_admission
    {
        config.memory_admission_limit_bytes = default_admission_for_process(limit);
        config.query_memory_limit_bytes = config
            .query_memory_limit_bytes
            .min(config.memory_admission_limit_bytes);
    }
    if config.query_memory_limit_bytes == 0 {
        anyhow::bail!("query memory limit must be greater than zero");
    }
    if config.memory_admission_limit_bytes < config.query_memory_limit_bytes {
        anyhow::bail!("memory admission limit must be at least the per-query limit");
    }
    if let Some(limit) = config.process_memory_limit_bytes
        && config.memory_admission_limit_bytes > limit
    {
        anyhow::bail!(
            "memory admission limit {} exceeds the process memory limit {}",
            config.memory_admission_limit_bytes,
            limit
        );
    }
    if let Ok(value) = std::env::var("KAVEON_MEMORY_ADMISSION_QUEUE") {
        config.memory_admission_queue = value.parse().map_err(|_| {
            anyhow::anyhow!("KAVEON_MEMORY_ADMISSION_QUEUE must be an unsigned integer")
        })?;
    }
    if let Ok(value) = std::env::var("KAVEON_MEMORY_ADMISSION_WAIT_SECONDS") {
        config.memory_admission_wait_seconds = value.parse().map_err(|_| {
            anyhow::anyhow!("KAVEON_MEMORY_ADMISSION_WAIT_SECONDS must be an unsigned integer")
        })?;
    }
    anyhow::ensure!(
        config.memory_admission_queue == 0 || config.memory_admission_wait_seconds > 0,
        "a memory admission queue needs a positive KAVEON_MEMORY_ADMISSION_WAIT_SECONDS"
    );
    if let Ok(value) = std::env::var("KAVEON_STAGE_RETRY_LIMIT") {
        config.stage_retry_limit = value
            .parse()
            .map_err(|_| anyhow::anyhow!("KAVEON_STAGE_RETRY_LIMIT must be an unsigned integer"))?;
    }

    if let Ok(value) = std::env::var("KAVEON_SECURITY_JSON") {
        config.security = serde_json::from_str(&value)?;
    }
    if let Ok(value) = std::env::var("KAVEON_STUDIO_URL") {
        config.security.studio_url = Some(value);
    }
    if let Ok(value) = std::env::var("KAVEON_INSECURE_DEVELOPMENT") {
        anyhow::ensure!(
            matches!(value.as_str(), "true" | "false"),
            "KAVEON_INSECURE_DEVELOPMENT must be true or false"
        );
        config.security.insecure_development = value == "true";
    }
    if let Ok(value) = std::env::var("KAVEON_BIND_HOST") {
        config.bind_host = value;
    }
    if let Ok(value) = std::env::var("KAVEON_PRINCIPAL_QUERY_LIMIT") {
        config.principal_query_limit = value.parse()?;
    }
    anyhow::ensure!(
        config.principal_query_limit > 0,
        "principal query limit must be positive"
    );
    // The state directory: the variable, the key, else beside the
    // catalog store.
    config.state_dir = match std::env::var("KAVEON_STATE_DIR") {
        Ok(value) => PathBuf::from(value),
        Err(_) => state_dir_from_config.unwrap_or_else(|| {
            config
                .catalog_database_path
                .parent()
                .filter(|parent| !parent.as_os_str().is_empty())
                .map_or_else(|| PathBuf::from("."), Path::to_path_buf)
        }),
    };
    if let Ok(value) = std::env::var("KAVEON_RESOURCE_GROUPS") {
        anyhow::ensure!(
            !value.trim().is_empty(),
            "KAVEON_RESOURCE_GROUPS must name a JSON or TOML file"
        );
        config.resource_groups_path = Some(PathBuf::from(value));
    }
    anyhow::ensure!(
        config.resource_groups_section.is_none() || config.security.resource_groups.is_empty(),
        "resource groups are configured twice: the [resource_groups] section and security.resource_groups; keep one"
    );
    config.audit_dir = match std::env::var("KAVEON_AUDIT_DIR") {
        Ok(value) => PathBuf::from(value),
        Err(_) => audit_dir_from_config.unwrap_or_else(|| config.state_dir.join("audit")),
    };
    if let Ok(value) = std::env::var("KAVEON_AUDIT_RETENTION_DAYS") {
        config.audit_retention_days = value.parse().map_err(|_| {
            anyhow::anyhow!("KAVEON_AUDIT_RETENTION_DAYS must be an unsigned integer")
        })?;
    }
    if let Ok(value) = std::env::var("KAVEON_AUDIT_SEGMENT_BYTES") {
        config.audit_segment_bytes = value.parse().map_err(|_| {
            anyhow::anyhow!("KAVEON_AUDIT_SEGMENT_BYTES must be an unsigned integer")
        })?;
    }
    anyhow::ensure!(
        config.audit_segment_bytes >= 1024 * 1024,
        "KAVEON_AUDIT_SEGMENT_BYTES must be at least 1 MiB"
    );
    config.tls_cert_path = std::env::var("KAVEON_TLS_CERT_PATH")
        .ok()
        .map(PathBuf::from);
    config.tls_key_path = std::env::var("KAVEON_TLS_KEY_PATH").ok().map(PathBuf::from);
    anyhow::ensure!(
        config.tls_cert_path.is_some() == config.tls_key_path.is_some(),
        "TLS requires both KAVEON_TLS_CERT_PATH and KAVEON_TLS_KEY_PATH"
    );
    let mut credentials = std::collections::HashSet::new();
    for token in config
        .security
        .principals
        .iter()
        .map(|value| value.token.as_str())
        .chain(config.security.bridge_token.as_deref())
        .chain(config.catalog_admin_token.as_deref())
        .chain(config.exchange_token.as_deref())
    {
        anyhow::ensure!(
            !token.is_empty() && credentials.insert(token),
            "public, bridge, catalog and exchange tokens must be nonempty and distinct"
        );
    }
    let address: std::net::IpAddr = config.bind_host.parse()?;
    anyhow::ensure!(
        address.is_loopback()
            || config.tls_cert_path.is_some()
            || config.security.insecure_development
            || std::env::var("KAVEON_TLS_PROXY_BOUNDARY").as_deref() == Ok("true"),
        "external HTTP binding requires KAVEON_TLS_PROXY_BOUNDARY=true and a network-isolated TLS proxy, or explicit insecure development mode"
    );
    if let Ok(value) = std::env::var("KAVEON_COORDINATOR_EXCHANGE_SPOOL") {
        anyhow::ensure!(
            matches!(value.as_str(), "true" | "false"),
            "KAVEON_COORDINATOR_EXCHANGE_SPOOL must be true or false"
        );
        config.coordinator_exchange_spool = value == "true";
    }
    if let Ok(value) = std::env::var("KAVEON_WORKER_EXCHANGE_SPOOL") {
        anyhow::ensure!(
            matches!(value.as_str(), "true" | "false"),
            "KAVEON_WORKER_EXCHANGE_SPOOL must be true or false"
        );
        config.worker_exchange_spool = value == "true";
    }
    if let Ok(value) = std::env::var("KAVEON_EXCHANGE_SPOOL_ROOT") {
        config.exchange_spool_root = value.into();
    }
    if let Ok(value) = std::env::var("KAVEON_EXCHANGE_DISK_LIMIT_BYTES") {
        config.exchange_disk_limit_bytes = value.parse()?;
    }
    if let Ok(value) = std::env::var("KAVEON_EXCHANGE_QUERY_DISK_LIMIT_BYTES") {
        config.exchange_query_disk_limit_bytes = value.parse()?;
    }
    anyhow::ensure!(
        config.exchange_disk_limit_bytes > 0 && config.exchange_query_disk_limit_bytes > 0,
        "exchange disk limits must be positive"
    );
    if let Ok(value) = std::env::var("KAVEON_RESULT_QUERY_DISK_LIMIT_BYTES") {
        config.result_query_disk_limit_bytes = value.parse().map_err(|_| {
            anyhow::anyhow!("KAVEON_RESULT_QUERY_DISK_LIMIT_BYTES must be an unsigned integer")
        })?;
    }
    if let Ok(value) = std::env::var("KAVEON_RESULT_DISK_LIMIT_BYTES") {
        config.result_disk_limit_bytes = value.parse().map_err(|_| {
            anyhow::anyhow!("KAVEON_RESULT_DISK_LIMIT_BYTES must be an unsigned integer")
        })?;
    }
    anyhow::ensure!(
        config.result_query_disk_limit_bytes > 0
            && config.result_disk_limit_bytes >= config.result_query_disk_limit_bytes,
        "result disk limits must be positive and the process limit at least the query limit"
    );
    if let Ok(value) = std::env::var("KAVEON_RESULT_CACHE_BYTES") {
        config.result_cache_bytes = value.parse().map_err(|_| {
            anyhow::anyhow!("KAVEON_RESULT_CACHE_BYTES must be an unsigned integer")
        })?;
    }
    if let Ok(value) = std::env::var("KAVEON_STATISTICS_AUTO_REFRESH") {
        config.statistics_auto_refresh = value
            .parse()
            .map_err(|_| anyhow::anyhow!("KAVEON_STATISTICS_AUTO_REFRESH must be true or false"))?;
    }
    if let Ok(value) = std::env::var("KAVEON_CUBE_MAX_CELLS") {
        config.cube_max_cells = value
            .parse::<u64>()
            .ok()
            .filter(|cells| *cells > 0)
            .ok_or_else(|| anyhow::anyhow!("KAVEON_CUBE_MAX_CELLS must be a positive integer"))?;
    }
    if let Ok(value) = std::env::var("KAVEON_RESULT_CACHE_TTL_SECONDS") {
        config.result_cache_ttl_seconds = value.parse().map_err(|_| {
            anyhow::anyhow!("KAVEON_RESULT_CACHE_TTL_SECONDS must be an unsigned integer")
        })?;
    }
    anyhow::ensure!(
        config.result_cache_bytes == 0 || config.result_cache_ttl_seconds > 0,
        "an enabled result cache needs a positive KAVEON_RESULT_CACHE_TTL_SECONDS"
    );
    config.security.validate()?;
    validate_product_transactions(&config)?;
    Ok(config)
}

fn validate_product_transactions(config: &ServerConfig) -> anyhow::Result<()> {
    if config.product_transactions.enabled {
        anyhow::ensure!(
            config.coordinator,
            "product transactions may be enabled only on a coordinator"
        );
        anyhow::ensure!(
            matches!(
                config.product_transactions.storage_mode.as_str(),
                "adls" | "local"
            ),
            "product transaction storage mode must be 'adls' or 'local'"
        );
        if config.product_transactions.storage_mode == "adls" {
            anyhow::ensure!(
                !config.product_transactions.account.is_empty(),
                "ADLS product transactions require an account"
            );
            anyhow::ensure!(
                !config.product_transactions.container.is_empty(),
                "ADLS product transactions require a container"
            );
        } else {
            anyhow::ensure!(
                !config
                    .product_transactions
                    .local_path
                    .as_os_str()
                    .is_empty(),
                "local product transactions require a storage path"
            );
        }
        anyhow::ensure!(
            !config.product_transactions.prefix.is_empty(),
            "enabled product transactions require an ADLS prefix"
        );
    }
    Ok(())
}

pub fn product_catalog_commit(
    config: &ServerConfig,
) -> anyhow::Result<Option<kaveon_catalog::product_commit::ProductCatalogCommit>> {
    if !config.product_transactions.enabled {
        return Ok(None);
    }
    let storage = if config.product_transactions.storage_mode == "local" {
        kaveon_storage::local_file_commit(&config.product_transactions.local_path)
            .map_err(anyhow::Error::msg)?
    } else {
        kaveon_storage::workload_identity_adls_commit(
            &config.product_transactions.account,
            &config.product_transactions.container,
        )
        .map_err(anyhow::Error::msg)?
    };
    let commit = kaveon_catalog::product_commit::ProductCatalogCommit::new(
        storage,
        &config.product_transactions.prefix,
        Arc::new(kaveon_catalog::product_metrics::TransactionMetrics::default()),
    )
    .map_err(anyhow::Error::msg)?;
    Ok(Some(commit))
}

const BOOTSTRAP_ACTOR: &str = "engine-bootstrap";

pub fn open_catalog(config: &ServerConfig) -> anyhow::Result<(CatalogStore, CatalogManager)> {
    if let Some(parent) = config.catalog_database_path.parent()
        && !parent.as_os_str().is_empty()
    {
        std::fs::create_dir_all(parent)?;
    }
    let store = CatalogStore::open(&config.catalog_database_path)?;
    bootstrap_catalog(&store, config)?;
    let manager = catalog_manager_snapshot(&store)?;
    Ok((store, manager))
}

fn bootstrap_catalog(store: &CatalogStore, config: &ServerConfig) -> anyhow::Result<()> {
    let discovered = build_catalog_manager(config);
    // A product deployment with curated catalog definitions mounted in the
    // durable catalog store must not recreate the legacy catch-all `/data`
    // catalog on every restart.  That auto-discovery path is retained for a
    // first-run standalone Engine, but once local curated definitions exist it
    // is a duplicate (and used to reintroduce the lowercase `kaveon` catalog).
    let has_local_curated_catalogs = store
        .list_catalogs()?
        .iter()
        .any(|definition| definition.id().as_str().starts_with("local-"));
    for catalog_name in discovered.catalog_names() {
        if has_local_curated_catalogs && catalog_name == "kaveon" {
            continue;
        }
        let provider = discovered.catalog(&catalog_name).ok_or_else(|| {
            anyhow::anyhow!("catalog '{catalog_name}' disappeared during bootstrap")
        })?;
        let catalog_id = if let Some(existing) = store.catalog_by_name(&catalog_name)? {
            existing.id().clone()
        } else {
            let catalog_id = CatalogId::new(format!("catalog:{catalog_name}"))?;
            let catalog = CatalogDefinition::new(
                catalog_id.clone(),
                &catalog_name,
                CatalogAdapter::Native,
                provider.storage_type().clone(),
            )?
            .transition(CatalogLifecycle::Active)?;
            store.create_catalog(BOOTSTRAP_ACTOR, &catalog)?;
            catalog_id
        };

        for schema_name in provider.schema_names() {
            let existing_schemas = store.list_schemas(&catalog_id)?;
            let schema_id = if let Some(existing) = existing_schemas
                .iter()
                .find(|schema| schema.name() == schema_name)
            {
                existing.id().clone()
            } else {
                let schema_id = SchemaId::new(format!("schema:{catalog_name}:{schema_name}"))?;
                let schema =
                    SchemaDefinition::new(schema_id.clone(), catalog_id.clone(), &schema_name)?
                        .transition(CatalogLifecycle::Active)?;
                store.create_schema(BOOTSTRAP_ACTOR, &schema)?;
                schema_id
            };
            let discovered_tables = provider.table_names(&schema_name)?;
            // A table this bootstrap registered from the data directory on
            // an earlier start whose file is no longer there would answer
            // every query with a missing-file failure on the workers: it
            // is retired here. A table anyone created through the catalog
            // API or DDL is theirs and stays, whatever its location.
            if let Some(data_dir) = config
                .data_dir
                .as_deref()
                .filter(|_| catalog_name == "kaveon")
            {
                for stored in store.list_tables(&schema_id)? {
                    if discovered_tables.iter().any(|name| name == stored.name()) {
                        continue;
                    }
                    let created_by = store.creator("table", stored.id().as_str())?;
                    if created_by.as_deref() != Some(BOOTSTRAP_ACTOR) {
                        continue;
                    }
                    let path = data_dir.join(stored.location());
                    if path.exists() {
                        continue;
                    }
                    store.delete_table(BOOTSTRAP_ACTOR, stored.id(), stored.revision())?;
                    eprintln!(
                        "catalog: retired {catalog_name}.{schema_name}.{}: registered from the data directory, {} no longer exists",
                        stored.name(),
                        path.display()
                    );
                }
            }
            for table_name in discovered_tables {
                if store
                    .list_tables(&schema_id)?
                    .iter()
                    .any(|table| table.name() == table_name)
                {
                    continue;
                }
                let table = provider.table(&schema_name, &table_name)?.ok_or_else(|| {
                    anyhow::anyhow!(
                        "table '{catalog_name}.{schema_name}.{table_name}' disappeared during bootstrap"
                    )
                })?;
                let columns = table
                    .arrow_schema
                    .fields()
                    .iter()
                    .map(|field| {
                        ColumnDefinition::new(
                            field.name(),
                            field.data_type().clone(),
                            field.is_nullable(),
                        )
                    })
                    .collect::<kaveon_core::Result<Vec<_>>>()?;
                let definition = TableDefinition::new(
                    TableId::new(format!("table:{catalog_name}:{schema_name}:{table_name}"))?,
                    schema_id.clone(),
                    &table_name,
                    &table.location,
                    table.access,
                    table.format,
                    columns,
                )?
                .transition(CatalogLifecycle::Active)?;
                store.create_table(BOOTSTRAP_ACTOR, &definition)?;
            }
        }
    }
    Ok(())
}

pub fn catalog_manager_snapshot(store: &CatalogStore) -> anyhow::Result<CatalogManager> {
    let catalogs = store.list_catalogs()?;
    let default_catalog = catalogs
        .iter()
        .find(|catalog| catalog.name() == "kaveon")
        .or_else(|| catalogs.first())
        .map(|catalog| catalog.name().to_owned())
        .unwrap_or_else(|| "kaveon".to_owned());
    let mut manager = CatalogManager::new(&default_catalog, "default");
    for definition in catalogs {
        if definition.lifecycle() != CatalogLifecycle::Active {
            continue;
        }
        let mut catalog = MemoryCatalog::new(definition.name(), definition.storage().clone());
        for schema in store.list_schemas(definition.id())? {
            if schema.lifecycle() != CatalogLifecycle::Active {
                continue;
            }
            catalog = catalog.with_schema(schema.name());
            for table in store.list_tables(schema.id())? {
                if table.lifecycle() != CatalogLifecycle::Active {
                    continue;
                }
                let fields = table
                    .columns()
                    .iter()
                    .map(|column| {
                        Ok(arrow::datatypes::Field::new(
                            column.name(),
                            column.data_type().clone(),
                            column.nullable(),
                        ))
                    })
                    .collect::<anyhow::Result<Vec<_>>>()?;
                catalog.register_table(
                    schema.name(),
                    TableMeta {
                        name: table.name().to_owned(),
                        arrow_schema: Arc::new(arrow::datatypes::Schema::new(fields)),
                        location: table.location().to_owned(),
                        access: table.access(),
                        format: table.format(),
                    },
                )?;
            }
        }
        manager.register_catalog(Box::new(catalog));
    }
    Ok(manager)
}

pub fn build_catalog_manager(config: &ServerConfig) -> CatalogManager {
    let mut mgr = CatalogManager::new("kaveon", "default");

    if let Some(ref catalog_dir) = config.catalog_dir
        && catalog_dir.is_dir()
        && let Ok(entries) = std::fs::read_dir(catalog_dir)
    {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().is_some_and(|e| e == "toml") {
                let name = path
                    .file_stem()
                    .and_then(|s| s.to_str())
                    .unwrap_or("unknown")
                    .to_owned();
                if let Ok(content) = std::fs::read_to_string(&path)
                    && let Ok(cat) = build_catalog_from_toml(&name, &content)
                {
                    mgr.register_catalog(Box::new(cat));
                }
            }
        }
    }

    if let Some(ref data_dir) = config.data_dir
        && data_dir.is_dir()
    {
        let catalog = build_local_catalog("kaveon", data_dir);
        mgr.register_catalog(Box::new(catalog));
    }

    mgr
}

fn build_local_catalog(name: &str, dir: &Path) -> MemoryCatalog {
    let mut catalog = MemoryCatalog::new(
        name,
        StorageType::Local {
            base_path: dir.to_path_buf(),
        },
    )
    .with_schema("default");

    if let Ok(entries) = std::fs::read_dir(dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() && path.join("_delta_log").is_dir() {
                if let Some(table_name) = path.file_name().and_then(|s| s.to_str())
                    && let Ok(meta) = DeltaTableReader::new(&path).metadata()
                {
                    let _ = catalog.register_table(
                        "default",
                        TableMeta {
                            name: table_name.to_owned(),
                            arrow_schema: meta.schema,
                            location: path.file_name().unwrap().to_string_lossy().into_owned(),
                            access: AccessPattern::Shortcut,
                            format: DataFormat::Delta,
                        },
                    );
                }
            } else if path.is_dir() {
                // A directory of Parquet files without a Delta log is a
                // Parquet table; one that holds no data files is not.
                if let Some(table_name) = path.file_name().and_then(|s| s.to_str())
                    && let Ok(meta) = ParquetReader::new(&path).metadata()
                {
                    let _ = catalog.register_table(
                        "default",
                        TableMeta {
                            name: table_name.to_owned(),
                            arrow_schema: meta.schema,
                            location: table_name.to_owned(),
                            access: AccessPattern::Shortcut,
                            format: DataFormat::Parquet,
                        },
                    );
                }
            } else if path.extension().is_some_and(|e| e == "parquet")
                && let Some(table_name) = path.file_stem().and_then(|s| s.to_str())
                && let Ok(meta) = ParquetReader::new(&path).metadata()
            {
                let _ = catalog.register_table(
                    "default",
                    TableMeta {
                        name: table_name.to_owned(),
                        arrow_schema: meta.schema,
                        location: path.file_name().unwrap().to_string_lossy().into_owned(),
                        access: AccessPattern::Shortcut,
                        format: DataFormat::Parquet,
                    },
                );
            }
        }
    }

    catalog
}

fn build_catalog_from_toml(name: &str, content: &str) -> anyhow::Result<MemoryCatalog> {
    let mut storage_type = String::new();
    let mut base_path = None;
    let mut account = None;
    let mut container = None;
    let mut root_path = None;
    let mut bucket = None;
    let mut region = None;
    let mut prefix = None;

    #[derive(Default)]
    struct TblEntry {
        name: String,
        schema: String,
        location: String,
        access: String,
        format: String,
    }

    let mut tables: Vec<TblEntry> = Vec::new();
    let mut current: Option<TblEntry> = None;
    let mut in_table = false;

    for line in content.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        if trimmed == "[[table]]" {
            if let Some(t) = current.take() {
                tables.push(t);
            }
            current = Some(TblEntry::default());
            in_table = true;
            continue;
        }
        if let Some((key, val)) = parse_kv(trimmed) {
            if in_table {
                if let Some(t) = current.as_mut() {
                    match key {
                        "name" => t.name = val,
                        "schema" => t.schema = val,
                        "location" => t.location = val,
                        "access" => t.access = val,
                        "format" => t.format = val,
                        _ => {}
                    }
                }
            } else {
                match key {
                    "type" | "connector.name" => storage_type = val,
                    "base_path" => base_path = Some(val),
                    "account" => account = Some(val),
                    "container" => container = Some(val),
                    "root_path" => root_path = Some(val),
                    "bucket" => bucket = Some(val),
                    "region" => region = Some(val),
                    "prefix" => prefix = Some(val),
                    _ => {}
                }
            }
        }
    }
    if let Some(t) = current.take() {
        tables.push(t);
    }

    let storage = match storage_type.as_str() {
        "local" => StorageType::Local {
            base_path: PathBuf::from(base_path.unwrap_or_else(|| ".".into())),
        },
        "adls_gen2" => StorageType::AdlsGen2 {
            account: account.unwrap_or_default(),
            container: container.unwrap_or_default(),
            root_path: root_path.unwrap_or_default(),
        },
        "s3" => StorageType::S3 {
            bucket: bucket.unwrap_or_default(),
            region: region.unwrap_or_else(|| "us-east-1".into()),
            prefix: prefix.unwrap_or_default(),
        },
        other => anyhow::bail!("unknown storage type '{other}'"),
    };

    let mut catalog = MemoryCatalog::new(name, storage.clone()).with_schema("default");

    if tables.is_empty()
        && let StorageType::Local { ref base_path } = storage
    {
        let built = build_local_catalog(name, base_path);
        return Ok(built);
    }

    for t in &tables {
        let schema_name = if t.schema.is_empty() {
            "default"
        } else {
            &t.schema
        };
        catalog = catalog.with_schema(schema_name);
        let access = match t.access.as_str() {
            "optimized" => AccessPattern::Optimized,
            _ => AccessPattern::Shortcut,
        };
        let format = match t.format.as_str() {
            "delta" => DataFormat::Delta,
            "iceberg" => DataFormat::Iceberg,
            _ => DataFormat::Parquet,
        };
        let arrow_schema = if let StorageType::Local { ref base_path } = storage {
            let full_path = base_path.join(&t.location);
            match format {
                DataFormat::Delta => DeltaTableReader::new(&full_path)
                    .metadata()
                    .map(|m| m.schema),
                DataFormat::Parquet => ParquetReader::new(&full_path).metadata().map(|m| m.schema),
                DataFormat::Iceberg => {
                    kaveon_storage::IcebergReader::new(full_path.to_string_lossy().into_owned())
                        .snapshot()
                        .map(|m| m.schema)
                }
            }
            .unwrap_or_else(|_| Arc::new(arrow::datatypes::Schema::empty()))
        } else {
            Arc::new(arrow::datatypes::Schema::empty())
        };
        let _ = catalog.register_table(
            schema_name,
            TableMeta {
                name: t.name.clone(),
                arrow_schema,
                location: t.location.clone(),
                access,
                format,
            },
        );
    }

    Ok(catalog)
}

fn parse_kv(line: &str) -> Option<(&str, String)> {
    let (key, rest) = line.split_once('=')?;
    let key = key.trim();
    let value = rest.trim().trim_matches('"').to_owned();
    Some((key, value))
}

#[cfg(test)]
mod tests {
    use super::{
        ProductTransactionsConfig, ServerConfig, load_server_config, open_catalog,
        parse_raw_config, product_catalog_commit, validate_product_transactions,
    };
    use arrow::datatypes::{DataType, Field};
    use kaveon_core::{
        AccessPattern, CatalogAdapter, CatalogDefinition, CatalogId, CatalogLifecycle,
        ColumnDefinition, DataFormat, SchemaDefinition, SchemaId, StorageType, TableDefinition,
        TableId, TableReference,
    };
    use std::sync::Arc;

    fn temporary_directory() -> std::path::PathBuf {
        let path = std::env::temp_dir().join(format!("kaveon-server-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&path).unwrap();
        path
    }

    /// A data directory registers `*.parquet` files, Delta directories and
    /// directories of Parquet files; a directory holding no data files, or a
    /// file that is not data, registers nothing.
    #[test]
    fn a_data_directory_registers_directory_parquet_tables() {
        use kaveon_core::CatalogProvider;
        let directory = temporary_directory();
        let schema = Arc::new(arrow::datatypes::Schema::new(vec![Field::new(
            "id",
            DataType::Int64,
            false,
        )]));
        let write = |path: &std::path::Path, values: Vec<i64>| {
            let batch = arrow::record_batch::RecordBatch::try_new(
                Arc::clone(&schema),
                vec![Arc::new(arrow::array::Int64Array::from(values))],
            )
            .unwrap();
            let mut writer = parquet::arrow::ArrowWriter::try_new(
                std::fs::File::create(path).unwrap(),
                Arc::clone(&schema),
                None,
            )
            .unwrap();
            writer.write(&batch).unwrap();
            writer.close().unwrap();
        };
        write(&directory.join("single.parquet"), vec![1]);
        std::fs::create_dir_all(directory.join("parts")).unwrap();
        write(&directory.join("parts").join("part-0.parquet"), vec![2]);
        write(&directory.join("parts").join("part-1.parquet"), vec![3]);
        std::fs::write(directory.join("parts").join("_SUCCESS"), b"").unwrap();
        std::fs::create_dir_all(directory.join("notes")).unwrap();
        std::fs::write(directory.join("notes").join("readme.txt"), b"x").unwrap();
        std::fs::create_dir_all(directory.join("empty")).unwrap();

        let catalog = super::build_local_catalog("kaveon", &directory);
        let mut tables = catalog.table_names("default").unwrap();
        tables.sort();
        assert_eq!(tables, ["parts", "single"]);
        let parts = catalog.table("default", "parts").unwrap().unwrap();
        assert_eq!(parts.location, "parts");
        assert_eq!(parts.format, DataFormat::Parquet);
        assert_eq!(parts.arrow_schema, schema);
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn catalog_configuration_loads_database_and_admin_token() {
        let directory = temporary_directory();
        let config_path = directory.join("config.toml");
        std::fs::write(
            &config_path,
            "[catalog]\ndatabase_path = \"state/catalog.db\"\nadmin_token = \"test-token\"\n",
        )
        .unwrap();

        let config = load_server_config(&config_path).unwrap();
        assert_eq!(
            config.catalog_database_path,
            std::path::PathBuf::from("state/catalog.db")
        );
        assert_eq!(config.catalog_admin_token.as_deref(), Some("test-token"));
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn json_rendered_config_is_accepted_for_aca_compatibility() {
        let directory = temporary_directory();
        let config_path = directory.join("config.toml");
        std::fs::write(
            &config_path,
            r#"{"node":{"id":"aca-coordinator","coordinator":true},"http":{"port":8090},"catalog":{"database_path":"state/catalog.db","admin_token":"test-token"}}"#,
        )
        .unwrap();
        let config = load_server_config(&config_path).unwrap();
        assert_eq!(config.node_id, "aca-coordinator");
        assert!(config.coordinator);
        assert_eq!(config.http_port, 8090);
        assert_eq!(config.catalog_admin_token.as_deref(), Some("test-token"));
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn malformed_config_requires_explicit_environment_fallback() {
        let malformed = "{node: not-json}";
        assert!(parse_raw_config(malformed, false).is_err());
        let fallback = parse_raw_config(malformed, true).unwrap();
        assert!(fallback.node.is_none());
        assert!(fallback.catalog.is_none());
    }

    #[test]
    fn result_cache_configuration_defaults_and_loads_from_the_file() {
        let directory = temporary_directory();
        let config_path = directory.join("config.toml");
        let defaults = load_server_config(&directory.join("missing.toml")).unwrap();
        assert_eq!(defaults.result_cache_bytes, 256 * 1024 * 1024);
        assert_eq!(defaults.result_cache_ttl_seconds, 600);
        std::fs::write(
            &config_path,
            "[result_cache]
bytes = 1048576
ttl_seconds = 30
",
        )
        .unwrap();
        let config = load_server_config(&config_path).unwrap();
        assert_eq!(config.result_cache_bytes, 1_048_576);
        assert_eq!(config.result_cache_ttl_seconds, 30);
        std::fs::write(
            &config_path,
            "[result_cache]
bytes = 1
ttl_seconds = 0
",
        )
        .unwrap();
        assert!(load_server_config(&config_path).is_err());
        std::fs::write(
            &config_path,
            "[result_cache]
bytes = 0
ttl_seconds = 0
",
        )
        .unwrap();
        assert_eq!(
            load_server_config(&config_path).unwrap().result_cache_bytes,
            0
        );
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn admission_queue_configuration_defaults_and_loads_from_the_file() {
        let directory = temporary_directory();
        let config_path = directory.join("config.toml");
        let defaults = load_server_config(&directory.join("missing.toml")).unwrap();
        assert_eq!(defaults.memory_admission_queue, 64);
        assert_eq!(defaults.memory_admission_wait_seconds, 60);
        std::fs::write(
            &config_path,
            "[memory]
admission_queue = 8
admission_wait_seconds = 5
",
        )
        .unwrap();
        let config = load_server_config(&config_path).unwrap();
        assert_eq!(config.memory_admission_queue, 8);
        assert_eq!(config.memory_admission_wait_seconds, 5);
        // A queue nobody may wait in is a contradiction; no queue needs no wait.
        std::fs::write(
            &config_path,
            "[memory]
admission_queue = 8
admission_wait_seconds = 0
",
        )
        .unwrap();
        assert!(load_server_config(&config_path).is_err());
        std::fs::write(
            &config_path,
            "[memory]
admission_queue = 0
admission_wait_seconds = 0
",
        )
        .unwrap();
        assert_eq!(
            load_server_config(&config_path)
                .unwrap()
                .memory_admission_queue,
            0
        );
        std::fs::remove_dir_all(directory).unwrap();
    }

    /// The `[resource_groups]` section loads and validates; the state
    /// directory follows the catalog store unless `node.state_dir` says
    /// otherwise; the section and the legacy security list together are
    /// refused; the loader's precedence and the built-in fallback hold.
    #[test]
    fn resource_group_configuration_loads_from_the_file_and_falls_back_to_the_builtin() {
        let directory = temporary_directory();
        let config_path = directory.join("config.toml");
        let defaults = load_server_config(&directory.join("missing.toml")).unwrap();
        assert!(defaults.resource_groups_section.is_none());
        assert_eq!(defaults.state_dir, std::path::PathBuf::from("."));
        let (groups, source, store_path) =
            crate::resource_groups::load(&defaults, defaults.resource_groups_section.clone())
                .unwrap();
        assert_eq!(source, crate::resource_groups::Source::Builtin);
        assert_eq!(
            groups.groups[0].max_concurrent,
            defaults.principal_query_limit
        );
        assert_eq!(
            store_path,
            std::path::PathBuf::from("./resource-groups.json")
        );

        std::fs::write(
            &config_path,
            format!(
                "[catalog]
database_path = \"{}\"

[[resource_groups.groups]]
name = \"default\"
max_concurrent = 2

[[resource_groups.groups]]
name = \"etl\"
max_concurrent = 1
max_queued = 3
max_queue_wait_seconds = 120
priority = 2
default_settings = {{ result_cache = false }}

[[resource_groups.selectors]]
client_tag = \"etl\"
group = \"etl\"
",
                directory
                    .join("catalog")
                    .join("kaveon-catalog.db")
                    .display()
                    .to_string()
                    .replace('\\', "/")
            ),
        )
        .unwrap();
        let config = load_server_config(&config_path).unwrap();
        assert_eq!(config.state_dir, directory.join("catalog"));
        assert_eq!(config.audit_dir, directory.join("catalog").join("audit"));
        assert_eq!(config.audit_retention_days, 90);
        assert_eq!(config.audit_segment_bytes, 64 * 1024 * 1024);
        let section = config.resource_groups_section.clone().unwrap();
        assert_eq!(section.groups[1].name, "etl");
        assert_eq!(section.groups[1].max_queue_wait_seconds, 120);
        assert_eq!(section.groups[1].default_settings["result_cache"], false);
        let (groups, source, store_path) =
            crate::resource_groups::load(&config, config.resource_groups_section.clone()).unwrap();
        assert_eq!(source, crate::resource_groups::Source::ConfigFile);
        assert_eq!(groups.selectors[0].client_tag.as_deref(), Some("etl"));
        assert_eq!(
            store_path,
            directory.join("catalog").join("resource-groups.json")
        );

        // A durable runtime copy beside the catalog store wins over the section.
        std::fs::create_dir_all(directory.join("catalog")).unwrap();
        std::fs::write(
            directory.join("catalog").join("resource-groups.json"),
            r#"{"groups":[{"name":"default","max_concurrent":7}],"selectors":[]}"#,
        )
        .unwrap();
        let (groups, source, _) =
            crate::resource_groups::load(&config, config.resource_groups_section.clone()).unwrap();
        assert_eq!(source, crate::resource_groups::Source::Runtime);
        assert_eq!(groups.groups[0].max_concurrent, 7);

        // A section that does not validate is refused at load; the audit
        // section loads beside it.
        std::fs::write(
            &config_path,
            "[audit]
dir = \"/var/lib/kaveon/ledger\"
retention_days = 7
segment_bytes = 2097152

[[resource_groups.groups]]
name = \"etl\"
max_concurrent = 1
",
        )
        .unwrap();
        let config = load_server_config(&config_path).unwrap();
        assert_eq!(
            config.audit_dir,
            std::path::PathBuf::from("/var/lib/kaveon/ledger")
        );
        assert_eq!(config.audit_retention_days, 7);
        assert_eq!(config.audit_segment_bytes, 2 * 1024 * 1024);
        let error = crate::resource_groups::load(&config, config.resource_groups_section.clone())
            .unwrap_err()
            .to_string();
        assert!(error.contains("default"), "{error}");
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn product_transaction_configuration_is_coordinator_only_and_secret_free() {
        let mut config = ServerConfig {
            product_transactions: ProductTransactionsConfig {
                enabled: true,
                account: "kaveontest".into(),
                container: "product".into(),
                prefix: "kaveon/product-catalog".into(),
                storage_mode: "adls".into(),
                local_path: "/tmp/kaveon-product-transactions".into(),
            },
            ..ServerConfig::default()
        };
        validate_product_transactions(&config).unwrap();
        assert!(product_catalog_commit(&config).unwrap().is_some());
        config.coordinator = false;
        assert!(validate_product_transactions(&config).is_err());
    }

    #[test]
    fn enabled_product_transactions_reject_missing_or_unsafe_storage_coordinates() {
        let mut config = ServerConfig {
            product_transactions: ProductTransactionsConfig {
                enabled: true,
                ..Default::default()
            },
            ..ServerConfig::default()
        };
        assert!(validate_product_transactions(&config).is_err());
        config.product_transactions.account = "Unsafe Account".into();
        config.product_transactions.container = "product".into();
        assert!(product_catalog_commit(&config).is_err());
        config.product_transactions.account = "kaveontest".into();
        config.product_transactions.prefix = "../escape".into();
        assert!(product_catalog_commit(&config).is_err());
    }

    #[test]
    fn local_product_transactions_need_only_a_durable_path() {
        let directory = temporary_directory();
        let mut config = ServerConfig {
            product_transactions: ProductTransactionsConfig {
                enabled: true,
                storage_mode: "local".into(),
                local_path: directory.join("products"),
                ..Default::default()
            },
            ..ServerConfig::default()
        };
        validate_product_transactions(&config).unwrap();
        assert!(product_catalog_commit(&config).unwrap().is_some());
        config.product_transactions.local_path = std::path::PathBuf::new();
        assert!(validate_product_transactions(&config).is_err());
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn bootstrap_retires_a_data_directory_table_whose_file_is_gone_and_keeps_the_rest() {
        let data_dir = temporary_directory();
        let schema = Arc::new(arrow::datatypes::Schema::new(vec![Field::new(
            "value",
            DataType::Int64,
            false,
        )]));
        let write = |path: &std::path::Path| {
            let batch = arrow::record_batch::RecordBatch::try_new(
                Arc::clone(&schema),
                vec![Arc::new(arrow::array::Int64Array::from(vec![1]))],
            )
            .unwrap();
            let mut writer = parquet::arrow::ArrowWriter::try_new(
                std::fs::File::create(path).unwrap(),
                Arc::clone(&schema),
                None,
            )
            .unwrap();
            writer.write(&batch).unwrap();
            writer.close().unwrap();
        };
        write(&data_dir.join("keep.parquet"));
        write(&data_dir.join("gone.parquet"));
        let config = ServerConfig {
            data_dir: Some(data_dir.clone()),
            catalog_database_path: data_dir.join("catalog.db"),
            ..ServerConfig::default()
        };
        let (store, manager) = open_catalog(&config).unwrap();
        assert!(
            manager
                .resolve_table(&TableReference::parse("kaveon.default.gone"))
                .is_ok()
        );
        // A table a person registered, with a location that does not
        // exist, is not the bootstrap's to remove.
        let theirs = TableDefinition::new(
            TableId::new("table:kaveon:default:theirs").unwrap(),
            SchemaId::new("schema:kaveon:default").unwrap(),
            "theirs",
            "elsewhere.parquet",
            AccessPattern::Shortcut,
            DataFormat::Parquet,
            vec![ColumnDefinition::new("value", DataType::Int64, true).unwrap()],
        )
        .unwrap()
        .transition(CatalogLifecycle::Active)
        .unwrap();
        store.create_table("analyst", &theirs).unwrap();
        drop(store);
        std::fs::remove_file(data_dir.join("gone.parquet")).unwrap();

        let (store, manager) = open_catalog(&config).unwrap();
        assert!(
            manager
                .resolve_table(&TableReference::parse("kaveon.default.keep"))
                .is_ok()
        );
        assert!(
            manager
                .resolve_table(&TableReference::parse("kaveon.default.theirs"))
                .is_ok()
        );
        assert!(
            manager
                .resolve_table(&TableReference::parse("kaveon.default.gone"))
                .is_err(),
            "the table whose file is gone is retired"
        );
        assert_eq!(
            store
                .creator("table", "table:kaveon:default:theirs")
                .unwrap()
                .as_deref(),
            Some("analyst")
        );
        drop(store);
        std::fs::remove_dir_all(&data_dir).unwrap();
    }

    #[test]
    fn durable_catalog_survives_server_reopen() {
        let directory = temporary_directory();
        let config = ServerConfig {
            catalog_database_path: directory.join("catalog.db"),
            ..ServerConfig::default()
        };
        let (store, _) = open_catalog(&config).unwrap();
        let definition = CatalogDefinition::new(
            CatalogId::new("catalog:durable").unwrap(),
            "durable",
            CatalogAdapter::Native,
            StorageType::Local {
                base_path: directory.clone(),
            },
        )
        .unwrap()
        .transition(CatalogLifecycle::Active)
        .unwrap();
        store
            .create_catalog("test", &definition)
            .expect("catalog creation must succeed");
        let schema = SchemaDefinition::new(
            SchemaId::new("schema:durable:default").unwrap(),
            definition.id().clone(),
            "default",
        )
        .unwrap()
        .transition(CatalogLifecycle::Active)
        .unwrap();
        store.create_schema("test", &schema).unwrap();
        let nested_type = DataType::List(Arc::new(Field::new("element", DataType::Int64, true)));
        let table = TableDefinition::new(
            TableId::new("table:durable:default:nested").unwrap(),
            schema.id().clone(),
            "nested",
            "nested.parquet",
            AccessPattern::Shortcut,
            DataFormat::Parquet,
            vec![ColumnDefinition::new("items", nested_type.clone(), true).unwrap()],
        )
        .unwrap()
        .transition(CatalogLifecycle::Active)
        .unwrap();
        store.create_table("test", &table).unwrap();
        drop(store);

        let (reopened, manager) = open_catalog(&config).unwrap();
        assert_eq!(
            reopened.catalog_by_name("durable").unwrap().unwrap().id(),
            definition.id()
        );
        let resolved = manager
            .resolve_table(&TableReference::parse("durable.default.nested"))
            .unwrap();
        assert_eq!(
            resolved.table.arrow_schema.field(0).data_type(),
            &nested_type
        );
        drop(reopened);
        std::fs::remove_dir_all(directory).unwrap();
    }
}
