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
// A backstop rather than the ceiling. Records live in sharded objects now,
// each bounded when it is encoded, and that is what constrains a deployment.
// At the per-owner volumes the product permits, 100,000 was reached by 57
// owners, which was the old inline ceiling wearing a different hat.
pub const MAX_PRODUCT_RECORDS: usize = 5_000_000;
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

impl TableManifestRef {
    /// Validates a manifest assembled from immutable writer references before
    /// it is staged into a catalog snapshot.
    pub fn validated(
        manifest: ImmutableFileRef,
        parquet_files: Vec<ImmutableFileRef>,
    ) -> Result<Self, ManifestError> {
        let reference = Self {
            manifest,
            parquet_files,
        };
        validate_table_reference(&reference)?;
        Ok(reference)
    }
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

/// Small typed row representation used by the catalog transaction layer.
///
/// JSON values are supported for control-plane documents (for example DLM
/// configuration and visibility rules), but remain bounded and validated by
/// the manifest. Large artifacts belong in immutable table objects referenced
/// by a future mutation writer.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", content = "value", rename_all = "snake_case")]
pub enum TypedValue {
    Null,
    Boolean(bool),
    Integer(i64),
    String(String),
    /// A bounded JSON document. This is deliberately one variant instead of
    /// exposing an unbounded collection of engine-specific scalar types.
    Json(serde_json::Value),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TypedColumnType {
    Null,
    Boolean,
    Integer,
    String,
    Json,
}

impl TypedValue {
    fn column_type(&self) -> TypedColumnType {
        match self {
            Self::Null => TypedColumnType::Null,
            Self::Boolean(_) => TypedColumnType::Boolean,
            Self::Integer(_) => TypedColumnType::Integer,
            Self::String(_) => TypedColumnType::String,
            Self::Json(_) => TypedColumnType::Json,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TypedTableSchema {
    pub columns: BTreeMap<String, TypedColumnType>,
    pub primary_key: String,
    #[serde(default)]
    pub unique_keys: BTreeSet<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TypedRow {
    pub primary_key: String,
    pub revision: u64,
    pub columns: BTreeMap<String, TypedValue>,
    #[serde(default)]
    pub unique_keys: BTreeMap<String, String>,
}

/// One typed table's rows, stored as an immutable object outside the snapshot.
///
/// The object is named by the digest of its contents, which is what makes a
/// commit cost the size of what it changed: a table whose rows are untouched
/// keeps the reference it already had and is not written again.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TypedTableRef {
    pub document: ImmutableFileRef,
    /// Carried so a reader can report a table's size without fetching it.
    pub rows: u64,
}

/// Rebuildable manifest indexes for typed rows. The index values identify the
/// row revision, allowing validation to detect stale or mismatched index data.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct TypedRowIndexes {
    pub primary_keys: BTreeMap<String, u64>,
    pub unique_keys: BTreeMap<String, BTreeMap<String, String>>,
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
    /// Revisioned typed rows owned by the same immutable catalog snapshot.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub typed_rows: BTreeMap<String, BTreeMap<String, TypedRow>>,
    /// Where each typed table's rows live, one immutable object per table,
    /// content-addressed.
    ///
    /// Typed rows used to be serialised into the snapshot itself, and a commit
    /// republishes the whole snapshot — so the cost of any write was the size
    /// of the entire catalog, and the snapshot had a ceiling it could reach. A
    /// production installation stopped accepting writes with 5,650 rows of
    /// query history filling 63% of an 8 MiB snapshot, while every dataset,
    /// chart and dashboard it held came to 74 KB. Saving a favourite rewrote
    /// all of it.
    ///
    /// Naming a table's object by the digest of its contents means a table
    /// that did not change is not written again, and the snapshot stays a few
    /// kilobytes of references however large the tables grow.
    #[serde(default)]
    pub typed_tables: BTreeMap<String, TypedTableRef>,
    /// Persistent, rebuildable typed-row primary and unique indexes.
    /// Older snapshots may omit this field and are accepted as legacy.
    #[serde(default)]
    pub typed_row_indexes: BTreeMap<String, TypedRowIndexes>,
    /// Typed-table schema metadata. Legacy typed-row snapshots may omit this
    /// field; the next typed-row change infers a bounded schema.
    #[serde(default)]
    pub typed_schemas: BTreeMap<String, TypedTableSchema>,
    /// How many product records point at each referenced target, keyed the way
    /// `product_record_key` keys a record.
    ///
    /// Referential integrity is enforced today by scanning every record and
    /// resolving its references against this same map of records. That scan is
    /// exactly what eviction removes: once the bulk kinds live in side objects
    /// the snapshot can no longer see who points at a dataset, and deleting
    /// one would stop being restricted without anything failing.
    ///
    /// So this is maintained alongside the scan while the scan is still
    /// authoritative, and `validate` checks the two agree. It is small by
    /// construction -- one entry per referenced target rather than per
    /// referencing record, and the 800 query-history rows an owner may hold
    /// point at a handful of datasets between them.
    ///
    /// Absent on snapshots written before this field existed, which is why it
    /// is only cross-checked when present.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub reference_counts: BTreeMap<String, u64>,
    /// Where each accumulating kind's records live, one immutable object per
    /// kind, content-addressed and named by the digest of its contents.
    ///
    /// `product_records` still holds every kind in memory; this is only about
    /// which of them are serialised into the snapshot. A kind named here is
    /// resolved and merged back on read, so nothing above this layer can tell
    /// the difference.
    ///
    /// Empty on snapshots written before the split, which is what makes those
    /// readable: no entries means nothing to resolve and every record is
    /// already inline.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub product_record_tables: BTreeMap<String, ImmutableFileRef>,
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
    InsertTypedRow {
        table: String,
        row: TypedRow,
    },
    UpdateTypedRow {
        table: String,
        expected_revision: u64,
        row: TypedRow,
    },
    DeleteTypedRow {
        table: String,
        primary_key: String,
        expected_revision: u64,
    },
    DefineTypedSchema {
        table: String,
        schema: TypedTableSchema,
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
            typed_rows: BTreeMap::new(),
            typed_tables: BTreeMap::new(),
            typed_row_indexes: BTreeMap::new(),
            typed_schemas: BTreeMap::new(),
            reference_counts: BTreeMap::new(),
            product_record_tables: BTreeMap::new(),
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
        self.prepare_verified(request)
    }

    /// `prepare` for a base that is already known to be valid.
    ///
    /// Validating the base is a full walk of every product record and typed row
    /// it holds plus a rebuild of all its indexes, and a transaction pays it
    /// again for every statement it stages even though the base cannot change
    /// while the session is open. The two callers inside this crate both hold a
    /// snapshot that was validated when it was read from storage, or that was
    /// built and validated by this function; nothing else can reach it, which is
    /// why it is crate-private rather than part of the published API.
    pub(crate) fn prepare_verified(&self, request: PrepareChange) -> Result<Self, ManifestError> {
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
        let mut typed_rows = self.typed_rows.clone();
        let mut typed_schemas = self.typed_schemas.clone();
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
                CatalogChange::InsertTypedRow { table, row } => {
                    ensure_typed_schema(&mut typed_schemas, &table, &row)?;
                    validate_row_against_schema(&typed_schemas[&table], &row)?;
                    let rows = typed_rows.entry(table).or_default();
                    if rows.contains_key(&row.primary_key) {
                        return Err(error("typed row primary key already exists"));
                    }
                    if row.revision != 1 {
                        return Err(error("new typed row revision must be one"));
                    }
                    rows.insert(row.primary_key.clone(), row);
                }
                CatalogChange::UpdateTypedRow {
                    table,
                    expected_revision,
                    row,
                } => {
                    ensure_typed_schema(&mut typed_schemas, &table, &row)?;
                    validate_row_against_schema(&typed_schemas[&table], &row)?;
                    let rows = typed_rows
                        .get_mut(&table)
                        .ok_or_else(|| error("typed row table does not exist"))?;
                    let current = rows
                        .get(&row.primary_key)
                        .ok_or_else(|| error("typed row primary key does not exist"))?;
                    if current.revision != expected_revision {
                        return Err(error("stale typed row revision"));
                    }
                    let next_revision = expected_revision
                        .checked_add(1)
                        .ok_or_else(|| error("typed row revision overflow"))?;
                    if row.revision != next_revision {
                        return Err(error("typed row revision must advance by one"));
                    }
                    rows.insert(row.primary_key.clone(), row);
                }
                CatalogChange::DeleteTypedRow {
                    table,
                    primary_key,
                    expected_revision,
                } => {
                    let rows = typed_rows
                        .get_mut(&table)
                        .ok_or_else(|| error("typed row table does not exist"))?;
                    let current = rows
                        .get(&primary_key)
                        .ok_or_else(|| error("typed row primary key does not exist"))?;
                    if current.revision != expected_revision {
                        return Err(error("stale typed row revision"));
                    }
                    rows.remove(&primary_key);
                }
                CatalogChange::DefineTypedSchema { table, schema } => {
                    validate_typed_schema(&schema)?;
                    if let Some(rows) = typed_rows.get(&table) {
                        for row in rows.values() {
                            validate_row_against_schema(&schema, row)?;
                        }
                    }
                    if let Some(previous) = typed_schemas.get(&table)
                        && previous != &schema
                    {
                        return Err(error("typed table schema cannot change in place"));
                    }
                    typed_schemas.insert(table, schema);
                }
            }
        }
        if tables.len() > MAX_TABLES {
            return Err(error("snapshot table limit exceeded"));
        }
        if control_records.len() > MAX_CONTROL_RECORDS {
            return Err(error("snapshot control-record limit exceeded"));
        }
        let reference_counts = count_references(&product_records);
        validate_product_records(&product_records, &reference_counts)?;
        // `build_typed_row_indexes` validates the rows it is given before it
        // builds anything, so validating them here as well walked every row in
        // the catalog twice for nothing.
        validate_typed_schema_bindings(&typed_rows, &typed_schemas)?;
        let typed_row_indexes = build_typed_row_indexes(&typed_rows)?;
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
            typed_rows,
            typed_tables: self.typed_tables.clone(),
            typed_row_indexes,
            typed_schemas,
            // Derived from the records this snapshot actually carries, so the
            // index is correct by construction at every generation rather than
            // carried forward and hoped about.
            reference_counts,
            // Assigned when the manifest is split for publication; the
            // in-memory snapshot carries every record inline until then.
            product_record_tables: BTreeMap::new(),
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
        validate_product_records(&self.product_records, &self.reference_counts)?;
        validate_typed_rows(&self.typed_rows)?;
        validate_typed_schema_bindings(&self.typed_rows, &self.typed_schemas)?;
        validate_typed_row_indexes(&self.typed_rows, &self.typed_row_indexes)?;
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
    let mut typed_keys = BTreeSet::new();
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
            CatalogChange::InsertTypedRow { table, row }
            | CatalogChange::UpdateTypedRow { table, row, .. } => {
                validate_table_name(table)?;
                validate_typed_row(row)?;
                if !typed_keys.insert((table.as_str(), row.primary_key.as_str())) {
                    return Err(error(format!(
                        "typed row '{table}.{}' is changed more than once",
                        row.primary_key
                    )));
                }
            }
            CatalogChange::DeleteTypedRow {
                table, primary_key, ..
            } => {
                validate_table_name(table)?;
                validate_identifier("typed row primary key", primary_key)?;
                if !typed_keys.insert((table.as_str(), primary_key.as_str())) {
                    return Err(error(format!(
                        "typed row '{table}.{primary_key}' is changed more than once"
                    )));
                }
            }
            CatalogChange::DefineTypedSchema { table, schema } => {
                validate_table_name(table)?;
                validate_typed_schema(schema)?;
                if !typed_keys.insert((table.as_str(), "__schema__")) {
                    return Err(error(format!(
                        "typed schema '{table}' is changed more than once"
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
    Favorite,
    Source,
    UserRecent,
    QueryHistory,
    Activity,
    ChatSession,
    ChatMessage,
}

impl ProductRecordKind {
    pub(crate) fn name(self) -> &'static str {
        match self {
            Self::Dataset => "dataset",
            Self::Chart => "chart",
            Self::Dashboard => "dashboard",
            Self::SavedQuery => "saved_query",
            Self::UserTheme => "user_theme",
            Self::DlmDefinition => "dlm_definition",
            Self::DlmRun => "dlm_run",
            Self::Favorite => "favorite",
            Self::Source => "source",
            Self::UserRecent => "user_recent",
            Self::QueryHistory => "query_history",
            Self::Activity => "activity",
            Self::ChatSession => "chat_session",
            Self::ChatMessage => "chat_message",
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
            "favorite" => Some(Self::Favorite),
            "source" => Some(Self::Source),
            "user_recent" => Some(Self::UserRecent),
            "query_history" => Some(Self::QueryHistory),
            "activity" => Some(Self::Activity),
            "chat_session" => Some(Self::ChatSession),
            "chat_message" => Some(Self::ChatMessage),
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

/// Whether a kind accumulates with use rather than with curation.
///
/// The distinction is not importance, it is growth. A deployment holds as many
/// datasets as someone chose to create; it holds as many query-history rows as
/// its owners ran queries, bounded only by a retention cap, and as much
/// activity as they performed, bounded by nothing. Measured against the
/// snapshot ceiling, one fully active owner costs 695,400 bytes of these and
/// the whole snapshot holds twelve such owners.
///
/// `ChatSession` is deliberately not here. It is the target of every
/// `ChatMessage` reference, there is one per conversation rather than one per
/// message, and keeping it inline means the reference a message carries still
/// resolves against a record the snapshot can see.
#[must_use]
pub const fn accumulates_with_use(kind: ProductRecordKind) -> bool {
    matches!(
        kind,
        ProductRecordKind::QueryHistory
            | ProductRecordKind::ChatMessage
            | ProductRecordKind::UserRecent
            | ProductRecordKind::Activity
            | ProductRecordKind::DlmRun
    )
}

/// How many objects a single accumulating kind is spread over.
///
/// A kind was one object, so recording one query republished every query. The
/// shard count divides both the write and the capacity: a write rewrites one
/// shard, and a kind holds this many times one object's worth of records.
///
/// Sixty-four is chosen from the two failures either side of it. Too few and a
/// write still rewrites megabytes; too many and resolving a kind becomes a
/// long series of small reads on every snapshot load.
pub const PRODUCT_RECORD_SHARDS: u8 = 64;

/// Which shard a record belongs to, from the digest of its key.
///
/// The digest rather than the owner, so that one owner writing heavily does
/// not concentrate on a single shard, and so a record with no owner still has
/// a home. Stable for the life of a key, which is what lets an unchanged shard
/// keep its object.
#[must_use]
pub fn product_record_shard(key: &str) -> u8 {
    // FNV-1a, 64-bit. Small, stable across processes and architectures, and
    // not a hash whose output anyone stores -- only the bucket is used.
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in key.as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    // FNV-1a avalanches poorly in its low bits, and the bucket is taken from
    // exactly those. Measured without this step, keys of the shape the product
    // issues reached 16 of 64 shards and a write cost eight times what the
    // shard count promised. The MurmurHash3 finaliser spreads the entropy
    // across the whole word first.
    hash ^= hash >> 33;
    hash = hash.wrapping_mul(0xff51_afd7_ed55_8ccd);
    hash ^= hash >> 33;
    hash = hash.wrapping_mul(0xc4ce_b9fe_1a85_ec53);
    hash ^= hash >> 33;
    (hash % u64::from(PRODUCT_RECORD_SHARDS)) as u8
}

/// The name a kind's shard is published under.
#[must_use]
pub fn product_shard_name(kind_name: &str, shard: u8) -> String {
    format!("{kind_name}/{shard:02}")
}

/// Split records into the ones the snapshot carries and the ones it points at.
///
/// The second map is keyed by kind name, which is what names the object each
/// kind is published to. Kinds with no records produce no entry, so a
/// deployment that has never used chat publishes no chat object.
/// Product records keyed the way `product_record_key` keys them.
pub type ProductRecords = BTreeMap<String, ProductRecordRef>;
/// Those records grouped by kind name, which is what names the object each
/// kind is published to.
pub type ProductRecordsByKind = BTreeMap<String, ProductRecords>;

#[must_use]
pub fn partition_product_records(
    records: &ProductRecords,
) -> (ProductRecords, ProductRecordsByKind) {
    // Every kind, not only the growing ones. The artifacts were left inline
    // when the accumulating kinds were evicted, and measurement then put them
    // at the whole of the remaining ceiling: 11.2 KB an owner against an 8 MiB
    // snapshot, republished in full to record one favourite.
    let inline = BTreeMap::new();
    let mut evicted: ProductRecordsByKind = BTreeMap::new();
    for (key, record) in records {
        let shard = product_record_shard(key);
        evicted
            .entry(product_shard_name(record.kind.name(), shard))
            .or_default()
            .insert(key.clone(), record.clone());
    }
    (inline, evicted)
}

/// Rejoin what `partition_product_records` separated.
///
/// Reading a snapshot resolves its per-kind objects and merges them back, so
/// everything above this layer sees the map it has always seen. A key present
/// in both halves is a corrupt snapshot rather than a merge policy, so it is
/// refused instead of resolved.
pub fn merge_product_records(
    inline: ProductRecords,
    evicted: ProductRecordsByKind,
) -> Result<ProductRecords, ManifestError> {
    let mut merged = inline;
    for (shard_name, records) in evicted {
        for (key, record) in records {
            let expected = product_shard_name(record.kind.name(), product_record_shard(&key));
            if shard_name != expected {
                return Err(error(format!(
                    "product record '{key}' was published under '{shard_name}'"
                )));
            }
            if merged.insert(key.clone(), record).is_some() {
                return Err(error(format!(
                    "product record '{key}' is published both inline and in a per-kind object"
                )));
            }
        }
    }
    Ok(merged)
}

pub(crate) fn product_record_key(kind: ProductRecordKind, id: &str) -> String {
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
            | ProductRecordKind::UserTheme
            | ProductRecordKind::Source => false,
            ProductRecordKind::Activity => false,
            ProductRecordKind::ChatSession => false,
            ProductRecordKind::ChatMessage => reference.kind == ProductRecordKind::ChatSession,
            ProductRecordKind::QueryHistory => matches!(reference.kind, ProductRecordKind::Dataset),
            ProductRecordKind::DlmDefinition => reference.kind == ProductRecordKind::Dataset,
            ProductRecordKind::DlmRun => reference.kind == ProductRecordKind::DlmDefinition,
            ProductRecordKind::Favorite => matches!(
                reference.kind,
                ProductRecordKind::Dataset
                    | ProductRecordKind::Chart
                    | ProductRecordKind::Dashboard
                    | ProductRecordKind::SavedQuery
                    | ProductRecordKind::Source
            ),
            ProductRecordKind::UserRecent => matches!(
                reference.kind,
                ProductRecordKind::Dataset
                    | ProductRecordKind::Chart
                    | ProductRecordKind::Dashboard
            ),
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
    counts: &BTreeMap<String, u64>,
) -> Result<(), ManifestError> {
    // Bounds what one snapshot may hold in total.
    //
    // This counted every record in memory, which kept the pre-eviction ceiling
    // in force through a second door: measured, bytes permitted 749 owners
    // while this count permitted 57. It then counted only what the snapshot
    // carried, which became nothing once every kind was published beside it.
    //
    // A vacuous check is worse than none, because it reads as a bound while
    // being unable to fail. So it is a total again, raised to match what now
    // actually constrains a deployment: each kind is spread over
    // PRODUCT_RECORD_SHARDS objects, each bounded when it is encoded, and that
    // is the real limit. This is a backstop against a catalogue growing
    // without anyone noticing, not the ceiling.
    if records.len() > MAX_PRODUCT_RECORDS {
        return Err(error("snapshot product-record limit exceeded"));
    }
    let mut unique = BTreeSet::new();
    let mut observed: BTreeMap<String, u64> = BTreeMap::new();
    for (key, record) in records {
        validate_product_record(record)?;
        if key != &record.key() {
            return Err(error("product record map key does not match its identity"));
        }
        for (index, value) in &record.unique_values {
            // Ownership is metadata used for access control, not a uniqueness
            // constraint. A principal may own many datasets, charts, or runs.
            if matches!(
                index.as_str(),
                "owner_principal"
                    | "chart_dataset_revision"
                    | "dashboard_chart_revisions"
                    | "query_owner"
                    | "activity_actor"
                    | "chat_owner"
                    | "dlm_run_state"
                    | "dlm_definition_revision"
            ) {
                continue;
            }
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
            *observed.entry(target).or_insert(0) += 1;
        }
    }
    if !counts.is_empty() && *counts != observed {
        // The index is about to become the only answer to "may this be
        // deleted?", so a disagreement with the scan has to be loud while the
        // scan can still be consulted.
        return Err(error(
            "reference counts disagree with the records they were derived from",
        ));
    }
    Ok(())
}

/// Count references by target, the way `validate` derives them.
///
/// This is the index's definition. Keeping it beside the validation that
/// cross-checks it is what stops the two drifting apart.
#[must_use]
pub fn count_references(records: &BTreeMap<String, ProductRecordRef>) -> BTreeMap<String, u64> {
    let mut counts = BTreeMap::new();
    for record in records.values() {
        for reference in &record.references {
            *counts
                .entry(product_record_key(reference.kind, &reference.id))
                .or_insert(0) += 1;
        }
    }
    counts
}

fn validate_typed_rows(
    tables: &BTreeMap<String, BTreeMap<String, TypedRow>>,
) -> Result<(), ManifestError> {
    if tables.len() > MAX_TABLES {
        return Err(error("typed row table limit exceeded"));
    }
    for (table, rows) in tables {
        validate_table_name(table)?;
        if rows.len() > MAX_PRODUCT_RECORDS {
            return Err(error("typed row table limit exceeded"));
        }
        let mut unique = BTreeSet::new();
        for (primary_key, row) in rows {
            if primary_key != &row.primary_key {
                return Err(error("typed row map key does not match its primary key"));
            }
            validate_typed_row(row)?;
            for (index, value) in &row.unique_keys {
                if !unique.insert((index.as_str(), value.as_str())) {
                    return Err(error(format!(
                        "duplicate typed row unique index '{index}' value"
                    )));
                }
            }
        }
    }
    Ok(())
}

fn validate_typed_schema(schema: &TypedTableSchema) -> Result<(), ManifestError> {
    validate_identifier("typed schema primary key", &schema.primary_key)?;
    if schema.columns.len() > 256 {
        return Err(error("typed schema column limit exceeded"));
    }
    for name in schema.columns.keys() {
        validate_identifier("typed schema column", name)?;
    }
    if schema.unique_keys.len() > MAX_UNIQUE_VALUES_PER_PRODUCT_RECORD {
        return Err(error("typed schema unique-index limit exceeded"));
    }
    for name in &schema.unique_keys {
        validate_identifier("typed schema unique index", name)?;
        if !schema.columns.contains_key(name) {
            return Err(error("typed schema unique column is not declared"));
        }
    }
    Ok(())
}

fn ensure_typed_schema(
    schemas: &mut BTreeMap<String, TypedTableSchema>,
    table: &str,
    row: &TypedRow,
) -> Result<(), ManifestError> {
    if schemas.contains_key(table) {
        return Ok(());
    }
    let schema = TypedTableSchema {
        columns: row
            .columns
            .iter()
            .map(|(name, value)| (name.clone(), value.column_type()))
            .collect(),
        primary_key: "primary_key".into(),
        unique_keys: row.unique_keys.keys().cloned().collect(),
    };
    validate_typed_schema(&schema)?;
    schemas.insert(table.into(), schema);
    Ok(())
}

fn validate_row_against_schema(
    schema: &TypedTableSchema,
    row: &TypedRow,
) -> Result<(), ManifestError> {
    if row
        .columns
        .iter()
        .any(|(name, value)| schema.columns.get(name) != Some(&value.column_type()))
        || row.columns.len() != schema.columns.len()
    {
        return Err(error("typed row columns do not match table schema"));
    }
    let unique_keys = row.unique_keys.keys().collect::<BTreeSet<_>>();
    if unique_keys != schema.unique_keys.iter().collect::<BTreeSet<_>>() {
        return Err(error("typed row unique keys do not match table schema"));
    }
    Ok(())
}

fn validate_typed_schema_bindings(
    rows: &BTreeMap<String, BTreeMap<String, TypedRow>>,
    schemas: &BTreeMap<String, TypedTableSchema>,
) -> Result<(), ManifestError> {
    for (table, table_rows) in rows {
        if let Some(schema) = schemas.get(table) {
            validate_typed_schema(schema)?;
            for row in table_rows.values() {
                validate_row_against_schema(schema, row)?;
            }
        }
    }
    Ok(())
}

fn build_typed_row_indexes(
    tables: &BTreeMap<String, BTreeMap<String, TypedRow>>,
) -> Result<BTreeMap<String, TypedRowIndexes>, ManifestError> {
    validate_typed_rows(tables)?;
    let mut indexes = BTreeMap::new();
    for (table, rows) in tables {
        let mut index = TypedRowIndexes::default();
        for (primary_key, row) in rows {
            index.primary_keys.insert(primary_key.clone(), row.revision);
            for (name, value) in &row.unique_keys {
                let values = index.unique_keys.entry(name.clone()).or_default();
                if values.insert(value.clone(), primary_key.clone()).is_some() {
                    return Err(error(format!(
                        "duplicate typed row unique index '{name}' value"
                    )));
                }
            }
        }
        indexes.insert(table.clone(), index);
    }
    Ok(indexes)
}

fn validate_typed_row_indexes(
    tables: &BTreeMap<String, BTreeMap<String, TypedRow>>,
    indexes: &BTreeMap<String, TypedRowIndexes>,
) -> Result<(), ManifestError> {
    // Snapshots written before typed-row indexes existed remain readable. A
    // subsequent prepare rebuilds the indexes from the immutable row map.
    if indexes.is_empty() && !tables.is_empty() {
        return Ok(());
    }
    let expected = build_typed_row_indexes(tables)?;
    if &expected != indexes {
        return Err(error("typed row indexes do not match row data"));
    }
    Ok(())
}

fn validate_typed_row(row: &TypedRow) -> Result<(), ManifestError> {
    validate_identifier("typed row primary key", &row.primary_key)?;
    if row.revision == 0 {
        return Err(error("typed row revision must be positive"));
    }
    if row.columns.len() > 256 {
        return Err(error("typed row column limit exceeded"));
    }
    for (name, value) in &row.columns {
        validate_identifier("typed row column", name)?;
        if let TypedValue::String(value) = value
            && value.len() > 16 * 1024 * 1024
        {
            return Err(error("typed row string value exceeds limit"));
        }
        if let TypedValue::Json(value) = value {
            validate_typed_json(value, 0)?;
        }
    }
    if row.unique_keys.len() > MAX_UNIQUE_VALUES_PER_PRODUCT_RECORD {
        return Err(error("typed row unique-index limit exceeded"));
    }
    for (name, value) in &row.unique_keys {
        validate_identifier("typed row unique index", name)?;
        validate_identifier("typed row unique value", value)?;
    }
    Ok(())
}

const MAX_TYPED_JSON_BYTES: usize = 1024 * 1024;
const MAX_TYPED_JSON_DEPTH: usize = 16;

fn validate_typed_json(value: &serde_json::Value, depth: usize) -> Result<(), ManifestError> {
    if depth > MAX_TYPED_JSON_DEPTH {
        return Err(error("typed row JSON nesting limit exceeded"));
    }
    if serde_json::to_vec(value)
        .map_err(|_| error("typed row JSON cannot be serialized"))?
        .len()
        > MAX_TYPED_JSON_BYTES
    {
        return Err(error("typed row JSON value exceeds limit"));
    }
    match value {
        serde_json::Value::Array(values) => {
            for value in values {
                validate_typed_json(value, depth + 1)?;
            }
        }
        serde_json::Value::Object(values) => {
            for (name, value) in values {
                if name.len() > 128 {
                    return Err(error("typed row JSON key exceeds limit"));
                }
                validate_typed_json(value, depth + 1)?;
            }
        }
        serde_json::Value::Null
        | serde_json::Value::Bool(_)
        | serde_json::Value::Number(_)
        | serde_json::Value::String(_) => {}
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
    fn typed_row(id: &str, revision: u64, email: &str) -> TypedRow {
        TypedRow {
            primary_key: id.into(),
            revision,
            columns: BTreeMap::from([
                ("email".into(), TypedValue::String(email.into())),
                ("active".into(), TypedValue::Boolean(true)),
            ]),
            unique_keys: BTreeMap::from([("email".into(), email.into())]),
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
    fn typed_rows_insert_update_delete_atomically_enforce_keys_and_revisions() {
        let base = CatalogSnapshot::empty("snapshot-genesis").unwrap();
        let inserted = base
            .prepare(change(
                base.reference(),
                "typed-insert",
                DIGEST,
                vec![CatalogChange::InsertTypedRow {
                    table: "app.users".into(),
                    row: typed_row("u-1", 1, "ada@example.com"),
                }],
            ))
            .unwrap();
        assert_eq!(inserted.typed_rows["app.users"]["u-1"].revision, 1);
        assert_eq!(
            inserted.typed_row_indexes["app.users"].primary_keys["u-1"],
            1
        );
        assert_eq!(
            inserted.typed_row_indexes["app.users"].unique_keys["email"]["ada@example.com"],
            "u-1"
        );

        let updated = inserted
            .prepare(change(
                inserted.reference(),
                "typed-update",
                DIGEST,
                vec![CatalogChange::UpdateTypedRow {
                    table: "app.users".into(),
                    expected_revision: 1,
                    row: typed_row("u-1", 2, "grace@example.com"),
                }],
            ))
            .unwrap();
        assert_eq!(
            updated.typed_rows["app.users"]["u-1"].unique_keys["email"],
            "grace@example.com"
        );

        let deleted = updated
            .prepare(change(
                updated.reference(),
                "typed-delete",
                DIGEST,
                vec![CatalogChange::DeleteTypedRow {
                    table: "app.users".into(),
                    primary_key: "u-1".into(),
                    expected_revision: 2,
                }],
            ))
            .unwrap();
        assert!(deleted.typed_rows["app.users"].is_empty());

        let duplicate = base.prepare(change(
            base.reference(),
            "typed-duplicate",
            DIGEST,
            vec![
                CatalogChange::InsertTypedRow {
                    table: "app.users".into(),
                    row: typed_row("u-1", 1, "same@example.com"),
                },
                CatalogChange::InsertTypedRow {
                    table: "app.users".into(),
                    row: typed_row("u-2", 1, "same@example.com"),
                },
            ],
        ));
        assert!(duplicate.is_err());
        assert!(base.typed_rows.is_empty());
    }

    #[test]
    fn typed_row_indexes_round_trip_and_tampering_is_rejected() {
        let base = CatalogSnapshot::empty("snapshot-genesis").unwrap();
        let snapshot = base
            .prepare(change(
                base.reference(),
                "typed-index",
                DIGEST,
                vec![CatalogChange::InsertTypedRow {
                    table: "app.users".into(),
                    row: typed_row("u-1", 1, "ada@example.com"),
                }],
            ))
            .unwrap();
        let bytes = serde_json::to_vec(&snapshot).unwrap();
        let decoded: CatalogSnapshot = serde_json::from_slice(&bytes).unwrap();
        decoded.validate().unwrap();

        let mut tampered = decoded.clone();
        *tampered
            .typed_row_indexes
            .get_mut("app.users")
            .unwrap()
            .primary_keys
            .get_mut("u-1")
            .unwrap() = 99;
        assert!(tampered.validate().is_err());
    }

    #[test]
    fn typed_json_rows_round_trip_and_are_bounded() {
        let base = CatalogSnapshot::empty("snapshot-genesis").unwrap();
        let schema = TypedTableSchema {
            columns: BTreeMap::from([("document".into(), TypedColumnType::Json)]),
            primary_key: "primary_key".into(),
            unique_keys: BTreeSet::new(),
        };
        let row = TypedRow {
            primary_key: "config-1".into(),
            revision: 1,
            columns: BTreeMap::from([(
                "document".into(),
                TypedValue::Json(serde_json::json!({
                    "enabled": true,
                    "owners": ["admin", "operator"],
                    "options": {"retention_days": 30}
                })),
            )]),
            unique_keys: BTreeMap::new(),
        };
        let snapshot = base
            .prepare(change(
                base.reference(),
                "typed-json",
                DIGEST,
                vec![
                    CatalogChange::DefineTypedSchema {
                        table: "system.config".into(),
                        schema,
                    },
                    CatalogChange::InsertTypedRow {
                        table: "system.config".into(),
                        row: row.clone(),
                    },
                ],
            ))
            .unwrap();
        let bytes = serde_json::to_vec(&snapshot).unwrap();
        let decoded: CatalogSnapshot = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(decoded.typed_rows["system.config"]["config-1"], row);
        decoded.validate().unwrap();

        let mut too_deep = serde_json::Value::Null;
        for _ in 0..=MAX_TYPED_JSON_DEPTH {
            too_deep = serde_json::Value::Array(vec![too_deep]);
        }
        let invalid = TypedRow {
            primary_key: "config-2".into(),
            revision: 1,
            columns: BTreeMap::from([("document".into(), TypedValue::Json(too_deep))]),
            unique_keys: BTreeMap::new(),
        };
        assert!(
            base.prepare(change(
                base.reference(),
                "typed-json-invalid",
                DIGEST,
                vec![CatalogChange::InsertTypedRow {
                    table: "system.config".into(),
                    row: invalid,
                }],
            ))
            .is_err()
        );
    }

    #[test]
    fn explicit_typed_schema_accepts_multi_row_atomic_batch_and_rejects_type_drift() {
        let base = CatalogSnapshot::empty("snapshot-genesis").unwrap();
        let schema = TypedTableSchema {
            columns: BTreeMap::from([
                ("email".into(), TypedColumnType::String),
                ("active".into(), TypedColumnType::Boolean),
            ]),
            primary_key: "primary_key".into(),
            unique_keys: BTreeSet::from(["email".into()]),
        };
        let batch = base
            .prepare(change(
                base.reference(),
                "typed-batch",
                DIGEST,
                vec![
                    CatalogChange::DefineTypedSchema {
                        table: "app.users".into(),
                        schema: schema.clone(),
                    },
                    CatalogChange::InsertTypedRow {
                        table: "app.users".into(),
                        row: typed_row("u-1", 1, "ada@example.com"),
                    },
                    CatalogChange::InsertTypedRow {
                        table: "app.users".into(),
                        row: typed_row("u-2", 1, "grace@example.com"),
                    },
                ],
            ))
            .unwrap();
        assert_eq!(batch.typed_schemas["app.users"], schema);
        assert_eq!(batch.typed_rows["app.users"].len(), 2);

        let mut invalid = typed_row("u-3", 1, "linus@example.com");
        invalid
            .columns
            .insert("active".into(), TypedValue::Integer(1));
        assert!(
            batch
                .prepare(change(
                    batch.reference(),
                    "typed-invalid",
                    DIGEST,
                    vec![CatalogChange::InsertTypedRow {
                        table: "app.users".into(),
                        row: invalid,
                    }],
                ))
                .is_err()
        );
        assert_eq!(batch.typed_rows["app.users"].len(), 2);
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
    fn owner_principal_is_not_a_global_unique_index() {
        let base = CatalogSnapshot::empty("snapshot-genesis").unwrap();
        let mut first = product(ProductRecordKind::Dataset, "dataset-1", 1, "alice/data-1");
        let mut second = product(ProductRecordKind::Dataset, "dataset-2", 1, "alice/data-2");
        first.unique_values =
            BTreeMap::from([(String::from("owner_principal"), String::from("alice"))]);
        second.unique_values =
            BTreeMap::from([(String::from("owner_principal"), String::from("alice"))]);
        let snapshot = base
            .prepare(change(
                base.reference(),
                "multiple-owned-datasets",
                DIGEST,
                vec![
                    CatalogChange::CreateProduct { record: first },
                    CatalogChange::CreateProduct { record: second },
                ],
            ))
            .unwrap();
        assert_eq!(snapshot.product_records.len(), 2);
    }

    #[test]
    fn binding_metadata_is_not_a_global_unique_index() {
        let base = CatalogSnapshot::empty("snapshot-genesis").unwrap();
        let mut first = product(ProductRecordKind::Chart, "chart-1", 1, "alice/chart-1");
        let mut second = product(ProductRecordKind::Chart, "chart-2", 1, "alice/chart-2");
        let binding = (
            String::from("chart_dataset_revision"),
            String::from("dataset:1"),
        );
        first.unique_values = BTreeMap::from([binding.clone()]);
        second.unique_values = BTreeMap::from([binding]);
        let snapshot = base
            .prepare(change(
                base.reference(),
                "multiple-charts-per-dataset",
                DIGEST,
                vec![
                    CatalogChange::CreateProduct { record: first },
                    CatalogChange::CreateProduct { record: second },
                ],
            ))
            .unwrap();
        assert_eq!(snapshot.product_records.len(), 2);
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
    fn counts_references_by_target_not_by_referencing_record() {
        // Three recents pointing at one dashboard is one entry of three, not
        // three entries. That ratio is the whole reason the index is small
        // enough to keep in the snapshot once the records leave it.
        let mut records = BTreeMap::new();
        for index in 0..3 {
            let id = format!("recent-{index}");
            records.insert(
                product_record_key(ProductRecordKind::UserRecent, &id),
                ProductRecordRef {
                    kind: ProductRecordKind::UserRecent,
                    id,
                    revision: 1,
                    document: ImmutableFileRef {
                        path: "product/recent/document.json".into(),
                        sha256: DIGEST.into(),
                    },
                    unique_values: BTreeMap::new(),
                    references: BTreeSet::from([ProductRecordReference {
                        kind: ProductRecordKind::Dashboard,
                        id: "board".into(),
                    }]),
                },
            );
        }

        let counts = count_references(&records);
        assert_eq!(
            counts.len(),
            1,
            "one referenced target, not three referrers"
        );
        assert_eq!(
            counts[&product_record_key(ProductRecordKind::Dashboard, "board")],
            3
        );
    }

    #[test]
    fn a_reference_index_that_disagrees_with_the_records_is_refused() {
        let mut records = BTreeMap::new();
        records.insert(
            product_record_key(ProductRecordKind::UserRecent, "recent"),
            ProductRecordRef {
                kind: ProductRecordKind::UserRecent,
                id: "recent".into(),
                revision: 1,
                document: ImmutableFileRef {
                    path: "product/recent/document.json".into(),
                    sha256: DIGEST.into(),
                },
                unique_values: BTreeMap::new(),
                references: BTreeSet::from([ProductRecordReference {
                    kind: ProductRecordKind::Dashboard,
                    id: "board".into(),
                }]),
            },
        );
        records.insert(
            product_record_key(ProductRecordKind::Dashboard, "board"),
            ProductRecordRef {
                kind: ProductRecordKind::Dashboard,
                id: "board".into(),
                revision: 1,
                document: ImmutableFileRef {
                    path: "product/dashboard/document.json".into(),
                    sha256: DIGEST.into(),
                },
                unique_values: BTreeMap::new(),
                references: BTreeSet::new(),
            },
        );

        // Derived from the records: accepted.
        let honest = count_references(&records);
        assert!(validate_product_records(&records, &honest).is_ok());

        // Understating a count is the dangerous direction: it would let a
        // dashboard be deleted while something still points at it. It must not
        // pass quietly.
        let mut understated = honest.clone();
        understated.insert(product_record_key(ProductRecordKind::Dashboard, "board"), 0);
        let refused = validate_product_records(&records, &understated);
        assert!(
            refused.is_err(),
            "an index claiming nothing references the dashboard must be refused while \
the records still say otherwise"
        );
        assert!(
            refused
                .unwrap_err()
                .to_string()
                .contains("reference counts disagree"),
            "the refusal must name the disagreement rather than some other failure"
        );

        // An empty index is how every snapshot written before this field
        // existed looks, and those must still validate.
        assert!(
            validate_product_records(&records, &BTreeMap::new()).is_ok(),
            "a snapshot predating the index must not be refused for lacking it"
        );
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
