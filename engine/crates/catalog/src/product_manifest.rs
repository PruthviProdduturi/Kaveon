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
pub struct CatalogSnapshot {
    pub version: u32,
    pub generation: u64,
    pub snapshot_id: String,
    pub parent: Option<SnapshotRef>,
    pub operation_id: String,
    pub request_digest: String,
    pub tables: BTreeMap<String, TableManifestRef>,
    /// Immutable product-control documents (datasets, charts, dashboards, and
    /// similar metadata) published under the same atomic snapshot head.
    #[serde(default)]
    pub control_records: BTreeMap<String, ImmutableFileRef>,
    #[serde(default)]
    pub product_records: BTreeMap<String, ProductRecordRef>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CatalogChange {
    Put {
        table: String,
        reference: TableManifestRef,
    },
    Delete {
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
        let mut control_records = self.control_records.clone();
        let mut product_records = self.product_records.clone();
        for change in request.changes {
            match change {
                CatalogChange::Put { table, reference } => {
                    tables.insert(table, reference);
                }
                CatalogChange::Delete { table } => {
                    if tables.remove(&table).is_none() {
                        return Err(error(format!("cannot delete absent table '{table}'")));
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
}

impl ProductRecordKind {
    fn name(self) -> &'static str {
        match self {
            Self::Dataset => "dataset",
            Self::Chart => "chart",
            Self::Dashboard => "dashboard",
            Self::SavedQuery => "saved_query",
            Self::UserTheme => "user_theme",
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
}

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
    for (name, value) in &record.unique_values {
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
    }
    Ok(())
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
}
