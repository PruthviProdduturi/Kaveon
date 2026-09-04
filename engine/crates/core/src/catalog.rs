use crate::Result;
use arrow::datatypes::SchemaRef;
use arrow_schema::DataType;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum AccessPattern {
    Shortcut,
    Optimized,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum DataFormat {
    Parquet,
    Delta,
    Iceberg,
}

const MAX_CATALOG_IDENTIFIER_LENGTH: usize = 255;

fn validate_metadata_text(kind: &str, value: &str) -> Result<()> {
    if value.trim().is_empty() {
        return Err(crate::KaveonError::Execution(format!(
            "{kind} cannot be empty"
        )));
    }
    if value.len() > MAX_CATALOG_IDENTIFIER_LENGTH {
        return Err(crate::KaveonError::Execution(format!(
            "{kind} exceeds {MAX_CATALOG_IDENTIFIER_LENGTH} bytes"
        )));
    }
    if value.chars().any(char::is_control) {
        return Err(crate::KaveonError::Execution(format!(
            "{kind} cannot contain control characters"
        )));
    }
    Ok(())
}

macro_rules! metadata_id {
    ($name:ident, $kind:literal) => {
        #[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
        #[serde(transparent)]
        pub struct $name(String);

        impl $name {
            pub fn new(value: impl Into<String>) -> Result<Self> {
                let value = value.into();
                validate_metadata_text($kind, &value)?;
                Ok(Self(value))
            }

            pub fn as_str(&self) -> &str {
                &self.0
            }
        }
    };
}

metadata_id!(CatalogId, "catalog ID");
metadata_id!(SchemaId, "schema ID");
metadata_id!(TableId, "table ID");

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct CatalogRevision(u64);

impl CatalogRevision {
    pub fn new(value: u64) -> Result<Self> {
        if value == 0 {
            return Err(crate::KaveonError::Execution(
                "catalog revision must be greater than zero".into(),
            ));
        }
        Ok(Self(value))
    }

    pub const fn initial() -> Self {
        Self(1)
    }

    pub const fn value(self) -> u64 {
        self.0
    }

    pub fn next(self) -> Result<Self> {
        self.0
            .checked_add(1)
            .map(Self)
            .ok_or_else(|| crate::KaveonError::Execution("catalog revision overflow".into()))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum CatalogLifecycle {
    Draft,
    Active,
    Suspended,
    Deleting,
    Deleted,
}

impl CatalogLifecycle {
    pub fn validate_transition(self, target: Self) -> Result<()> {
        let valid = matches!(
            (self, target),
            (Self::Draft, Self::Active)
                | (Self::Draft, Self::Deleted)
                | (Self::Active, Self::Suspended)
                | (Self::Active, Self::Deleting)
                | (Self::Suspended, Self::Active)
                | (Self::Suspended, Self::Deleting)
                | (Self::Deleting, Self::Deleted)
        );
        if valid {
            Ok(())
        } else {
            Err(crate::KaveonError::Execution(format!(
                "invalid catalog lifecycle transition from {self:?} to {target:?}"
            )))
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum CredentialKind {
    ManagedIdentity,
    WorkloadIdentity,
    Environment,
    SecretStore,
}

/// An indirect credential handle. Secret material must never be stored here.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CredentialReference {
    kind: CredentialKind,
    reference: String,
}

impl CredentialReference {
    pub fn new(kind: CredentialKind, reference: impl Into<String>) -> Result<Self> {
        let reference = reference.into();
        validate_metadata_text("credential reference", &reference)?;
        Ok(Self { kind, reference })
    }

    pub const fn kind(&self) -> CredentialKind {
        self.kind
    }

    pub fn reference(&self) -> &str {
        &self.reference
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum CatalogAdapter {
    Native,
    HiveMetastore,
    AwsGlue,
    UnityCatalog,
    IcebergRest,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum CatalogCapability {
    DiscoverNamespaces,
    DiscoverTables,
    ReadMetadata,
    CreateNamespace,
    CreateTable,
    AlterTable,
    DropTable,
    AtomicCommit,
    Statistics,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AdapterCapabilities {
    adapter: CatalogAdapter,
    capabilities: Vec<CatalogCapability>,
}

impl AdapterCapabilities {
    pub fn for_adapter(adapter: CatalogAdapter) -> Self {
        use CatalogCapability as Capability;
        let capabilities = match adapter {
            CatalogAdapter::Native => vec![
                Capability::DiscoverNamespaces,
                Capability::DiscoverTables,
                Capability::ReadMetadata,
                Capability::CreateNamespace,
                Capability::CreateTable,
                Capability::AlterTable,
                Capability::DropTable,
                Capability::AtomicCommit,
                Capability::Statistics,
            ],
            CatalogAdapter::HiveMetastore => vec![
                Capability::DiscoverNamespaces,
                Capability::DiscoverTables,
                Capability::ReadMetadata,
                Capability::CreateNamespace,
                Capability::CreateTable,
                Capability::AlterTable,
                Capability::DropTable,
                Capability::Statistics,
            ],
            CatalogAdapter::AwsGlue | CatalogAdapter::UnityCatalog => vec![
                Capability::DiscoverNamespaces,
                Capability::DiscoverTables,
                Capability::ReadMetadata,
                Capability::CreateNamespace,
                Capability::CreateTable,
                Capability::AlterTable,
                Capability::DropTable,
                Capability::Statistics,
            ],
            CatalogAdapter::IcebergRest => vec![
                Capability::DiscoverNamespaces,
                Capability::DiscoverTables,
                Capability::ReadMetadata,
                Capability::CreateNamespace,
                Capability::CreateTable,
                Capability::AlterTable,
                Capability::DropTable,
                Capability::AtomicCommit,
            ],
        };
        Self {
            adapter,
            capabilities,
        }
    }

    pub const fn adapter(&self) -> CatalogAdapter {
        self.adapter
    }

    pub fn supports(&self, capability: CatalogCapability) -> bool {
        self.capabilities.contains(&capability)
    }

    pub fn capabilities(&self) -> &[CatalogCapability] {
        &self.capabilities
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ColumnDefinition {
    name: String,
    data_type: DataType,
    nullable: bool,
}

impl ColumnDefinition {
    pub fn new(name: impl Into<String>, data_type: DataType, nullable: bool) -> Result<Self> {
        let name = name.into();
        validate_metadata_text("column name", &name)?;
        Ok(Self {
            name,
            data_type,
            nullable,
        })
    }

    pub fn name(&self) -> &str {
        &self.name
    }
    pub const fn data_type(&self) -> &DataType {
        &self.data_type
    }
    pub const fn nullable(&self) -> bool {
        self.nullable
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CatalogDefinition {
    id: CatalogId,
    name: String,
    revision: CatalogRevision,
    adapter: CatalogAdapter,
    storage: StorageType,
    credential: Option<CredentialReference>,
    lifecycle: CatalogLifecycle,
}

impl CatalogDefinition {
    pub fn new(
        id: CatalogId,
        name: impl Into<String>,
        adapter: CatalogAdapter,
        storage: StorageType,
    ) -> Result<Self> {
        let name = name.into();
        validate_metadata_text("catalog name", &name)?;
        Ok(Self {
            id,
            name,
            revision: CatalogRevision::initial(),
            adapter,
            storage,
            credential: None,
            lifecycle: CatalogLifecycle::Draft,
        })
    }
    pub fn with_credential(mut self, credential: CredentialReference) -> Self {
        self.credential = Some(credential);
        self
    }
    pub fn id(&self) -> &CatalogId {
        &self.id
    }
    pub fn name(&self) -> &str {
        &self.name
    }
    pub const fn revision(&self) -> CatalogRevision {
        self.revision
    }
    pub const fn adapter(&self) -> CatalogAdapter {
        self.adapter
    }
    pub fn storage(&self) -> &StorageType {
        &self.storage
    }
    pub fn credential(&self) -> Option<&CredentialReference> {
        self.credential.as_ref()
    }
    pub const fn lifecycle(&self) -> CatalogLifecycle {
        self.lifecycle
    }
    pub fn transition(&self, target: CatalogLifecycle) -> Result<Self> {
        self.lifecycle.validate_transition(target)?;
        let mut next = self.clone();
        next.lifecycle = target;
        next.revision = self.revision.next()?;
        Ok(next)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SchemaDefinition {
    id: SchemaId,
    catalog_id: CatalogId,
    name: String,
    revision: CatalogRevision,
    lifecycle: CatalogLifecycle,
}

impl SchemaDefinition {
    pub fn new(id: SchemaId, catalog_id: CatalogId, name: impl Into<String>) -> Result<Self> {
        let name = name.into();
        validate_metadata_text("schema name", &name)?;
        Ok(Self {
            id,
            catalog_id,
            name,
            revision: CatalogRevision::initial(),
            lifecycle: CatalogLifecycle::Draft,
        })
    }
    pub fn id(&self) -> &SchemaId {
        &self.id
    }
    pub fn catalog_id(&self) -> &CatalogId {
        &self.catalog_id
    }
    pub fn name(&self) -> &str {
        &self.name
    }
    pub const fn revision(&self) -> CatalogRevision {
        self.revision
    }
    pub const fn lifecycle(&self) -> CatalogLifecycle {
        self.lifecycle
    }
    pub fn transition(&self, target: CatalogLifecycle) -> Result<Self> {
        self.lifecycle.validate_transition(target)?;
        let mut next = self.clone();
        next.lifecycle = target;
        next.revision = self.revision.next()?;
        Ok(next)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TableDefinition {
    id: TableId,
    schema_id: SchemaId,
    name: String,
    revision: CatalogRevision,
    location: String,
    access: AccessPattern,
    format: DataFormat,
    columns: Vec<ColumnDefinition>,
    lifecycle: CatalogLifecycle,
}

impl TableDefinition {
    pub fn new(
        id: TableId,
        schema_id: SchemaId,
        name: impl Into<String>,
        location: impl Into<String>,
        access: AccessPattern,
        format: DataFormat,
        columns: Vec<ColumnDefinition>,
    ) -> Result<Self> {
        let name = name.into();
        let location = location.into();
        validate_metadata_text("table name", &name)?;
        validate_metadata_text("table location", &location)?;
        if columns.is_empty() {
            return Err(crate::KaveonError::Execution(
                "table must contain at least one column".into(),
            ));
        }
        let mut names = std::collections::HashSet::new();
        if columns.iter().any(|column| !names.insert(column.name())) {
            return Err(crate::KaveonError::Execution(
                "table column names must be unique".into(),
            ));
        }
        Ok(Self {
            id,
            schema_id,
            name,
            revision: CatalogRevision::initial(),
            location,
            access,
            format,
            columns,
            lifecycle: CatalogLifecycle::Draft,
        })
    }
    pub fn id(&self) -> &TableId {
        &self.id
    }
    pub fn schema_id(&self) -> &SchemaId {
        &self.schema_id
    }
    pub fn name(&self) -> &str {
        &self.name
    }
    pub const fn revision(&self) -> CatalogRevision {
        self.revision
    }
    pub fn location(&self) -> &str {
        &self.location
    }
    pub const fn access(&self) -> AccessPattern {
        self.access
    }
    pub const fn format(&self) -> DataFormat {
        self.format
    }
    pub fn columns(&self) -> &[ColumnDefinition] {
        &self.columns
    }
    pub const fn lifecycle(&self) -> CatalogLifecycle {
        self.lifecycle
    }
    pub fn transition(&self, target: CatalogLifecycle) -> Result<Self> {
        self.lifecycle.validate_transition(target)?;
        let mut next = self.clone();
        next.lifecycle = target;
        next.revision = self.revision.next()?;
        Ok(next)
    }
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
            Self::Bare { table } | Self::Partial { table, .. } | Self::Full { table, .. } => table,
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

    pub fn catalog_mut(&mut self, name: &str) -> Option<&mut (dyn CatalogProvider + '_)> {
        match self.catalogs.get_mut(name) {
            Some(catalog) => Some(catalog.as_mut()),
            None => None,
        }
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

    pub fn set_default(&mut self, catalog_name: &str, schema_name: &str) -> Result<()> {
        let catalog = self.catalogs.get(catalog_name).ok_or_else(|| {
            crate::KaveonError::Execution(format!("catalog '{catalog_name}' not found"))
        })?;
        if !catalog
            .schema_names()
            .iter()
            .any(|name| name == schema_name)
        {
            return Err(crate::KaveonError::Execution(format!(
                "schema '{schema_name}' not found in catalog '{catalog_name}'"
            )));
        }
        self.default_catalog = catalog_name.to_owned();
        self.default_schema = schema_name.to_owned();
        Ok(())
    }

    pub fn resolve_table(&self, reference: &TableReference) -> Result<ResolvedTable> {
        let (catalog_name, schema_name, table_name) = match reference {
            TableReference::Full {
                catalog,
                schema,
                table,
            } => (catalog.as_str(), schema.as_str(), table.as_str()),
            TableReference::Partial { schema, table } => (
                self.default_catalog.as_str(),
                schema.as_str(),
                table.as_str(),
            ),
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
            StorageType::Local { base_path } => base_path
                .join(&self.table.location)
                .to_string_lossy()
                .into_owned(),
            StorageType::AdlsGen2 {
                account,
                container,
                root_path,
            } => format!(
                "abfss://{container}@{account}.dfs.core.windows.net/{root_path}/{}",
                self.table.location
            ),
            StorageType::S3 { bucket, prefix, .. } => {
                format!("s3://{bucket}/{prefix}/{}", self.table.location)
            }
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

        let resolved = mgr.resolve_table(&TableReference::parse("users")).unwrap();
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

        let resolved = mgr.resolve_table(&TableReference::parse("events")).unwrap();
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
        assert!(mgr.resolve_table(&TableReference::parse("users")).is_err());
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

    #[test]
    fn durable_definitions_have_stable_identity_and_monotonic_revisions() {
        let definition = CatalogDefinition::new(
            CatalogId::new("catalog-01").unwrap(),
            "lakehouse",
            CatalogAdapter::Native,
            StorageType::Local {
                base_path: PathBuf::from("/data"),
            },
        )
        .unwrap();

        let active = definition.transition(CatalogLifecycle::Active).unwrap();
        assert_eq!(active.id(), definition.id());
        assert_eq!(definition.revision(), CatalogRevision::initial());
        assert_eq!(active.revision().value(), 2);
        assert_eq!(definition.lifecycle(), CatalogLifecycle::Draft);
        assert_eq!(active.lifecycle(), CatalogLifecycle::Active);
    }

    #[test]
    fn lifecycle_rejects_invalid_and_terminal_transitions() {
        assert!(
            CatalogLifecycle::Draft
                .validate_transition(CatalogLifecycle::Suspended)
                .is_err()
        );
        assert!(
            CatalogLifecycle::Deleted
                .validate_transition(CatalogLifecycle::Active)
                .is_err()
        );
        assert!(CatalogRevision::new(0).is_err());
        assert!(CatalogRevision::new(u64::MAX).unwrap().next().is_err());
    }

    #[test]
    fn credential_contract_exposes_only_an_indirect_reference() {
        let credential =
            CredentialReference::new(CredentialKind::WorkloadIdentity, "identity/catalog-reader")
                .unwrap();
        let definition = CatalogDefinition::new(
            CatalogId::new("catalog-01").unwrap(),
            "lakehouse",
            CatalogAdapter::Native,
            StorageType::AdlsGen2 {
                account: "account".into(),
                container: "data".into(),
                root_path: "warehouse".into(),
            },
        )
        .unwrap()
        .with_credential(credential);

        assert_eq!(
            definition.credential().unwrap().kind(),
            CredentialKind::WorkloadIdentity
        );
        assert_eq!(
            definition.credential().unwrap().reference(),
            "identity/catalog-reader"
        );
    }

    #[test]
    fn adapter_capabilities_are_explicit() {
        let native = AdapterCapabilities::for_adapter(CatalogAdapter::Native);
        assert!(native.supports(CatalogCapability::AtomicCommit));
        assert!(native.supports(CatalogCapability::Statistics));

        let hive = AdapterCapabilities::for_adapter(CatalogAdapter::HiveMetastore);
        assert!(hive.supports(CatalogCapability::DiscoverTables));
        assert!(!hive.supports(CatalogCapability::AtomicCommit));

        let iceberg = AdapterCapabilities::for_adapter(CatalogAdapter::IcebergRest);
        assert!(iceberg.supports(CatalogCapability::AtomicCommit));
        assert!(!iceberg.supports(CatalogCapability::Statistics));
    }

    #[test]
    fn table_definition_validates_schema_and_remains_immutable() {
        let column = ColumnDefinition::new("order_id", DataType::Int64, false).unwrap();
        let table = TableDefinition::new(
            TableId::new("table-01").unwrap(),
            SchemaId::new("schema-01").unwrap(),
            "orders",
            "sales/orders",
            AccessPattern::Shortcut,
            DataFormat::Delta,
            vec![column.clone()],
        )
        .unwrap();
        assert_eq!(table.columns(), std::slice::from_ref(&column));
        assert_eq!(table.revision(), CatalogRevision::initial());

        assert!(
            TableDefinition::new(
                TableId::new("table-02").unwrap(),
                SchemaId::new("schema-01").unwrap(),
                "orders",
                "sales/orders",
                AccessPattern::Shortcut,
                DataFormat::Delta,
                Vec::new(),
            )
            .is_err()
        );

        let duplicate = ColumnDefinition::new("order_id", DataType::Utf8, true).unwrap();
        assert!(
            TableDefinition::new(
                TableId::new("table-03").unwrap(),
                SchemaId::new("schema-01").unwrap(),
                "orders",
                "sales/orders",
                AccessPattern::Shortcut,
                DataFormat::Delta,
                vec![column, duplicate],
            )
            .is_err()
        );
    }

    #[test]
    fn definitions_implement_serde_contracts() {
        fn assert_serde<T: serde::Serialize + for<'de> serde::Deserialize<'de>>() {}

        assert_serde::<CatalogDefinition>();
        assert_serde::<SchemaDefinition>();
        assert_serde::<TableDefinition>();
        assert_serde::<AdapterCapabilities>();
    }

    #[test]
    fn column_definition_round_trips_nested_arrow_types() {
        let column = ColumnDefinition::new(
            "items",
            DataType::List(Arc::new(arrow_schema::Field::new(
                "item",
                DataType::Decimal128(20, 4),
                true,
            ))),
            true,
        )
        .unwrap();

        let json = serde_json::to_string(&column).unwrap();
        let decoded: ColumnDefinition = serde_json::from_str(&json).unwrap();
        assert_eq!(decoded, column);
    }
}
