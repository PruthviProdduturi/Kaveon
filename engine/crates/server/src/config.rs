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

#[derive(Debug, Clone)]
pub struct ServerConfig {
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
}

impl Default for ServerConfig {
    fn default() -> Self {
        Self {
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
        }
    }
}

#[derive(Deserialize)]
struct RawConfig {
    node: Option<NodeConfig>,
    http: Option<HttpConfig>,
    discovery: Option<DiscoveryConfig>,
    storage: Option<StorageConfig>,
    exchange: Option<ExchangeConfig>,
    memory: Option<MemoryConfig>,
    catalog: Option<NativeCatalogConfig>,
}

#[derive(Deserialize)]
struct NodeConfig {
    id: Option<String>,
    environment: Option<String>,
    coordinator: Option<bool>,
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
}

#[derive(Deserialize)]
struct NativeCatalogConfig {
    database_path: Option<String>,
    admin_token: Option<String>,
}

pub fn default_config_path() -> PathBuf {
    PathBuf::from("/etc/kaveon/config.toml")
}

pub fn load_server_config(path: &Path) -> anyhow::Result<ServerConfig> {
    let mut config = ServerConfig::default();

    if path.exists() {
        let content = std::fs::read_to_string(path)?;
        let raw: RawConfig = toml::from_str(&content)?;

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
            }
        }
        if let Some(catalog) = raw.catalog {
            if let Some(path) = catalog.database_path {
                config.catalog_database_path = PathBuf::from(path);
            }
            config.catalog_admin_token = catalog.admin_token;
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
    if config.query_memory_limit_bytes == 0 {
        anyhow::bail!("query memory limit must be greater than zero");
    }
    if config.memory_admission_limit_bytes < config.query_memory_limit_bytes {
        anyhow::bail!("memory admission limit must be at least the per-query limit");
    }

    Ok(config)
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
    for catalog_name in discovered.catalog_names() {
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
            for table_name in provider.table_names(&schema_name)? {
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
                DataFormat::Delta => DeltaTableReader::new(&full_path).metadata(),
                DataFormat::Parquet => ParquetReader::new(&full_path).metadata(),
                DataFormat::Iceberg => anyhow::bail!("local Iceberg metadata is not implemented"),
            }
            .map(|m| m.schema)
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
    use super::{ServerConfig, load_server_config, open_catalog};
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
