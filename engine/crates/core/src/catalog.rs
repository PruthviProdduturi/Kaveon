use arrow::datatypes::SchemaRef;
use crate::Result;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

#[derive(Debug, Clone)]
pub enum StorageType {
    Local {
        base_path: PathBuf,
    },
    AdlsGen2 {
        account: String,
        container: String,
        root_path: String,
    },
    S3 {
        bucket: String,
        region: String,
        prefix: String,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AccessPattern {
    Shortcut,
    Optimized,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DataFormat {
    Parquet,
    Delta,
    Iceberg,
}

#[derive(Debug, Clone)]
pub struct TableMeta {
    pub name: String,
    pub arrow_schema: SchemaRef,
    pub location: String,
    pub access: AccessPattern,
    pub format: DataFormat,
}

#[derive(Debug, Clone)]
pub enum TableReference {
    Bare {
        table: String,
    },
    Partial {
        schema: String,
        table: String,
    },
    Full {
        catalog: String,
        schema: String,
        table: String,
    },
}

impl TableReference {
    pub fn parse(name: &str) -> Self {
        let parts: Vec<&str> = name.split('.').collect();
        match parts.len() {
            3 => Self::Full {
                catalog: parts[0].to_owned(),
                schema: parts[1].to_owned(),
                table: parts[2].to_owned(),
            },
            2 => Self::Partial {
                schema: parts[0].to_owned(),
                table: parts[1].to_owned(),
            },
            _ => Self::Bare {
                table: name.to_owned(),
            },
        }
    }

    pub fn table(&self) -> &str {
        match self {
            Self::Bare { table }
            | Self::Partial { table, .. }
            | Self::Full { table, .. } => table,
        }
    }
}

pub trait CatalogProvider: Send + Sync {
    fn name(&self) -> &str;
    fn storage_type(&self) -> &StorageType;
    fn schema_names(&self) -> Vec<String>;
    fn table_names(&self, schema: &str) -> Result<Vec<String>>;
    fn table(&self, schema: &str, table: &str) -> Result<Option<Arc<TableMeta>>>;
    fn register_table(&mut self, schema: &str, table: TableMeta) -> Result<()>;
}

pub struct CatalogManager {
    catalogs: HashMap<String, Box<dyn CatalogProvider>>,
    default_catalog: String,
    default_schema: String,
}

impl CatalogManager {
    pub fn new(default_catalog: impl Into<String>, default_schema: impl Into<String>) -> Self {
        Self {
            catalogs: HashMap::new(),
            default_catalog: default_catalog.into(),
            default_schema: default_schema.into(),
        }
    }

    pub fn register_catalog(&mut self, catalog: Box<dyn CatalogProvider>) {
        let name = catalog.name().to_owned();
        self.catalogs.insert(name, catalog);
    }

    pub fn catalog(&self, name: &str) -> Option<&dyn CatalogProvider> {
        self.catalogs.get(name).map(|c| c.as_ref())
    }

    pub fn catalog_mut(&mut self, name: &str) -> Option<&mut dyn CatalogProvider> {
        self.catalogs.get_mut(name).map(|c| c.as_mut())
    }

    pub fn catalog_names(&self) -> Vec<String> {
        self.catalogs.keys().cloned().collect()
    }

    pub fn default_catalog(&self) -> &str {
        &self.default_catalog
    }

    pub fn default_schema(&self) -> &str {
        &self.default_schema
    }

    pub fn resolve_table(&self, reference: &TableReference) -> Result<ResolvedTable> {
        let (catalog_name, schema_name, table_name) = match reference {
            TableReference::Full {
                catalog,
                schema,
                table,
            } => (catalog.as_str(), schema.as_str(), table.as_str()),
            TableReference::Partial { schema, table } => {
                (self.default_catalog.as_str(), schema.as_str(), table.as_str())
            }
            TableReference::Bare { table } => (
                self.default_catalog.as_str(),
                self.default_schema.as_str(),
                table.as_str(),
            ),
        };

        let catalog = self.catalogs.get(catalog_name).ok_or_else(|| {
            crate::KaveonError::Execution(format!("catalog '{catalog_name}' not found"))
        })?;

        let table = catalog.table(schema_name, table_name)?.ok_or_else(|| {
            crate::KaveonError::Execution(format!(
                "table '{catalog_name}.{schema_name}.{table_name}' not found"
            ))
        })?;

        Ok(ResolvedTable {
            catalog: catalog_name.to_owned(),
            schema: schema_name.to_owned(),
            table,
            storage: catalog.storage_type().clone(),
        })
    }
}

#[derive(Debug, Clone)]
pub struct ResolvedTable {
    pub catalog: String,
    pub schema: String,
    pub table: Arc<TableMeta>,
    pub storage: StorageType,
}

impl ResolvedTable {
    pub fn full_path(&self) -> String {
        match &self.storage {
            StorageType::Local { base_path } => {
                base_path.join(&self.table.location).to_string_lossy().into_owned()
            }
            StorageType::AdlsGen2 {
                account,
                container,
                root_path,
            } => format!(
                "abfss://{container}@{account}.dfs.core.windows.net/{root_path}/{}",
                self.table.location
            ),
            StorageType::S3 {
                bucket,
                prefix,
                ..
            } => format!("s3://{bucket}/{prefix}/{}", self.table.location),
        }
    }
}

pub struct MemoryCatalog {
    name: String,
    storage: StorageType,
    schemas: HashMap<String, HashMap<String, Arc<TableMeta>>>,
}

impl MemoryCatalog {
    pub fn new(name: impl Into<String>, storage: StorageType) -> Self {
        Self {
            name: name.into(),
            storage,
            schemas: HashMap::new(),
        }
    }

    pub fn with_schema(mut self, schema: impl Into<String>) -> Self {
        self.schemas.entry(schema.into()).or_default();
        self
    }
}

impl CatalogProvider for MemoryCatalog {
    fn name(&self) -> &str {
        &self.name
    }

    fn storage_type(&self) -> &StorageType {
        &self.storage
    }

    fn schema_names(&self) -> Vec<String> {
        self.schemas.keys().cloned().collect()
    }

    fn table_names(&self, schema: &str) -> Result<Vec<String>> {
        match self.schemas.get(schema) {
            Some(tables) => Ok(tables.keys().cloned().collect()),
            None => Err(crate::KaveonError::Execution(format!(
                "schema '{schema}' not found in catalog '{}'",
                self.name
            ))),
        }
    }

    fn table(&self, schema: &str, table: &str) -> Result<Option<Arc<TableMeta>>> {
        match self.schemas.get(schema) {
            Some(tables) => Ok(tables.get(table).cloned()),
            None => Err(crate::KaveonError::Execution(format!(
                "schema '{schema}' not found in catalog '{}'",
                self.name
            ))),
        }
    }

    fn register_table(&mut self, schema: &str, table: TableMeta) -> Result<()> {
        let tables = self.schemas.entry(schema.to_owned()).or_default();
        let name = table.name.clone();
        tables.insert(name, Arc::new(table));
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use arrow::datatypes::{DataType, Field, Schema};

    fn test_schema() -> SchemaRef {
        Arc::new(Schema::new(vec![
            Field::new("id", DataType::Int64, false),
            Field::new("name", DataType::Utf8, true),
        ]))
    }

    #[test]
    fn parses_table_references() {
        match TableReference::parse("users") {
            TableReference::Bare { table } => assert_eq!(table, "users"),
            _ => panic!("expected Bare"),
        }
        match TableReference::parse("public.users") {
            TableReference::Partial { schema, table } => {
                assert_eq!(schema, "public");
                assert_eq!(table, "users");
            }
            _ => panic!("expected Partial"),
        }
        match TableReference::parse("lakehouse.raw.events") {
            TableReference::Full {
                catalog,
                schema,
                table,
            } => {
                assert_eq!(catalog, "lakehouse");
                assert_eq!(schema, "raw");
                assert_eq!(table, "events");
            }
            _ => panic!("expected Full"),
        }
    }

    #[test]
    fn resolves_bare_reference_with_defaults() {
        let mut catalog = MemoryCatalog::new(
            "lakehouse",
            StorageType::Local {
                base_path: PathBuf::from("/data"),
            },
        )
        .with_schema("default");

        catalog
            .register_table(
                "default",
                TableMeta {
                    name: "users".into(),
                    arrow_schema: test_schema(),
                    location: "users.parquet".into(),
                    access: AccessPattern::Shortcut,
                    format: DataFormat::Parquet,
                },
            )
            .unwrap();

        let mut mgr = CatalogManager::new("lakehouse", "default");
        mgr.register_catalog(Box::new(catalog));

        let resolved = mgr
            .resolve_table(&TableReference::parse("users"))
            .unwrap();
        assert_eq!(resolved.table.name, "users");
        assert_eq!(resolved.catalog, "lakehouse");
        assert_eq!(resolved.schema, "default");
    }

    #[test]
    fn resolves_full_path_for_storage_types() {
        let mut catalog = MemoryCatalog::new(
            "azure",
            StorageType::AdlsGen2 {
                account: "kaveonsa".into(),
                container: "data".into(),
                root_path: "warehouse".into(),
            },
        )
        .with_schema("raw");

        catalog
            .register_table(
                "raw",
                TableMeta {
                    name: "events".into(),
                    arrow_schema: test_schema(),
                    location: "events/".into(),
                    access: AccessPattern::Optimized,
                    format: DataFormat::Delta,
                },
            )
            .unwrap();

        let mut mgr = CatalogManager::new("azure", "raw");
        mgr.register_catalog(Box::new(catalog));

        let resolved = mgr
            .resolve_table(&TableReference::parse("events"))
            .unwrap();
        assert_eq!(
            resolved.full_path(),
            "abfss://data@kaveonsa.dfs.core.windows.net/warehouse/events/"
        );
        assert_eq!(resolved.table.access, AccessPattern::Optimized);
        assert_eq!(resolved.table.format, DataFormat::Delta);
    }

    #[test]
    fn rejects_unknown_catalog_and_schema() {
        let mgr = CatalogManager::new("default", "public");
        assert!(mgr
            .resolve_table(&TableReference::parse("users"))
            .is_err());
    }

    #[test]
    fn registers_multiple_schemas_and_tables() {
        let mut catalog = MemoryCatalog::new(
            "local",
            StorageType::Local {
                base_path: PathBuf::from("/tmp"),
            },
        )
        .with_schema("raw")
        .with_schema("analytics");

        catalog
            .register_table(
                "raw",
                TableMeta {
                    name: "clicks".into(),
                    arrow_schema: test_schema(),
                    location: "clicks.parquet".into(),
                    access: AccessPattern::Shortcut,
                    format: DataFormat::Parquet,
                },
            )
            .unwrap();
        catalog
            .register_table(
                "analytics",
                TableMeta {
                    name: "daily_agg".into(),
                    arrow_schema: test_schema(),
                    location: "daily_agg.parquet".into(),
                    access: AccessPattern::Optimized,
                    format: DataFormat::Parquet,
                },
            )
            .unwrap();

        assert_eq!(catalog.table_names("raw").unwrap().len(), 1);
        assert_eq!(catalog.table_names("analytics").unwrap().len(), 1);
        assert!(catalog.table("raw", "clicks").unwrap().is_some());
        assert!(catalog.table("analytics", "daily_agg").unwrap().is_some());
        assert!(catalog.table("raw", "daily_agg").unwrap().is_none());
    }
}
