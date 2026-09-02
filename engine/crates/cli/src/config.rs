use kaveon_core::{
    AccessPattern, CatalogManager, CatalogProvider, DataFormat, KaveonError, MemoryCatalog, Result,
    StorageType, TableMeta,
};
use kaveon_storage::ParquetReader;
use std::path::{Path, PathBuf};
use std::sync::Arc;

pub fn default_config_path() -> PathBuf {
    dirs_home().join(".kaveon").join("config.toml")
}

pub fn default_catalogs_dir() -> PathBuf {
    dirs_home().join(".kaveon").join("catalogs")
}

pub fn load_config(path: &Path) -> Result<CatalogManager> {
    let catalogs_dir = path
        .parent()
        .map(|p| p.join("catalogs"))
        .unwrap_or_else(default_catalogs_dir);

    if catalogs_dir.is_dir() {
        return load_catalog_files(&catalogs_dir, path);
    }

    if !path.exists() {
        return Err(KaveonError::Io(std::io::Error::new(
            std::io::ErrorKind::NotFound,
            format!("config not found: {}", path.display()),
        )));
    }

    let content = std::fs::read_to_string(path)?;
    parse_config(&content)
}

fn load_catalog_files(dir: &Path, config_path: &Path) -> Result<CatalogManager> {
    let mut default_catalog = "kaveon".to_owned();
    let mut default_schema = "default".to_owned();

    if config_path.exists() {
        let content = std::fs::read_to_string(config_path)?;
        for line in content.lines() {
            let trimmed = line.trim();
            if let Some((key, value)) = parse_kv(trimmed) {
                match key {
                    "default_catalog" => default_catalog = value,
                    "default_schema" => default_schema = value,
                    _ => {}
                }
            }
        }
    }

    let mut mgr = CatalogManager::new(&default_catalog, &default_schema);

    let mut entries: Vec<_> = std::fs::read_dir(dir)?
        .flatten()
        .filter(|e| {
            e.path()
                .extension()
                .is_some_and(|ext| ext == "toml" || ext == "properties")
        })
        .collect();
    entries.sort_by_key(|e| e.file_name());

    for entry in entries {
        let path = entry.path();
        let catalog_name = path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("unknown")
            .to_owned();

        let content = std::fs::read_to_string(&path)?;
        match build_catalog_from_file(&catalog_name, &content) {
            Ok(catalog) => {
                mgr.register_catalog(Box::new(catalog));
            }
            Err(e) => {
                eprintln!("warning: skipping catalog '{}': {e}", path.display());
            }
        }
    }

    Ok(mgr)
}

fn build_catalog_from_file(name: &str, content: &str) -> Result<MemoryCatalog> {
    let mut entry = CatalogEntry {
        name: name.to_owned(),
        ..Default::default()
    };
    let mut current_table: Option<TableEntry> = None;
    let mut in_table = false;

    for line in content.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        if trimmed == "[[table]]" {
            if let Some(tbl) = current_table.take() {
                entry.tables.push(tbl);
            }
            current_table = Some(TableEntry::default());
            in_table = true;
            continue;
        }
        if let Some((key, value)) = parse_kv(trimmed) {
            if in_table {
                if let Some(tbl) = current_table.as_mut() {
                    match key {
                        "name" => tbl.name = value,
                        "schema" => tbl.schema = value,
                        "location" => tbl.location = value,
                        "access" => tbl.access = value,
                        "format" => tbl.format = value,
                        _ => {}
                    }
                }
            } else {
                match key {
                    "type" | "connector.name" => entry.storage_type = value,
                    "base_path" => entry.base_path = Some(value),
                    "account" => entry.account = Some(value),
                    "container" => entry.container = Some(value),
                    "root_path" => entry.root_path = Some(value),
                    "bucket" => entry.bucket = Some(value),
                    "region" => entry.region = Some(value),
                    "prefix" => entry.prefix = Some(value),
                    _ => {}
                }
            }
        }
    }
    if let Some(tbl) = current_table.take() {
        entry.tables.push(tbl);
    }

    build_catalog_from_entry(entry)
}

