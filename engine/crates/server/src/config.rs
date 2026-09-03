use kaveon_core::{
    AccessPattern, CatalogManager, CatalogProvider, DataFormat, MemoryCatalog, StorageType,
    TableMeta,
};
use kaveon_storage::{DeltaTableReader, ParquetReader};
use serde::Deserialize;
use std::path::{Path, PathBuf};
use std::sync::Arc;

#[derive(Debug, Clone)]
pub struct ServerConfig {
    pub node_id: String,
    pub environment: String,
    pub coordinator: bool,
    pub http_port: u16,
    pub discovery_uri: String,
    pub data_dir: Option<PathBuf>,
    pub catalog_dir: Option<PathBuf>,
}

impl Default for ServerConfig {
    fn default() -> Self {
        Self {
            node_id: uuid::Uuid::new_v4().to_string(),
            environment: "production".into(),
            coordinator: true,
            http_port: 8080,
            discovery_uri: "http://localhost:8080".into(),
            data_dir: None,
            catalog_dir: None,
        }
    }
}

#[derive(Deserialize)]
struct RawConfig {
    node: Option<NodeConfig>,
    http: Option<HttpConfig>,
    discovery: Option<DiscoveryConfig>,
    storage: Option<StorageConfig>,
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
}

#[derive(Deserialize)]
struct StorageConfig {
    data_dir: Option<String>,
    catalog_dir: Option<String>,
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
        if let Some(disc) = raw.discovery
            && let Some(uri) = disc.uri
        {
            config.discovery_uri = uri;
        }
        if let Some(storage) = raw.storage {
            if let Some(dir) = storage.data_dir {
                config.data_dir = Some(PathBuf::from(dir));
            }
            if let Some(dir) = storage.catalog_dir {
                config.catalog_dir = Some(PathBuf::from(dir));
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
    if let Ok(v) = std::env::var("KAVEON_DATA_DIR") {
        config.data_dir = Some(PathBuf::from(v));
    }
    if let Ok(v) = std::env::var("KAVEON_CATALOG_DIR") {
        config.catalog_dir = Some(PathBuf::from(v));
    }

    Ok(config)
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
