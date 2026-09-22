use crate::Result;
use crate::shape::TableShape;
use arrow::datatypes::SchemaRef;
use arrow_schema::DataType;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
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

/// A column of a directory Parquet table whose values come from the
/// `key=value` path segments of its files (the Hive layout), not from the
/// files themselves. A path value is text; it is read as `bigint`, `date`
/// or `varchar`, inferred from the values or declared by the table.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PartitionColumn {
    name: String,
    data_type: DataType,
}

impl PartitionColumn {
    /// The types a path value can carry. A dictionary over text is text:
    /// the reader hands text partitions out dictionary-encoded, and a table
    /// whose columns were inferred from such a scan declares them that way.
    pub fn new(name: impl Into<String>, data_type: DataType) -> Result<Self> {
        let name = name.into();
        validate_metadata_text("partition column name", &name)?;
        let Some(data_type) = Self::partition_type(&data_type) else {
            return Err(crate::KaveonError::Execution(format!(
                "partition column '{name}' cannot be {data_type}; a path value is read as \
                 bigint, date or varchar"
            )));
        };
        Ok(Self { name, data_type })
    }

    /// `data_type` as a partition column carries it, or `None` when no
    /// path value can be read as that type.
    pub fn partition_type(data_type: &DataType) -> Option<DataType> {
        match data_type {
            DataType::Dictionary(_, values) if **values == DataType::Utf8 => Some(DataType::Utf8),
            DataType::Int64 | DataType::Date32 | DataType::Utf8 => Some(data_type.clone()),
            _ => None,
        }
    }

    pub fn name(&self) -> &str {
        &self.name
    }
    pub const fn data_type(&self) -> &DataType {
        &self.data_type
    }
    /// Whether a column declared as `data_type` reads this partition
    /// column's values (a dictionary over text reads text).
    pub fn accepts(&self, data_type: &DataType) -> bool {
        Self::partition_type(data_type).as_ref() == Some(&self.data_type)
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

/// How a table's files are laid out when the Engine writes them: the
/// columns rows are clustered (sorted) by within every file, and the
/// columns that carry a Bloom filter per row group. `OPTIMIZE` rewrites a
/// table to this layout; a reader prunes by it. Both lists name columns of
/// the definition; the clustering columns always carry a Bloom filter.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TableLayout {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    clustered_by: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    bloom: Vec<String>,
}

impl TableLayout {
    pub fn new(clustered_by: Vec<String>, bloom: Vec<String>) -> Result<Self> {
        let mut seen = std::collections::HashSet::new();
        for column in &clustered_by {
            validate_metadata_text("clustering column", column)?;
            if !seen.insert(column.as_str()) {
                return Err(crate::KaveonError::Execution(format!(
                    "clustering column '{column}' is listed twice"
                )));
            }
        }
        seen.clear();
        for column in &bloom {
            validate_metadata_text("bloom column", column)?;
            if !seen.insert(column.as_str()) {
                return Err(crate::KaveonError::Execution(format!(
                    "bloom column '{column}' is listed twice"
                )));
            }
        }
        Ok(Self {
            clustered_by,
            bloom,
        })
    }

    /// The columns rows are sorted by, in order.
    pub fn clustered_by(&self) -> &[String] {
        &self.clustered_by
    }

    /// The columns declared to carry a Bloom filter, beyond the clustering
    /// columns.
    pub fn bloom(&self) -> &[String] {
        &self.bloom
    }

    /// Every column that carries a Bloom filter: the clustering columns
    /// first, then the declared ones not already listed.
    pub fn bloom_columns(&self) -> Vec<String> {
        let mut columns = self.clustered_by.clone();
        for column in &self.bloom {
            if !columns.contains(column) {
                columns.push(column.clone());
            }
        }
        columns
    }

    pub fn is_empty(&self) -> bool {
        self.clustered_by.is_empty() && self.bloom.is_empty()
    }