fn parse_config(content: &str) -> Result<CatalogManager> {
    let mut default_catalog = "kaveon".to_owned();
    let mut default_schema = "default".to_owned();
    let mut catalogs: Vec<CatalogEntry> = Vec::new();
    let mut current_catalog: Option<CatalogEntry> = None;
    let mut in_table = false;
    let mut current_table: Option<TableEntry> = None;

    for line in content.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }

        if trimmed == "[[catalog]]" {
            if let Some(mut cat) = current_catalog.take() {
                if let Some(tbl) = current_table.take() {
                    cat.tables.push(tbl);
                }
                catalogs.push(cat);
            }
            current_catalog = Some(CatalogEntry::default());
            in_table = false;
            continue;
        }

        if trimmed == "[[catalog.table]]" {
            if let Some(tbl) = current_table.take()
                && let Some(cat) = current_catalog.as_mut() {
                    cat.tables.push(tbl);
                }
            current_table = Some(TableEntry::default());
            in_table = true;
            continue;
        }

        if let Some((key, value)) = parse_kv(trimmed) {
            if in_table {
                if let Some(tbl) = current_table.as_mut() {
                    match key {
                        "name" => tbl.name = value,
                        "schema" => tbl.schema = value,
                        "location" => tbl.location = value,
                        "access" => tbl.access = value,
                        "format" => tbl.format = value,
                        _ => {}
                    }
                }
            } else if let Some(cat) = current_catalog.as_mut() {
                match key {
                    "name" => cat.name = value,
                    "type" => cat.storage_type = value,
                    "base_path" => cat.base_path = Some(value),
                    "account" => cat.account = Some(value),
                    "container" => cat.container = Some(value),
                    "root_path" => cat.root_path = Some(value),
                    "bucket" => cat.bucket = Some(value),
                    "region" => cat.region = Some(value),
                    "prefix" => cat.prefix = Some(value),
                    _ => {}
                }
            } else {
                match key {
                    "default_catalog" => default_catalog = value,
                    "default_schema" => default_schema = value,
                    _ => {}
                }
            }
        }
    }

    if let Some(mut cat) = current_catalog.take() {
        if let Some(tbl) = current_table.take() {
            cat.tables.push(tbl);
        }
        catalogs.push(cat);
    }

    let mut mgr = CatalogManager::new(default_catalog, default_schema);

    for entry in catalogs {
        match build_catalog_from_entry(entry) {
            Ok(catalog) => mgr.register_catalog(Box::new(catalog)),
            Err(e) => eprintln!("warning: skipping catalog: {e}"),
        }
    }

    Ok(mgr)
}

fn build_catalog_from_entry(entry: CatalogEntry) -> Result<MemoryCatalog> {
    let storage = match entry.storage_type.as_str() {
        "local" => StorageType::Local {
            base_path: PathBuf::from(entry.base_path.unwrap_or_else(|| ".".into())),
        },
        "adls_gen2" => StorageType::AdlsGen2 {
            account: entry.account.unwrap_or_default(),
            container: entry.container.unwrap_or_default(),
            root_path: entry.root_path.unwrap_or_default(),
        },
        "s3" => StorageType::S3 {
            bucket: entry.bucket.unwrap_or_default(),
            region: entry.region.unwrap_or_else(|| "us-east-1".into()),
            prefix: entry.prefix.unwrap_or_default(),
        },
        other => {
            return Err(KaveonError::Execution(format!(
                "unknown catalog type '{other}'"
            )));
        }
    };

    let mut catalog = MemoryCatalog::new(&entry.name, storage.clone()).with_schema("default");

    if entry.tables.is_empty() {
        if let StorageType::Local { ref base_path } = storage {
            auto_discover_tables(&mut catalog, base_path);
        }
    } else {
        for tbl in &entry.tables {
            let schema_name = if tbl.schema.is_empty() {
                "default"
            } else {
                &tbl.schema
            };
            catalog = catalog.with_schema(schema_name);
            let access = match tbl.access.as_str() {
                "optimized" => AccessPattern::Optimized,
                _ => AccessPattern::Shortcut,
            };
            let format = match tbl.format.as_str() {
                "delta" => DataFormat::Delta,
                "iceberg" => DataFormat::Iceberg,
                _ => DataFormat::Parquet,
            };

            let arrow_schema = if let StorageType::Local { ref base_path } = storage {
                let full_path = base_path.join(&tbl.location);
                ParquetReader::new(&full_path)
                    .metadata()
                    .map(|m| m.schema)
                    .unwrap_or_else(|_| Arc::new(arrow::datatypes::Schema::empty()))
            } else {
                Arc::new(arrow::datatypes::Schema::empty())
            };

            let _ = catalog.register_table(
                schema_name,
                TableMeta {
                    name: tbl.name.clone(),
                    arrow_schema,
                    location: tbl.location.clone(),
                    access,
                    format,
                },
            );
        }
    }

    Ok(catalog)
}

fn auto_discover_tables(catalog: &mut MemoryCatalog, dir: &Path) {
    if let Ok(entries) = std::fs::read_dir(dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().is_some_and(|e| e == "parquet")
                && let Some(table_name) = path.file_stem().and_then(|s| s.to_str())
                    && let Ok(meta) = ParquetReader::new(&path).metadata() {
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
}

fn parse_kv(line: &str) -> Option<(&str, String)> {
    let (key, rest) = line.split_once('=')?;
    let key = key.trim();
    let value = rest.trim().trim_matches('"').to_owned();
    Some((key, value))
}

fn dirs_home() -> PathBuf {
    std::env::var("HOME")
        .or_else(|_| std::env::var("USERPROFILE"))
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("."))
}

#[derive(Default)]
struct CatalogEntry {
    name: String,
    storage_type: String,
    base_path: Option<String>,
    account: Option<String>,
    container: Option<String>,
    root_path: Option<String>,
    bucket: Option<String>,
    region: Option<String>,
    prefix: Option<String>,
    tables: Vec<TableEntry>,
}

#[derive(Default)]
struct TableEntry {
    name: String,
    schema: String,
    location: String,
    access: String,
    format: String,
}
