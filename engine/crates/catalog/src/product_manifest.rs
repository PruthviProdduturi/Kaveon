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
            }
        }
        if tables.len() > MAX_TABLES {
            return Err(error("snapshot table limit exceeded"));
        }
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
        for (table, reference) in &self.tables {
            validate_table_name(table)?;
            validate_table_reference(reference)?;
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
    let mut names = BTreeSet::new();
    for change in &request.changes {
        let table = match change {
            CatalogChange::Put { table, reference } => {
                validate_table_reference(reference)?;
                table
            }
            CatalogChange::Delete { table } => table,
        };
        validate_table_name(table)?;
        if !names.insert(table) {
            return Err(error(format!("table '{table}' is changed more than once")));
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