    /// Every named column must be one of `columns`.
    fn check_against(&self, columns: &[ColumnDefinition]) -> Result<()> {
        for (kind, names) in [("clustering", &self.clustered_by), ("bloom", &self.bloom)] {
            if let Some(unknown) = names
                .iter()
                .find(|name| !columns.iter().any(|column| column.name() == name.as_str()))
            {
                return Err(crate::KaveonError::Execution(format!(
                    "{kind} column '{unknown}' is not a column of the table"
                )));
            }
        }
        Ok(())
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
    /// The columns read from `key=value` path segments (a directory Parquet
    /// table): each names a column of `columns` and gives its type. Empty
    /// for every other table, and absent from older stored definitions.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    partitions: Vec<PartitionColumn>,
    lifecycle: CatalogLifecycle,
    /// Absent in definitions stored before layouts existed: no clustering,
    /// no Bloom filters.
    #[serde(default, skip_serializing_if = "TableLayout::is_empty")]
    layout: TableLayout,
    /// The declared shape the cube is built over; absent (no dimensions,
    /// no measures, no time) when none is declared, and in definitions
    /// stored before shapes existed.
    #[serde(default, skip_serializing_if = "TableShape::is_empty")]
    shape: TableShape,
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
            partitions: Vec::new(),
            lifecycle: CatalogLifecycle::Draft,
            layout: TableLayout::default(),
            shape: TableShape::default(),
        })
    }
    /// The definition with its partition columns declared: each must name
    /// one of the table's columns, with that column's type, and no column
    /// twice. Nothing else about the definition changes.
    pub fn partitioned_by(mut self, partitions: Vec<PartitionColumn>) -> Result<Self> {
        let mut seen = std::collections::HashSet::new();
        for partition in &partitions {
            if !seen.insert(partition.name()) {
                return Err(crate::KaveonError::Execution(format!(
                    "partition column '{}' is declared twice",
                    partition.name()
                )));
            }
            let Some(column) = self
                .columns
                .iter()
                .find(|column| column.name() == partition.name())
            else {
                return Err(crate::KaveonError::Execution(format!(
                    "partition column '{}' is not a column of table '{}'",
                    partition.name(),
                    self.name
                )));
            };
            if !partition.accepts(column.data_type()) {
                return Err(crate::KaveonError::Execution(format!(
                    "partition column '{}' is declared {} but column '{}' is {}",
                    partition.name(),
                    partition.data_type(),
                    column.name(),
                    column.data_type()
                )));
            }
        }
        self.partitions = partitions;
        Ok(self)
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
    /// The columns read from the files' paths, in path order; empty when
    /// the table is not a partitioned directory.
    pub fn partitions(&self) -> &[PartitionColumn] {
        &self.partitions
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
    /// The next revision of this definition at another location, the same
    /// lifecycle, columns, format and access: what `ALTER TABLE … SET
    /// LOCATION` publishes.
    pub fn with_location(&self, location: impl Into<String>) -> Result<Self> {
        let location = location.into();
        validate_metadata_text("table location", &location)?;
        let mut next = self.clone();
        next.location = location;
        next.revision = self.revision.next()?;
        Ok(next)
    }
    /// The layout the Engine writes this table in.
    pub const fn layout(&self) -> &TableLayout {
        &self.layout
    }
    /// This definition, at the same revision, with `layout`; every column
    /// the layout names must be a column of the table. What `CREATE TABLE
    /// … WITH (clustered_by = …)` stores.
    pub fn with_layout(mut self, layout: TableLayout) -> Result<Self> {
        layout.check_against(&self.columns)?;
        self.layout = layout;
        Ok(self)
    }
    /// The next revision of this definition with another layout: what
    /// `ALTER TABLE … SET CLUSTERED BY (…)` publishes.
    pub fn with_layout_revision(&self, layout: TableLayout) -> Result<Self> {
        layout.check_against(&self.columns)?;
        let mut next = self.clone();
        next.layout = layout;
        next.revision = self.revision.next()?;
        Ok(next)
    }
    /// The declared shape; empty when none is declared.
    pub const fn shape(&self) -> &TableShape {
        &self.shape
    }
    /// This definition, at the same revision, with `shape`; every column
    /// the shape names must be a column of the table with a type its role
    /// accepts. What `CREATE TABLE … WITH (dimensions = …)` stores.
    pub fn with_shape(mut self, shape: TableShape) -> Result<Self> {
        shape.check_against(&self.columns)?;
        self.shape = shape;
        Ok(self)
    }
    /// The next revision of this definition with another shape: what
    /// `ALTER TABLE … SET SHAPE (…)` and `DROP SHAPE` publish.
    pub fn with_shape_revision(&self, shape: TableShape) -> Result<Self> {
        shape.check_against(&self.columns)?;
        let mut next = self.clone();
        next.shape = shape;
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

/// The catalogs a session resolves names against.
///
/// The registered providers are shared: `restricted` returns a view over the
/// same providers that exposes only the catalogs it names, so a principal's
/// binder, planner and metadata calls all resolve against one object and an
/// invisible catalog is indistinguishable from an absent one (`catalog 'x'
/// not found`). A view never widens: restricting a view intersects.
#[derive(Clone)]
pub struct CatalogManager {
    catalogs: Arc<HashMap<String, Arc<dyn CatalogProvider>>>,
    default_catalog: String,
    default_schema: String,
    /// `None` exposes every registered catalog.
    visible: Option<Arc<HashSet<String>>>,
}

impl CatalogManager {
    pub fn new(default_catalog: impl Into<String>, default_schema: impl Into<String>) -> Self {
        Self {
            catalogs: Arc::new(HashMap::new()),
            default_catalog: default_catalog.into(),
            default_schema: default_schema.into(),
            visible: None,
        }
    }

    pub fn register_catalog(&mut self, catalog: Box<dyn CatalogProvider>) {
        let name = catalog.name().to_owned();
        Arc::make_mut(&mut self.catalogs).insert(name, Arc::from(catalog));
    }

    fn is_visible(&self, name: &str) -> bool {
        self.visible
            .as_ref()
            .is_none_or(|visible| visible.contains(name))
    }

    pub fn catalog(&self, name: &str) -> Option<&dyn CatalogProvider> {
        if !self.is_visible(name) {
            return None;
        }
        self.catalogs.get(name).map(|c| c.as_ref())
    }

    /// Every registered catalog, hidden ones included: the shape of the
    /// deployment, for the evaluator that decides what a view exposes.
    pub fn registered_catalog_names(&self) -> Vec<String> {
        self.catalogs.keys().cloned().collect()
    }

    pub fn catalog_names(&self) -> Vec<String> {
        self.catalogs
            .keys()
            .filter(|name| self.is_visible(name))
            .cloned()
            .collect()
    }

    /// This manager's providers, exposing only the catalogs in `visible`
    /// that this view already exposes.
    pub fn restricted(&self, visible: impl IntoIterator<Item = String>) -> Self {
        let visible: HashSet<String> = visible
            .into_iter()
            .filter(|name| self.is_visible(name))
            .collect();
        Self {
            catalogs: Arc::clone(&self.catalogs),
            default_catalog: self.default_catalog.clone(),
            default_schema: self.default_schema.clone(),
            visible: Some(Arc::new(visible)),
        }
    }

    /// Whether this manager is a restricted view.
    pub fn is_restricted(&self) -> bool {
        self.visible.is_some()
    }

    pub fn default_catalog(&self) -> &str {
        &self.default_catalog
    }

    pub fn default_schema(&self) -> &str {
        &self.default_schema
    }

    pub fn set_default(&mut self, catalog_name: &str, schema_name: &str) -> Result<()> {
        let catalog = self.catalog(catalog_name).ok_or_else(|| {
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

        let catalog = self.catalog(catalog_name).ok_or_else(|| {
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
                "abfss://{container}@{account}.dfs.core.windows.net/{}",
                [
                    root_path.trim_matches('/'),
                    self.table.location.trim_matches('/')
                ]
                .into_iter()
                .filter(|part| !part.is_empty())
                .collect::<Vec<_>>()
                .join("/")
            ),
            StorageType::S3 { bucket, prefix, .. } => {
                format!(
                    "s3://{bucket}/{}",
                    [
                        prefix.trim_matches('/'),
                        self.table.location.trim_matches('/')
                    ]
                    .into_iter()
                    .filter(|part| !part.is_empty())
                    .collect::<Vec<_>>()
                    .join("/")
                )
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
            "abfss://data@kaveonsa.dfs.core.windows.net/warehouse/events"
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

        let active = table.transition(CatalogLifecycle::Active).unwrap();
        let relocated = active.with_location("sales/orders_v2").unwrap();
        assert_eq!(relocated.location(), "sales/orders_v2");
        assert_eq!(relocated.revision().value(), 3);
        assert_eq!(relocated.lifecycle(), CatalogLifecycle::Active);
        assert_eq!(relocated.columns(), active.columns());
        assert!(active.with_location(" ").is_err());

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
    fn table_layout_names_columns_of_the_table_and_survives_older_documents() {
        let columns = vec![
            ColumnDefinition::new("order_id", DataType::Int64, false).unwrap(),
            ColumnDefinition::new("region", DataType::Utf8, true).unwrap(),
        ];
        let table = TableDefinition::new(
            TableId::new("table-01").unwrap(),
            SchemaId::new("schema-01").unwrap(),
            "orders",
            "sales/orders",
            AccessPattern::Shortcut,
            DataFormat::Parquet,
            columns,
        )
        .unwrap();
        assert!(table.layout().is_empty());

        let layout = TableLayout::new(vec!["region".into()], vec!["order_id".into()]).unwrap();
        let clustered = table.clone().with_layout(layout.clone()).unwrap();
        assert_eq!(clustered.revision(), table.revision());
        assert_eq!(clustered.layout().clustered_by(), ["region"]);
        assert_eq!(clustered.layout().bloom_columns(), ["region", "order_id"]);
        let again = clustered
            .with_layout_revision(TableLayout::new(vec!["order_id".into()], vec![]).unwrap())
            .unwrap();
        assert_eq!(again.revision().value(), 2);
        assert_eq!(again.layout().clustered_by(), ["order_id"]);

        assert!(
            table
                .clone()
                .with_layout(TableLayout::new(vec!["missing".into()], vec![]).unwrap())
                .is_err()
        );
        assert!(TableLayout::new(vec!["a".into(), "a".into()], vec![]).is_err());
        assert!(TableLayout::new(vec![], vec![" ".into()]).is_err());

        // A definition stored before layouts existed carries no `layout`
        // key; a clustered one round-trips.
        let mut document = serde_json::to_value(&table).unwrap();
        assert!(document.get("layout").is_none());
        document.as_object_mut().unwrap().remove("layout");
        let restored: TableDefinition = serde_json::from_value(document).unwrap();
        assert_eq!(restored, table);
        let restored: TableDefinition =
            serde_json::from_str(&serde_json::to_string(&clustered).unwrap()).unwrap();
        assert_eq!(restored, clustered);
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

    #[test]
    fn a_restricted_view_hides_catalogs_without_disclosing_them() {
        let storage = StorageType::Local {
            base_path: PathBuf::from("/data"),
        };
        let mut open = MemoryCatalog::new("OpenSource", storage.clone()).with_schema("public");
        open.register_table(
            "public",
            TableMeta {
                name: "events".into(),
                arrow_schema: test_schema(),
                location: "events.parquet".into(),
                access: AccessPattern::Shortcut,
                format: DataFormat::Parquet,
            },
        )
        .unwrap();
        let mut kaveon = MemoryCatalog::new("Kaveon", storage).with_schema("usage");
        kaveon
            .register_table(
                "usage",
                TableMeta {
                    name: "sessions".into(),
                    arrow_schema: test_schema(),
                    location: "sessions.parquet".into(),
                    access: AccessPattern::Shortcut,
                    format: DataFormat::Parquet,
                },
            )
            .unwrap();
        let mut full = CatalogManager::new("OpenSource", "public");
        full.register_catalog(Box::new(open));
        full.register_catalog(Box::new(kaveon));
        assert!(!full.is_restricted());

        let mut view = full.restricted(["OpenSource".to_owned(), "Missing".to_owned()]);
        assert!(view.is_restricted());
        assert_eq!(view.catalog_names(), vec!["OpenSource".to_owned()]);
        let mut registered = view.registered_catalog_names();
        registered.sort();
        assert_eq!(registered, vec!["Kaveon".to_owned(), "OpenSource".into()]);
        assert!(view.catalog("Kaveon").is_none());
        assert!(view.catalog("OpenSource").is_some());
        // The hidden catalog and a catalog that does not exist fail alike.
        let hidden = view
            .resolve_table(&TableReference::parse("Kaveon.usage.sessions"))
            .unwrap_err()
            .to_string();
        let absent = view
            .resolve_table(&TableReference::parse("Nowhere.usage.sessions"))
            .unwrap_err()
            .to_string();
        assert_eq!(
            hidden.replace("Kaveon", "X"),
            absent.replace("Nowhere", "X")
        );
        assert!(view.set_default("Kaveon", "usage").is_err());
        assert!(
            view.resolve_table(&TableReference::parse("OpenSource.public.events"))
                .is_ok()
        );
        // Restricting a view intersects; it never widens.
        let widened = view.restricted(["Kaveon".to_owned(), "OpenSource".into()]);
        assert_eq!(widened.catalog_names(), vec!["OpenSource".to_owned()]);
        let none = view.restricted(Vec::<String>::new());
        assert!(none.catalog_names().is_empty());
        assert!(none.catalog("OpenSource").is_none());
        // The full manager is untouched by its views.
        assert_eq!(full.catalog_names().len(), 2);
        assert!(full.catalog("Kaveon").is_some());
    }
}
