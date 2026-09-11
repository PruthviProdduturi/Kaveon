//! Immutable, storage-agnostic product catalog snapshots.
//!
//! This module prepares a complete next snapshot in memory. Persisting it with
//! conditional ADLS writes is deliberately the caller's responsibility. Only
//! the current snapshot's operation is retained, so durable snapshot history
//! is required to deduplicate an older or otherwise ambiguous operation ID.

use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, BTreeSet},
    error::Error,
    fmt,
};

pub const SNAPSHOT_FORMAT_VERSION: u32 = 1;
pub const MAX_TABLES: usize = 1_000;
pub const MAX_CONTROL_RECORDS: usize = 10_000;
pub const MAX_PRODUCT_RECORDS: usize = 100_000;
pub const MAX_REFERENCES_PER_PRODUCT_RECORD: usize = 100;
pub const MAX_UNIQUE_VALUES_PER_PRODUCT_RECORD: usize = 32;
pub const MAX_PRODUCT_PAGE_SIZE: usize = 100;
pub const MAX_CHANGES: usize = 100;
pub const MAX_PARQUET_FILES_PER_TABLE: usize = 10_000;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SnapshotRef {
    pub generation: u64,
    pub snapshot_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ImmutableFileRef {
    /// A normalized path relative to the catalog root.
    pub path: String,
    /// Lowercase SHA-256 digest of the immutable object.
    pub sha256: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TableManifestRef {
    /// The immutable table manifest, which describes the table schema and files.
    pub manifest: ImmutableFileRef,
    /// Immutable Parquet objects referenced by that manifest.
    pub parquet_files: Vec<ImmutableFileRef>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TableStatisticsRef {
    /// Immutable catalog-definition snapshot used to resolve the runtime table.
    pub catalog_snapshot_sha256: String,
    /// Digest returned by storage for the exact analyzed source state.
    pub source_identity_sha256: String,
    /// Immutable detailed statistics document for future column estimates.
    pub document: ImmutableFileRef,
    /// Exact physical rows in the pinned immutable table snapshot.
    pub row_count: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RuntimeTableSourceRef {
    pub catalog_snapshot_sha256: String,
    pub source_identity_sha256: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CatalogSnapshot {
    pub version: u32,
    pub generation: u64,
    pub snapshot_id: String,
    pub parent: Option<SnapshotRef>,
    pub operation_id: String,
    pub request_digest: String,
    pub tables: BTreeMap<String, TableManifestRef>,
    #[serde(default)]
    pub runtime_table_sources: BTreeMap<String, RuntimeTableSourceRef>,
    #[serde(default)]
    pub table_statistics: BTreeMap<String, TableStatisticsRef>,
    /// Immutable product-control documents (datasets, charts, dashboards, and
    /// similar metadata) published under the same atomic snapshot head.
    #[serde(default)]
    pub control_records: BTreeMap<String, ImmutableFileRef>,
    #[serde(default)]
    pub product_records: BTreeMap<String, ProductRecordRef>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum CatalogChange {
    Put {
        table: String,
        reference: TableManifestRef,
    },
    Delete {
        table: String,
    },
    PutRuntimeTableSource {
        table: String,
        source: RuntimeTableSourceRef,
    },
    PutStatistics {
        table: String,
        statistics: TableStatisticsRef,
    },
    DeleteStatistics {
        table: String,
    },
    PutControl {
        key: String,
        reference: ImmutableFileRef,
    },
    DeleteControl {
        key: String,
    },
    CreateProduct {
        record: ProductRecordRef,
    },
    UpdateProduct {
        expected_revision: u64,
        record: ProductRecordRef,
    },
    DeleteProduct {
        kind: ProductRecordKind,
        id: String,
        expected_revision: u64,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PrepareChange {
    /// The exact immutable snapshot the caller read before preparing this change.
    pub base: SnapshotRef,
    pub snapshot_id: String,
    pub operation_id: String,
    /// SHA-256 of a trusted canonical request representation. Callers must
    /// bind this digest to the authenticated operation before publication.
    pub request_digest: String,
    pub changes: Vec<CatalogChange>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ManifestError(String);

impl fmt::Display for ManifestError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl Error for ManifestError {}

impl CatalogSnapshot {
    pub fn empty(snapshot_id: impl Into<String>) -> Result<Self, ManifestError> {
        let snapshot_id = snapshot_id.into();
        validate_snapshot_id("snapshot ID", &snapshot_id)?;
        Ok(Self {
            version: SNAPSHOT_FORMAT_VERSION,
            generation: 0,
            snapshot_id,
            parent: None,
            operation_id: "genesis".to_owned(),
            request_digest: "0".repeat(64),
            tables: BTreeMap::new(),
            runtime_table_sources: BTreeMap::new(),
            table_statistics: BTreeMap::new(),
            control_records: BTreeMap::new(),
            product_records: BTreeMap::new(),
        })
    }

    pub fn reference(&self) -> SnapshotRef {
        SnapshotRef {
            generation: self.generation,
            snapshot_id: self.snapshot_id.clone(),
        }
    }

    pub fn product_record(
        &self,
        kind: ProductRecordKind,
        id: &str,
    ) -> Result<Option<&ProductRecordRef>, ProductReadError> {
        validate_product_id(id).map_err(read_error)?;
        Ok(self.product_records.get(&product_record_key(kind, id)))
    }

    /// Exact byte-sensitive equality over a snapshot-validated unique index.
    pub fn product_record_by_unique_value(
        &self,
        kind: ProductRecordKind,
        index: &str,
        value: &str,
    ) -> Result<Option<&ProductRecordRef>, ProductReadError> {
        validate_unique_value(index, value).map_err(read_error)?;
        Ok(self.product_records.values().find(|record| {
            record.kind == kind
                && record
                    .unique_values
                    .get(index)
                    .is_some_and(|candidate| candidate == value)
        }))
    }

    /// Bytewise ID pagination bound to this immutable snapshot reference.
    pub fn product_records_page(
        &self,
        kind: ProductRecordKind,
        limit: usize,
        cursor: Option<&str>,
    ) -> Result<ProductRecordPage, ProductReadError> {
        if limit == 0 || limit > MAX_PRODUCT_PAGE_SIZE {
            return Err(ProductReadError(format!(
                "product page size must be between 1 and {MAX_PRODUCT_PAGE_SIZE}"
            )));
        }
        let after = cursor
            .map(|value| decode_product_cursor(value, &self.reference(), kind))
            .transpose()?;
        let prefix = format!("{}/", kind.name());
        let mut matching = self
            .product_records
            .range(prefix.clone()..)
            .take_while(|(key, _)| key.starts_with(&prefix))
            .filter(|(_, record)| {
                after
                    .as_ref()
                    .is_none_or(|id| record.id.as_bytes() > id.as_bytes())
            })
            .map(|(_, record)| record.clone());
        let records = matching.by_ref().take(limit).collect::<Vec<_>>();
        let has_more = matching.next().is_some();
        let next_cursor = has_more
            .then(|| records.last())
            .flatten()
            .map(|record| encode_product_cursor(&self.reference(), kind, &record.id));
        Ok(ProductRecordPage {
            snapshot: self.reference(),
            records,
            next_cursor,
        })
    }

    /// Prepares an all-or-nothing next snapshot. The caller must make storage
    /// publication conditional on `request.base`; this function performs no I/O.
    pub fn prepare(&self, request: PrepareChange) -> Result<Self, ManifestError> {
        self.validate()?;
        validate_request(&request)?;

        if request.operation_id == self.operation_id {
            if request.request_digest != self.request_digest {
                return Err(error(
                    "operation ID was already used with a different request digest",
                ));
            }
            if self.parent.as_ref() == Some(&request.base)
                && request.snapshot_id == self.snapshot_id
            {
                return Ok(self.clone());
            }
            return Err(error("operation ID replay does not describe this snapshot"));
        }
        if request.base != self.reference() {
            return Err(error("stale base snapshot generation or ID"));
        }

        let mut tables = self.tables.clone();
        let mut runtime_table_sources = self.runtime_table_sources.clone();
        let mut table_statistics = self.table_statistics.clone();
        let mut control_records = self.control_records.clone();
        let mut product_records = self.product_records.clone();
        for change in request.changes {
            match change {
                CatalogChange::Put { table, reference } => {
                    table_statistics.remove(&table);
                    tables.insert(table, reference);
                }
                CatalogChange::Delete { table } => {
                    if tables.remove(&table).is_none() {
                        return Err(error(format!("cannot delete absent table '{table}'")));
                    }
                    table_statistics.remove(&table);
                }
                CatalogChange::PutStatistics { table, statistics } => {
                    table_statistics.insert(table, statistics);
                }
                CatalogChange::PutRuntimeTableSource { table, source } => {
                    runtime_table_sources.insert(table, source);
                }
                CatalogChange::DeleteStatistics { table } => {
                    if table_statistics.remove(&table).is_none() {
                        return Err(error(format!(
                            "cannot delete absent statistics for '{table}'"
                        )));
                    }
                }
                CatalogChange::PutControl { key, reference } => {
                    control_records.insert(key, reference);
                }
                CatalogChange::DeleteControl { key } => {
                    if control_records.remove(&key).is_none() {
                        return Err(error(format!(
                            "cannot delete absent control record '{key}'"
                        )));
                    }
                }
                CatalogChange::CreateProduct { record } => {
                    let key = record.key();
                    if product_records.contains_key(&key) {
                        return Err(error(format!("product record '{key}' already exists")));
                    }
                    if record.revision != 1 {
                        return Err(error("new product record revision must be one"));
                    }
                    product_records.insert(key, record);
                }
                CatalogChange::UpdateProduct {
                    expected_revision,
                    record,
                } => {
                    let key = record.key();
                    let Some(current) = product_records.get(&key) else {
                        return Err(error(format!("product record '{key}' does not exist")));
                    };
                    if current.revision != expected_revision {
                        return Err(error(format!("stale product record revision for '{key}'")));
                    }
                    let next_revision = expected_revision.checked_add(1).ok_or_else(|| {
                        error(format!("product record revision overflow for '{key}'"))
                    })?;
                    if record.revision != next_revision {
                        return Err(error(format!(
                            "product record '{key}' revision must advance by one"
                        )));
                    }
                    product_records.insert(key, record);
                }
                CatalogChange::DeleteProduct {
                    kind,
                    id,
                    expected_revision,
                } => {
                    let key = product_record_key(kind, &id);
                    let Some(current) = product_records.get(&key) else {
                        return Err(error(format!("product record '{key}' does not exist")));
                    };
                    if current.revision != expected_revision {
                        return Err(error(format!("stale product record revision for '{key}'")));
                    }
                    product_records.remove(&key);
                }
            }
        }
        if tables.len() > MAX_TABLES {
            return Err(error("snapshot table limit exceeded"));
        }
        if control_records.len() > MAX_CONTROL_RECORDS {
            return Err(error("snapshot control-record limit exceeded"));
        }
        validate_product_records(&product_records)?;
        let generation = self
            .generation
            .checked_add(1)
            .ok_or_else(|| error("generation overflow"))?;
        let next = Self {
            version: SNAPSHOT_FORMAT_VERSION,
            generation,
            snapshot_id: request.snapshot_id,
            parent: Some(self.reference()),
            operation_id: request.operation_id,
            request_digest: request.request_digest,
            tables,
            runtime_table_sources,
            table_statistics,
            control_records,
            product_records,
        };
        next.validate()?;
        Ok(next)
    }

    pub fn validate(&self) -> Result<(), ManifestError> {
        if self.version != SNAPSHOT_FORMAT_VERSION {
            return Err(error("unsupported snapshot format version"));
        }
        validate_snapshot_id("snapshot ID", &self.snapshot_id)?;
        validate_identifier("operation ID", &self.operation_id)?;
        validate_digest(&self.request_digest)?;
        if self.tables.len() > MAX_TABLES {
            return Err(error("snapshot table limit exceeded"));
        }
        if self.control_records.len() > MAX_CONTROL_RECORDS {
            return Err(error("snapshot control-record limit exceeded"));
        }
        validate_product_records(&self.product_records)?;
        for (table, reference) in &self.tables {
            validate_table_name(table)?;
            validate_table_reference(reference)?;
        }
        for (table, statistics) in &self.table_statistics {
            validate_table_name(table)?;
            validate_statistics(statistics)?;
            let source = self.runtime_table_sources.get(table).ok_or_else(|| {
                error(format!(
                    "statistics reference absent runtime table '{table}'"
                ))
            })?;
            if source.catalog_snapshot_sha256 != statistics.catalog_snapshot_sha256
                || source.source_identity_sha256 != statistics.source_identity_sha256
            {
                return Err(error("statistics do not match the runtime table source"));
            }
        }
        for (table, source) in &self.runtime_table_sources {
            validate_table_name(table)?;
            validate_digest(&source.catalog_snapshot_sha256)?;
            validate_digest(&source.source_identity_sha256)?;
        }
        for (key, reference) in &self.control_records {
            validate_control_key(key)?;
            validate_file(reference, false)?;
        }
        if let Some(parent) = &self.parent {
            validate_snapshot_id("parent snapshot ID", &parent.snapshot_id)?;
            if parent.generation >= self.generation {
                return Err(error("parent generation must precede snapshot generation"));
            }
        } else if self.generation != 0 {
            return Err(error("non-genesis snapshot requires a parent"));
        }
        Ok(())
    }
}

fn validate_request(request: &PrepareChange) -> Result<(), ManifestError> {
    validate_snapshot_id("base snapshot ID", &request.base.snapshot_id)?;
    validate_snapshot_id("snapshot ID", &request.snapshot_id)?;
    validate_identifier("operation ID", &request.operation_id)?;
    validate_digest(&request.request_digest)?;
    if request.changes.is_empty() || request.changes.len() > MAX_CHANGES {
        return Err(error(
            "change count must be between 1 and the configured limit",
        ));
    }
    let mut table_names = BTreeSet::new();
    let mut source_names = BTreeSet::new();
    let mut statistics_names = BTreeSet::new();
    let mut control_keys = BTreeSet::new();
    let mut product_keys = BTreeSet::new();
    for change in &request.changes {
        match change {
            CatalogChange::Put { table, reference } => {
                validate_table_reference(reference)?;
                validate_table_name(table)?;
                if !table_names.insert(table) {
                    return Err(error(format!("table '{table}' is changed more than once")));
                }
            }
            CatalogChange::Delete { table } => {
                validate_table_name(table)?;
                if !table_names.insert(table) {
                    return Err(error(format!("table '{table}' is changed more than once")));
                }
            }
            CatalogChange::PutStatistics { table, statistics } => {
                validate_table_name(table)?;
                validate_statistics(statistics)?;
                if !statistics_names.insert(table) {
                    return Err(error(format!(
                        "statistics for '{table}' are changed more than once"
                    )));
                }
            }
            CatalogChange::PutRuntimeTableSource { table, source } => {
                validate_table_name(table)?;
                validate_digest(&source.catalog_snapshot_sha256)?;
                validate_digest(&source.source_identity_sha256)?;
                if !source_names.insert(table) {
                    return Err(error(format!(
                        "runtime source for '{table}' is changed more than once"
                    )));
                }
            }
            CatalogChange::DeleteStatistics { table } => {
                validate_table_name(table)?;
                if !statistics_names.insert(table) {
                    return Err(error(format!(
                        "statistics for '{table}' are changed more than once"
                    )));
                }
            }
            CatalogChange::PutControl { key, reference } => {
                validate_control_key(key)?;
                validate_file(reference, false)?;
                if !control_keys.insert(key) {
                    return Err(error(format!(
                        "control record '{key}' is changed more than once"
                    )));
                }
            }
            CatalogChange::DeleteControl { key } => {
                validate_control_key(key)?;
                if !control_keys.insert(key) {
                    return Err(error(format!(
                        "control record '{key}' is changed more than once"
                    )));
                }
            }
            CatalogChange::CreateProduct { record }
            | CatalogChange::UpdateProduct { record, .. } => {
                validate_product_record(record)?;
                let key = record.key();
                if !product_keys.insert(key.clone()) {
                    return Err(error(format!(
                        "product record '{key}' is changed more than once"
                    )));
                }
            }
            CatalogChange::DeleteProduct { kind, id, .. } => {
                validate_product_id(id)?;
                let key = product_record_key(*kind, id);
                if !product_keys.insert(key.clone()) {
                    return Err(error(format!(
                        "product record '{key}' is changed more than once"
                    )));
                }
            }
        }
    }
    Ok(())
}

fn validate_table_reference(reference: &TableManifestRef) -> Result<(), ManifestError> {
    validate_file(&reference.manifest, false)?;
    if reference.parquet_files.len() > MAX_PARQUET_FILES_PER_TABLE {
        return Err(error("Parquet file limit exceeded"));
    }
    let mut paths = BTreeSet::new();
    for file in &reference.parquet_files {
        validate_file(file, true)?;
        if !paths.insert(&file.path) {
            return Err(error("table manifest has duplicate Parquet paths"));
        }
    }
    Ok(())
}

fn validate_statistics(statistics: &TableStatisticsRef) -> Result<(), ManifestError> {
    validate_digest(&statistics.catalog_snapshot_sha256)?;
    validate_digest(&statistics.source_identity_sha256)?;
    validate_file(&statistics.document, false)?;
    Ok(())
}

fn validate_file(file: &ImmutableFileRef, parquet: bool) -> Result<(), ManifestError> {
    validate_relative_path(&file.path)?;
    if parquet && !file.path.ends_with(".parquet") {
        return Err(error("Parquet reference must end in .parquet"));
    }
    validate_digest(&file.sha256)
}

fn validate_relative_path(path: &str) -> Result<(), ManifestError> {
    if path.is_empty()
        || path.len() > 1_024
        || path.starts_with('/')
        || path.contains('\\')
        || path.contains(':')
    {
        return Err(error("path must be a normalized relative path"));
    }
    for segment in path.split('/') {
        if segment.is_empty()
            || segment == "."
            || segment == ".."
            || segment.chars().any(char::is_control)
        {
            return Err(error("path must be a normalized relative path"));
        }
    }
    Ok(())
}

fn validate_table_name(name: &str) -> Result<(), ManifestError> {
    if name.is_empty()
        || name.len() > 255
        || name.starts_with('.')
        || name.ends_with('.')
        || name.contains("..")
    {
        return Err(error("table name is invalid"));
    }
    for segment in name.split('.') {
        let mut chars = segment.chars();
        if !matches!(chars.next(), Some(character) if character.is_ascii_alphabetic() || character == '_')
            || !chars.all(|character| character.is_ascii_alphanumeric() || character == '_')
        {
            return Err(error("table name is invalid"));
        }
    }
    Ok(())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProductRecordKind {
    Dataset,
    Chart,
    Dashboard,
    SavedQuery,
    UserTheme,
    DlmDefinition,
    DlmRun,
}

impl ProductRecordKind {
    fn name(self) -> &'static str {
        match self {
            Self::Dataset => "dataset",
            Self::Chart => "chart",
            Self::Dashboard => "dashboard",
            Self::SavedQuery => "saved_query",
            Self::UserTheme => "user_theme",
            Self::DlmDefinition => "dlm_definition",
            Self::DlmRun => "dlm_run",
        }
    }

    fn from_name(value: &str) -> Option<Self> {
        match value {
            "dataset" => Some(Self::Dataset),
            "chart" => Some(Self::Chart),
            "dashboard" => Some(Self::Dashboard),
            "saved_query" => Some(Self::SavedQuery),
            "user_theme" => Some(Self::UserTheme),
            "dlm_definition" => Some(Self::DlmDefinition),
            "dlm_run" => Some(Self::DlmRun),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProductRecordRef {
    pub kind: ProductRecordKind,
    pub id: String,
    pub revision: u64,
    pub document: ImmutableFileRef,
    #[serde(default)]
    pub unique_values: BTreeMap<String, String>,
    /// Typed foreign-key-like references resolved against the final snapshot.
    /// Deletion uses restrict semantics; this layer never cascades implicitly.
    #[serde(default)]
    pub references: BTreeSet<ProductRecordReference>,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct ProductRecordReference {
    pub kind: ProductRecordKind,
    pub id: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProductRecordPage {
    pub snapshot: SnapshotRef,
    pub records: Vec<ProductRecordRef>,
    pub next_cursor: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProductReadError(String);

impl fmt::Display for ProductReadError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl Error for ProductReadError {}

impl ProductRecordRef {
    fn key(&self) -> String {
        product_record_key(self.kind, &self.id)
    }
}

fn validate_control_key(key: &str) -> Result<(), ManifestError> {
    if key.is_empty()
        || key.len() > 512
        || key.starts_with('.')
        || key.ends_with('.')
        || key.contains("..")
        || key.chars().any(char::is_control)
        || !key.chars().all(|character| {
            character.is_ascii_alphanumeric() || matches!(character, '_' | '-' | '.')
        })
    {
        return Err(error("control-record key is invalid"));
    }
    Ok(())
}

fn product_record_key(kind: ProductRecordKind, id: &str) -> String {
    format!("{}/{}", kind.name(), id)
}

fn validate_product_id(id: &str) -> Result<(), ManifestError> {
    validate_identifier("product record ID", id)?;
    if id.contains(['/', '\\']) {
        return Err(error("product record ID is invalid"));
    }
    Ok(())
}

fn validate_product_record(record: &ProductRecordRef) -> Result<(), ManifestError> {
    validate_product_id(&record.id)?;
    if record.revision == 0 {
        return Err(error("product record revision must be positive"));
    }
    validate_file(&record.document, false)?;
    if record.unique_values.len() > MAX_UNIQUE_VALUES_PER_PRODUCT_RECORD {
        return Err(error("product record unique-value limit exceeded"));
    }
    for (name, value) in &record.unique_values {
        validate_unique_value(name, value)?;
    }
    if record.references.len() > MAX_REFERENCES_PER_PRODUCT_RECORD {
        return Err(error("product record reference limit exceeded"));
    }
    for reference in &record.references {
        validate_product_id(&reference.id)?;
        if reference.kind == record.kind && reference.id == record.id {
            return Err(error("product record cannot reference itself"));
        }
        let allowed = match record.kind {
            ProductRecordKind::Chart => reference.kind == ProductRecordKind::Dataset,
            ProductRecordKind::Dashboard => matches!(
                reference.kind,
                ProductRecordKind::Chart | ProductRecordKind::Dataset
            ),
            ProductRecordKind::Dataset
            | ProductRecordKind::SavedQuery
            | ProductRecordKind::UserTheme => false,
            ProductRecordKind::DlmDefinition => reference.kind == ProductRecordKind::Dataset,
            ProductRecordKind::DlmRun => reference.kind == ProductRecordKind::DlmDefinition,
        };
        if !allowed {
            return Err(error(format!(
                "{} cannot reference {}",
                record.kind.name(),
                reference.kind.name()
            )));
        }
    }
    Ok(())
}

fn validate_unique_value(name: &str, value: &str) -> Result<(), ManifestError> {
    validate_identifier("unique index name", name)?;
    if !name
        .chars()
        .all(|character| character.is_ascii_alphanumeric() || character == '_')
        || value.is_empty()
        || value.len() > 512
        || value.chars().any(char::is_control)
    {
        return Err(error("product record unique value is invalid"));
    }
    Ok(())
}

fn encode_product_cursor(snapshot: &SnapshotRef, kind: ProductRecordKind, id: &str) -> String {
    let encoded_id = id
        .as_bytes()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    format!(
        "v1:{}:{}:{}:{encoded_id}",
        snapshot.generation,
        snapshot.snapshot_id,
        kind.name()
    )
}

fn decode_product_cursor(
    cursor: &str,
    snapshot: &SnapshotRef,
    expected_kind: ProductRecordKind,
) -> Result<String, ProductReadError> {
    let parts = cursor.split(':').collect::<Vec<_>>();
    if parts.len() != 5 || parts[0] != "v1" {
        return Err(ProductReadError("malformed product cursor".into()));
    }
    let generation = parts[1]
        .parse::<u64>()
        .map_err(|_| ProductReadError("malformed product cursor".into()))?;
    let kind = ProductRecordKind::from_name(parts[3])
        .ok_or_else(|| ProductReadError("malformed product cursor".into()))?;
    if generation != snapshot.generation || parts[2] != snapshot.snapshot_id {
        return Err(ProductReadError(
            "product cursor belongs to a different snapshot".into(),
        ));
    }
    if kind != expected_kind {
        return Err(ProductReadError(
            "product cursor belongs to a different record kind".into(),
        ));
    }
    let encoded = parts[4].as_bytes();
    if encoded.is_empty() || encoded.len() % 2 != 0 {
        return Err(ProductReadError("malformed product cursor".into()));
    }
    let (pairs, _) = encoded.as_chunks::<2>();
    let bytes = pairs
        .iter()
        .map(|pair| {
            std::str::from_utf8(pair)
                .ok()
                .and_then(|hex| u8::from_str_radix(hex, 16).ok())
                .ok_or_else(|| ProductReadError("malformed product cursor".into()))
        })
        .collect::<Result<Vec<_>, _>>()?;
    let id = String::from_utf8(bytes)
        .map_err(|_| ProductReadError("malformed product cursor".into()))?;
    validate_product_id(&id).map_err(read_error)?;
    Ok(id)
}

fn read_error(error: ManifestError) -> ProductReadError {
    ProductReadError(error.to_string())
}

fn validate_product_records(
    records: &BTreeMap<String, ProductRecordRef>,
) -> Result<(), ManifestError> {
    if records.len() > MAX_PRODUCT_RECORDS {
        return Err(error("snapshot product-record limit exceeded"));
    }
    let mut unique = BTreeSet::new();
    for (key, record) in records {
        validate_product_record(record)?;
        if key != &record.key() {
            return Err(error("product record map key does not match its identity"));
        }
        for (index, value) in &record.unique_values {
            if !unique.insert((record.kind, index.as_str(), value.as_str())) {
                return Err(error(format!(
                    "duplicate {} unique index '{index}' value",
                    record.kind.name()
                )));
            }
        }
        for reference in &record.references {
            let target = product_record_key(reference.kind, &reference.id);
            if !records.contains_key(&target) {
                return Err(error(format!(
                    "product record '{key}' references missing '{target}'"
                )));
            }
        }
    }
    Ok(())
}

fn validate_identifier(kind: &str, value: &str) -> Result<(), ManifestError> {
    if value.is_empty() || value.len() > 128 || value.chars().any(char::is_control) {
        return Err(error(format!("{kind} is invalid")));
    }
    Ok(())
}

fn validate_snapshot_id(kind: &str, value: &str) -> Result<(), ManifestError> {
    validate_identifier(kind, value)?;
    if value.contains('/')
        || value.contains('\\')
        || value.contains(':')
        || value == "."
        || value == ".."
    {
        return Err(error(format!("{kind} is invalid")));
    }
    Ok(())
}

fn validate_digest(value: &str) -> Result<(), ManifestError> {
    if value.len() != 64
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return Err(error("digest must be a lowercase SHA-256 hex value"));
    }
    Ok(())
}

fn error(message: impl Into<String>) -> ManifestError {
    ManifestError(message.into())
}

#[cfg(test)]
mod tests {
    use super::*;

    const DIGEST: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";

    fn table(path: &str) -> TableManifestRef {
        TableManifestRef {
            manifest: ImmutableFileRef {
                path: format!("tables/{path}/manifest.json"),
                sha256: DIGEST.into(),
            },
            parquet_files: vec![ImmutableFileRef {
                path: format!("tables/{path}/part-000.parquet"),
                sha256: DIGEST.into(),
            }],
        }
    }
    fn control(path: &str) -> ImmutableFileRef {
        ImmutableFileRef {
            path: format!("control/{path}.json"),
            sha256: DIGEST.into(),
        }
    }
    fn product(
        kind: ProductRecordKind,
        id: &str,
        revision: u64,
        unique_name: &str,
    ) -> ProductRecordRef {
        ProductRecordRef {
            kind,
            id: id.into(),
            revision,
            document: control(&format!("{id}-{revision}")),
            unique_values: BTreeMap::from([("owner_name".into(), unique_name.into())]),
            references: BTreeSet::new(),
        }
    }
    fn reference(kind: ProductRecordKind, id: &str) -> ProductRecordReference {
        ProductRecordReference {
            kind,
            id: id.into(),
        }
    }
    fn change(
        base: SnapshotRef,
        id: &str,
        digest: &str,
        changes: Vec<CatalogChange>,
    ) -> PrepareChange {
        PrepareChange {
            base,
            snapshot_id: format!("snapshot-{id}"),
            operation_id: id.into(),
            request_digest: digest.into(),
            changes,
        }
    }

    #[test]
    fn prepares_multiple_tables_together() {
        let base = CatalogSnapshot::empty("snapshot-genesis").unwrap();
        let next = base
            .prepare(change(
                base.reference(),
                "op-1",
                DIGEST,
                vec![
                    CatalogChange::Put {
                        table: "bronze.orders".into(),
                        reference: table("orders"),
                    },
                    CatalogChange::Put {
                        table: "silver.customers".into(),
                        reference: table("customers"),
                    },
                ],
            ))
            .unwrap();
        assert_eq!(next.generation, 1);
        assert_eq!(next.parent, Some(base.reference()));
        assert_eq!(next.tables.len(), 2);
    }

    #[test]
    fn publishes_table_and_control_records_in_one_snapshot() {
        let base = CatalogSnapshot::empty("snapshot-genesis").unwrap();
        let next = base
            .prepare(change(
                base.reference(),
                "op-product-migration",
                DIGEST,
                vec![
                    CatalogChange::Put {
                        table: "kaveon.system_events".into(),
                        reference: table("system_events"),
                    },
                    CatalogChange::PutControl {
                        key: "datasets.550e8400-e29b-41d4-a716-446655440000".into(),
                        reference: control("dataset-1"),
                    },
                    CatalogChange::PutControl {
                        key: "dashboards.executive-overview".into(),
                        reference: control("dashboard-1"),
                    },
                ],
            ))
            .unwrap();

        assert_eq!(next.tables.len(), 1);
        assert_eq!(next.control_records.len(), 2);
        assert!(base.tables.is_empty());
        assert!(base.control_records.is_empty());
    }

    #[test]
    fn invalid_control_change_aborts_the_whole_preparation() {
        let base = CatalogSnapshot::empty("snapshot-genesis").unwrap();
        let result = base.prepare(change(
            base.reference(),
            "op-invalid-control",
            DIGEST,
            vec![
                CatalogChange::Put {
                    table: "kaveon.system_events".into(),
                    reference: table("system_events"),
                },
                CatalogChange::PutControl {
                    key: "../credentials".into(),
                    reference: control("invalid"),
                },
            ],
        ));

        assert!(result.is_err());
        assert!(base.tables.is_empty());
        assert!(base.control_records.is_empty());
    }

    #[test]
    fn snapshots_without_control_records_remain_readable() {
        let legacy = serde_json::json!({
            "version": SNAPSHOT_FORMAT_VERSION,
            "generation": 0,
            "snapshot_id": "snapshot-genesis",
            "parent": null,
            "operation_id": "genesis",
            "request_digest": "0".repeat(64),
            "tables": {}
        });
        let decoded: CatalogSnapshot = serde_json::from_value(legacy).unwrap();

        assert!(decoded.control_records.is_empty());
        assert!(decoded.product_records.is_empty());
        decoded.validate().unwrap();
    }

    #[test]
    fn typed_product_crud_enforces_revisions() {
        let base = CatalogSnapshot::empty("snapshot-genesis").unwrap();
        let created = base
            .prepare(change(
                base.reference(),
                "create-dashboard",
                DIGEST,
                vec![CatalogChange::CreateProduct {
                    record: product(ProductRecordKind::Dashboard, "dash-1", 1, "alice/home"),
                }],
            ))
            .unwrap();
        let stale = created.prepare(change(
            created.reference(),
            "stale-dashboard",
            DIGEST,
            vec![CatalogChange::UpdateProduct {
                expected_revision: 0,
                record: product(ProductRecordKind::Dashboard, "dash-1", 1, "alice/new"),
            }],
        ));
        assert!(stale.unwrap_err().to_string().contains("stale"));

        let updated = created
            .prepare(change(
                created.reference(),
                "update-dashboard",
                DIGEST,
                vec![CatalogChange::UpdateProduct {
                    expected_revision: 1,
                    record: product(ProductRecordKind::Dashboard, "dash-1", 2, "alice/new"),
                }],
            ))
            .unwrap();
        assert_eq!(updated.product_records["dashboard/dash-1"].revision, 2);
        let deleted = updated
            .prepare(change(
                updated.reference(),
                "delete-dashboard",
                DIGEST,
                vec![CatalogChange::DeleteProduct {
                    kind: ProductRecordKind::Dashboard,
                    id: "dash-1".into(),
                    expected_revision: 2,
                }],
            ))
            .unwrap();
        assert!(deleted.product_records.is_empty());
    }

    #[test]
    fn unique_indexes_validate_the_final_atomic_snapshot() {
        let base = CatalogSnapshot::empty("snapshot-genesis").unwrap();
        let collision = base.prepare(change(
            base.reference(),
            "duplicate-dashboard-name",
            DIGEST,
            vec![
                CatalogChange::CreateProduct {
                    record: product(ProductRecordKind::Dashboard, "dash-1", 1, "alice/home"),
                },
                CatalogChange::CreateProduct {
                    record: product(ProductRecordKind::Dashboard, "dash-2", 1, "alice/home"),
                },
            ],
        ));
        assert!(
            collision
                .unwrap_err()
                .to_string()
                .contains("duplicate dashboard")
        );
        assert!(base.product_records.is_empty());

        let same_value_different_kind = base
            .prepare(change(
                base.reference(),
                "different-kind-index",
                DIGEST,
                vec![
                    CatalogChange::CreateProduct {
                        record: product(ProductRecordKind::Dashboard, "dash-1", 1, "alice/home"),
                    },
                    CatalogChange::CreateProduct {
                        record: product(ProductRecordKind::Chart, "chart-1", 1, "alice/home"),
                    },
                ],
            ))
            .unwrap();
        assert_eq!(same_value_different_kind.product_records.len(), 2);
    }

    #[test]
    fn bounds_unique_values_per_product_record() {
        let base = CatalogSnapshot::empty("snapshot-genesis").unwrap();
        let mut record = product(ProductRecordKind::Dashboard, "dash-1", 1, "alice/home");
        record.unique_values = (0..=MAX_UNIQUE_VALUES_PER_PRODUCT_RECORD)
            .map(|index| (format!("index_{index}"), format!("value-{index}")))
            .collect();

        let result = base.prepare(change(
            base.reference(),
            "too-many-unique-values",
            DIGEST,
            vec![CatalogChange::CreateProduct { record }],
        ));
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("unique-value limit")
        );
        assert!(base.product_records.is_empty());
    }

    #[test]
    fn rejects_dangling_and_wrong_kind_product_references() {
        let base = CatalogSnapshot::empty("snapshot-genesis").unwrap();
        let mut chart = product(ProductRecordKind::Chart, "chart-1", 1, "alice/chart");
        chart
            .references
            .insert(reference(ProductRecordKind::Dataset, "missing"));
        assert!(
            base.prepare(change(
                base.reference(),
                "dangling-chart",
                DIGEST,
                vec![CatalogChange::CreateProduct { record: chart }],
            ))
            .unwrap_err()
            .to_string()
            .contains("references missing")
        );

        let mut dataset = product(ProductRecordKind::Dataset, "dataset-1", 1, "alice/data");
        dataset
            .references
            .insert(reference(ProductRecordKind::Chart, "chart-1"));
        assert!(
            base.prepare(change(
                base.reference(),
                "wrong-kind-reference",
                DIGEST,
                vec![CatalogChange::CreateProduct { record: dataset }],
            ))
            .unwrap_err()
            .to_string()
            .contains("dataset cannot reference chart")
        );
    }

    #[test]
    fn same_transaction_parent_and_child_resolve_in_final_snapshot() {
        let base = CatalogSnapshot::empty("snapshot-genesis").unwrap();
        let dataset = product(ProductRecordKind::Dataset, "dataset-1", 1, "alice/data");
        let mut chart = product(ProductRecordKind::Chart, "chart-1", 1, "alice/chart");
        chart
            .references
            .insert(reference(ProductRecordKind::Dataset, "dataset-1"));
        let mut dashboard = product(
            ProductRecordKind::Dashboard,
            "dashboard-1",
            1,
            "alice/dashboard",
        );
        dashboard.references.extend([
            reference(ProductRecordKind::Dataset, "dataset-1"),
            reference(ProductRecordKind::Chart, "chart-1"),
        ]);

        let snapshot = base
            .prepare(change(
                base.reference(),
                "create-related-records",
                DIGEST,
                vec![
                    CatalogChange::CreateProduct { record: dashboard },
                    CatalogChange::CreateProduct { record: chart },
                    CatalogChange::CreateProduct { record: dataset },
                ],
            ))
            .unwrap();
        assert_eq!(snapshot.product_records.len(), 3);
    }

    #[test]
    fn delete_restricts_referenced_product_records() {
        let base = CatalogSnapshot::empty("snapshot-genesis").unwrap();
        let dataset = product(ProductRecordKind::Dataset, "dataset-1", 1, "alice/data");
        let mut chart = product(ProductRecordKind::Chart, "chart-1", 1, "alice/chart");
        chart
            .references
            .insert(reference(ProductRecordKind::Dataset, "dataset-1"));
        let populated = base
            .prepare(change(
                base.reference(),
                "populate",
                DIGEST,
                vec![
                    CatalogChange::CreateProduct { record: dataset },
                    CatalogChange::CreateProduct { record: chart },
                ],
            ))
            .unwrap();

        let deletion = populated.prepare(change(
            populated.reference(),
            "delete-parent",
            DIGEST,
            vec![CatalogChange::DeleteProduct {
                kind: ProductRecordKind::Dataset,
                id: "dataset-1".into(),
                expected_revision: 1,
            }],
        ));
        assert!(
            deletion
                .unwrap_err()
                .to_string()
                .contains("references missing")
        );
        assert!(populated.product_records.contains_key("dataset/dataset-1"));

        let explicit_graph_delete = populated
            .prepare(change(
                populated.reference(),
                "delete-explicit-graph",
                DIGEST,
                vec![
                    CatalogChange::DeleteProduct {
                        kind: ProductRecordKind::Dataset,
                        id: "dataset-1".into(),
                        expected_revision: 1,
                    },
                    CatalogChange::DeleteProduct {
                        kind: ProductRecordKind::Chart,
                        id: "chart-1".into(),
                        expected_revision: 1,
                    },
                ],
            ))
            .unwrap();
        assert!(explicit_graph_delete.product_records.is_empty());
    }

    #[test]
    fn legacy_typed_records_without_references_remain_readable() {
        let record = serde_json::json!({
            "kind": "dashboard",
            "id": "dashboard-1",
            "revision": 1,
            "document": {"path": "control/dashboard-1.json", "sha256": DIGEST},
            "unique_values": {"owner_name": "alice/home"}
        });
        let decoded: ProductRecordRef = serde_json::from_value(record).unwrap();
        assert!(decoded.references.is_empty());
        validate_product_record(&decoded).unwrap();
    }

    #[test]
    fn product_reads_support_point_unique_and_deterministic_pages() {
        let base = CatalogSnapshot::empty("snapshot-genesis").unwrap();
        let snapshot = base
            .prepare(change(
                base.reference(),
                "read-fixture",
                DIGEST,
                ["charlie", "alpha", "bravo"]
                    .into_iter()
                    .map(|id| CatalogChange::CreateProduct {
                        record: product(
                            ProductRecordKind::Dashboard,
                            id,
                            1,
                            &format!("alice/{id}"),
                        ),
                    })
                    .collect(),
            ))
            .unwrap();

        assert_eq!(
            snapshot
                .product_record(ProductRecordKind::Dashboard, "bravo")
                .unwrap()
                .unwrap()
                .id,
            "bravo"
        );
        assert_eq!(
            snapshot
                .product_record_by_unique_value(
                    ProductRecordKind::Dashboard,
                    "owner_name",
                    "alice/charlie",
                )
                .unwrap()
                .unwrap()
                .id,
            "charlie"
        );
        assert!(
            snapshot
                .product_record_by_unique_value(
                    ProductRecordKind::Dashboard,
                    "owner_name",
                    "Alice/charlie",
                )
                .unwrap()
                .is_none()
        );
        let first = snapshot
            .product_records_page(ProductRecordKind::Dashboard, 2, None)
            .unwrap();
        assert_eq!(
            first
                .records
                .iter()
                .map(|record| record.id.as_str())
                .collect::<Vec<_>>(),
            ["alpha", "bravo"]
        );
        assert_eq!(first.snapshot, snapshot.reference());
        let second = snapshot
            .product_records_page(
                ProductRecordKind::Dashboard,
                2,
                first.next_cursor.as_deref(),
            )
            .unwrap();
        assert_eq!(
            second
                .records
                .iter()
                .map(|record| record.id.as_str())
                .collect::<Vec<_>>(),
            ["charlie"]
        );
        assert!(second.next_cursor.is_none());
    }

    #[test]
    fn product_cursor_is_snapshot_and_kind_pinned_and_malformed_input_fails() {
        let base = CatalogSnapshot::empty("snapshot-genesis").unwrap();
        let snapshot = base
            .prepare(change(
                base.reference(),
                "cursor-fixture",
                DIGEST,
                ["alpha", "bravo"]
                    .into_iter()
                    .map(|id| CatalogChange::CreateProduct {
                        record: product(
                            ProductRecordKind::Dashboard,
                            id,
                            1,
                            &format!("alice/{id}"),
                        ),
                    })
                    .collect(),
            ))
            .unwrap();
        let cursor = snapshot
            .product_records_page(ProductRecordKind::Dashboard, 1, None)
            .unwrap()
            .next_cursor
            .unwrap();
        let newer = snapshot
            .prepare(change(
                snapshot.reference(),
                "newer-snapshot",
                DIGEST,
                vec![CatalogChange::CreateProduct {
                    record: product(ProductRecordKind::Dashboard, "charlie", 1, "alice/charlie"),
                }],
            ))
            .unwrap();

        assert_eq!(
            snapshot
                .product_records_page(ProductRecordKind::Dashboard, 10, Some(&cursor))
                .unwrap()
                .records[0]
                .id,
            "bravo"
        );
        assert!(
            newer
                .product_records_page(ProductRecordKind::Dashboard, 10, Some(&cursor))
                .unwrap_err()
                .to_string()
                .contains("different snapshot")
        );
        assert!(
            snapshot
                .product_records_page(ProductRecordKind::Chart, 10, Some(&cursor))
                .unwrap_err()
                .to_string()
                .contains("different record kind")
        );
        for malformed in [
            "",
            "v2:1:x:dashboard:61",
            "v1:x:x:dashboard:61",
            "v1:1:x:dashboard:zz",
        ] {
            assert!(
                snapshot
                    .product_records_page(ProductRecordKind::Dashboard, 10, Some(malformed))
                    .is_err(),
                "{malformed}"
            );
        }
        assert!(
            snapshot
                .product_records_page(ProductRecordKind::Dashboard, 0, None)
                .is_err()
        );
    }

    #[test]
    fn failed_change_does_not_mutate_base() {
        let base = CatalogSnapshot::empty("snapshot-genesis").unwrap();
        let result = base.prepare(change(
            base.reference(),
            "op-1",
            DIGEST,
            vec![CatalogChange::Delete {
                table: "bronze.missing".into(),
            }],
        ));
        assert!(result.is_err());
        assert!(base.tables.is_empty());
        assert_eq!(base.generation, 0);
    }

    #[test]
    fn rejects_stale_generation() {
        let base = CatalogSnapshot::empty("snapshot-genesis").unwrap();
        let first = base
            .prepare(change(
                base.reference(),
                "op-1",
                DIGEST,
                vec![CatalogChange::Put {
                    table: "bronze.orders".into(),
                    reference: table("orders"),
                }],
            ))
            .unwrap();
        assert!(
            first
                .prepare(change(
                    base.reference(),
                    "op-2",
                    DIGEST,
                    vec![CatalogChange::Put {
                        table: "bronze.events".into(),
                        reference: table("events")
                    }]
                ))
                .unwrap_err()
                .to_string()
                .contains("stale")
        );
    }

    #[test]
    fn replays_current_operation_and_rejects_digest_mismatch() {
        let base = CatalogSnapshot::empty("snapshot-genesis").unwrap();
        let request = change(
            base.reference(),
            "op-1",
            DIGEST,
            vec![CatalogChange::Put {
                table: "bronze.orders".into(),
                reference: table("orders"),
            }],
        );
        let next = base.prepare(request.clone()).unwrap();
        assert_eq!(next.prepare(request).unwrap(), next);
        let mismatch = change(
            base.reference(),
            "op-1",
            &"b".repeat(64),
            vec![CatalogChange::Put {
                table: "bronze.orders".into(),
                reference: table("orders"),
            }],
        );
        assert!(
            next.prepare(mismatch)
                .unwrap_err()
                .to_string()
                .contains("different request digest")
        );
    }

    #[test]
    fn rejects_unsafe_paths() {
        for path in [
            "../escape.parquet",
            "a\\b.parquet",
            "C:drive.parquet",
            "/absolute.parquet",
            "a//b.parquet",
        ] {
            let reference = TableManifestRef {
                manifest: ImmutableFileRef {
                    path: "tables/a/manifest.json".into(),
                    sha256: DIGEST.into(),
                },
                parquet_files: vec![ImmutableFileRef {
                    path: path.into(),
                    sha256: DIGEST.into(),
                }],
            };
            assert!(validate_table_reference(&reference).is_err(), "{path}");
        }
    }

    #[test]
    fn statistics_bind_exact_table_snapshot_and_table_replacement_invalidates_them() {
        let base = CatalogSnapshot::empty("snapshot-genesis").unwrap();
        let orders = table("orders");
        let with_table = base
            .prepare(change(
                base.reference(),
                "put-table",
                DIGEST,
                vec![CatalogChange::Put {
                    table: "sales.orders".into(),
                    reference: orders.clone(),
                }],
            ))
            .unwrap();
        let statistics = TableStatisticsRef {
            catalog_snapshot_sha256: DIGEST.into(),
            source_identity_sha256: DIGEST.into(),
            document: control("statistics-orders"),
            row_count: 42,
        };
        let analyzed = with_table
            .prepare(change(
                with_table.reference(),
                "analyze-table",
                DIGEST,
                vec![
                    CatalogChange::PutRuntimeTableSource {
                        table: "sales.orders".into(),
                        source: RuntimeTableSourceRef {
                            catalog_snapshot_sha256: DIGEST.into(),
                            source_identity_sha256: DIGEST.into(),
                        },
                    },
                    CatalogChange::PutStatistics {
                        table: "sales.orders".into(),
                        statistics,
                    },
                ],
            ))
            .unwrap();
        assert_eq!(analyzed.table_statistics["sales.orders"].row_count, 42);

        let replaced = analyzed
            .prepare(change(
                analyzed.reference(),
                "replace-table",
                DIGEST,
                vec![CatalogChange::Put {
                    table: "sales.orders".into(),
                    reference: table("orders-v2"),
                }],
            ))
            .unwrap();
        assert!(!replaced.table_statistics.contains_key("sales.orders"));
    }

    #[test]
    fn statistics_validate_runtime_source_binding() {
        let base = CatalogSnapshot::empty("snapshot-genesis").unwrap();
        let statistics = TableStatisticsRef {
            catalog_snapshot_sha256: DIGEST.into(),
            source_identity_sha256: DIGEST.into(),
            document: control("statistics-orders"),
            row_count: 42,
        };
        assert!(
            base.prepare(change(
                base.reference(),
                "analyze-absent",
                DIGEST,
                vec![CatalogChange::PutStatistics {
                    table: "sales.orders".into(),
                    statistics: statistics.clone()
                }]
            ))
            .is_err()
        );
        let bound = base
            .prepare(change(
                base.reference(),
                "bind-runtime",
                DIGEST,
                vec![CatalogChange::PutRuntimeTableSource {
                    table: "sales.orders".into(),
                    source: RuntimeTableSourceRef {
                        catalog_snapshot_sha256: DIGEST.into(),
                        source_identity_sha256: DIGEST.into(),
                    },
                }],
            ))
            .unwrap();
        assert!(
            bound
                .prepare(change(
                    bound.reference(),
                    "valid-statistics",
                    DIGEST,
                    vec![CatalogChange::PutStatistics {
                        table: "sales.orders".into(),
                        statistics: statistics.clone()
                    }]
                ))
                .is_ok()
        );
        let mut invalid = statistics;
        invalid.source_identity_sha256 = "bad".into();
        assert!(
            base.prepare(change(
                base.reference(),
                "bad-statistics",
                DIGEST,
                vec![CatalogChange::PutStatistics {
                    table: "sales.orders".into(),
                    statistics: invalid
                }]
            ))
            .is_err()
        );
    }
}
