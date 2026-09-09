//! ADLS-backed transient product metadata repository.
//!
//! It reconstructs SQLite only from a pinned immutable snapshot, exports
//! changed tables as immutable Parquet, and publishes all references with one
//! catalog-head CAS. It intentionally does not implement API authorization.

use std::{collections::BTreeMap, fmt, sync::Arc};

use kaveon_storage::{AdlsConditionalCommit, CommitErrorKind};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::{
    product_commit::{CommitOutcome, ProductCatalogCommit},
    product_database::{
        ProductDatabase, ProductRuntimeRequest, ProductRuntimeResult, ProductStatement,
        ProductTable, decode_table_parquet, encode_table_parquet,
    },
    product_manifest::{
        CatalogChange, CatalogSnapshot, ImmutableFileRef, PrepareChange, TableManifestRef,
    },
    product_metrics::TransactionMetrics,
};

const MAX_OBJECT_BYTES: usize = 16 * 1024 * 1024;

#[derive(Clone)]
pub struct ProductRepository {
    storage: AdlsConditionalCommit,
    prefix: String,
    commit: ProductCatalogCommit,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RepositoryError {
    Conflict,
    Indeterminate,
    Invalid,
    Missing,
    Storage,
}
impl fmt::Display for RepositoryError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{self:?}")
    }
}
impl std::error::Error for RepositoryError {}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct StoredTable {
    name: String,
    parquet: ImmutableFileRef,
}

impl ProductRepository {
    pub fn new(storage: AdlsConditionalCommit, prefix: String) -> Result<Self, RepositoryError> {
        let prefix = prefix.trim_matches('/').to_owned();
        if prefix.is_empty() {
            return Err(RepositoryError::Invalid);
        }
        let commit = ProductCatalogCommit::new(
            storage.clone(),
            prefix.clone(),
            Arc::new(TransactionMetrics::default()),
        )
        .map_err(|_| RepositoryError::Invalid)?;
        Ok(Self {
            storage,
            prefix,
            commit,
        })
    }

    pub async fn initialize(
        &self,
        schema_sql: String,
        tables: Vec<ProductTable>,
        operation_id: String,
    ) -> Result<ProductRuntimeResult, RepositoryError> {
        let schema = self.write_schema(schema_sql).await?;
        let result = ProductDatabase::execute(ProductRuntimeRequest {
            schema_sql: self.read_utf8(&schema).await?,
            tables,
            statements: Vec::new(),
            read_only: false,
        })
        .map_err(|_| RepositoryError::Invalid)?;
        let mut snapshot = CatalogSnapshot::empty(snapshot_id(&operation_id, 0))
            .map_err(|_| RepositoryError::Invalid)?;
        snapshot.schema = Some(schema);
        snapshot.operation_id = operation_id;
        snapshot.request_digest = digest_json(&result)?;
        for table in &result.tables {
            snapshot
                .tables
                .insert(table.name.clone(), self.write_table(table).await?);
        }
        match self.commit.initialize(snapshot).await {
            CommitOutcome::Committed(_) => Ok(result),
            CommitOutcome::Conflict => Err(RepositoryError::Conflict),
            CommitOutcome::Indeterminate => Err(RepositoryError::Indeterminate),
            _ => Err(RepositoryError::Invalid),
        }
    }

    pub async fn execute(
        &self,
        statements: Vec<ProductStatement>,
        operation_id: String,
        read_only: bool,
    ) -> Result<ProductRuntimeResult, RepositoryError> {
        let current = self.commit.read_current().await.map_err(map_storage)?;
        let schema = current.schema.as_ref().ok_or(RepositoryError::Missing)?;
        let input = self.load_tables(&current).await?;
        let digest = digest_json(&statements)?;
        let result = ProductDatabase::execute(ProductRuntimeRequest {
            schema_sql: self.read_utf8(schema).await?,
            tables: input.clone(),
            statements,
            read_only,
        })
        .map_err(|_| RepositoryError::Invalid)?;
        if read_only {
            return Ok(result);
        }
        let before = input
            .into_iter()
            .map(|table| (table.name.clone(), table))
            .collect::<BTreeMap<_, _>>();
        let after = result
            .tables
            .iter()
            .map(|table| (table.name.clone(), table))
            .collect::<BTreeMap<_, _>>();
        let mut changes = Vec::new();
        for (name, table) in &after {
            if before.get(name) != Some(*table) {
                changes.push(CatalogChange::Put {
                    table: name.clone(),
                    reference: self.write_table(table).await?,
                });
            }
        }
        for name in before.keys() {
            if !after.contains_key(name) {
                changes.push(CatalogChange::Delete {
                    table: name.clone(),
                });
            }
        }
        if changes.is_empty() {
            return Ok(result);
        }
        let request = PrepareChange {
            base: current.reference(),
            snapshot_id: snapshot_id(&operation_id, current.generation + 1),
            operation_id,
            request_digest: digest,
            changes,
        };
        match self.commit.commit(request).await {
            CommitOutcome::Committed(_) | CommitOutcome::Replayed(_) => Ok(result),
            CommitOutcome::Conflict => Err(RepositoryError::Conflict),
            CommitOutcome::Indeterminate => Err(RepositoryError::Indeterminate),
            CommitOutcome::Rejected => Err(RepositoryError::Invalid),
        }
    }

    async fn load_tables(
        &self,
        snapshot: &CatalogSnapshot,
    ) -> Result<Vec<ProductTable>, RepositoryError> {
        let mut values = Vec::new();
        for reference in snapshot.tables.values() {
            let manifest: StoredTable = self.read_json(&reference.manifest).await?;
            let bytes = self.read_bytes(&manifest.parquet).await?;
            values.push(
                decode_table_parquet(manifest.name, bytes).map_err(|_| RepositoryError::Invalid)?,
            );
        }
        Ok(values)
    }
    async fn write_schema(&self, schema_sql: String) -> Result<ImmutableFileRef, RepositoryError> {
        self.write_immutable("schemas", "sql", schema_sql.into_bytes())
            .await
    }
    async fn write_table(&self, table: &ProductTable) -> Result<TableManifestRef, RepositoryError> {
        let parquet = self
            .write_immutable(
                "parquet",
                "parquet",
                encode_table_parquet(table).map_err(|_| RepositoryError::Invalid)?,
            )
            .await?;
        let manifest = StoredTable {
            name: table.name.clone(),
            parquet: parquet.clone(),
        };
        let manifest = self
            .write_immutable(
                "table-manifests",
                "json",
                serde_json::to_vec(&manifest).map_err(|_| RepositoryError::Invalid)?,
            )
            .await?;
        Ok(TableManifestRef {
            manifest,
            parquet_files: vec![parquet],
        })
    }
    async fn write_immutable(
        &self,
        kind: &str,
        extension: &str,
        bytes: Vec<u8>,
    ) -> Result<ImmutableFileRef, RepositoryError> {
        if bytes.len() > MAX_OBJECT_BYTES {
            return Err(RepositoryError::Invalid);
        }
        let sha256 = digest(&bytes);
        let path = format!("{}/objects/{kind}/{sha256}.{extension}", self.prefix);
        match self.storage.create_immutable(&path, bytes).await {
            Ok(_) => {}
            Err(error) if error.kind == CommitErrorKind::Conflict => {}
            Err(error) => return Err(map_storage(error.kind)),
        }
        Ok(ImmutableFileRef {
            path: path
                .trim_start_matches(&(self.prefix.clone() + "/"))
                .to_owned(),
            sha256,
        })
    }
    async fn read_utf8(&self, reference: &ImmutableFileRef) -> Result<String, RepositoryError> {
        String::from_utf8(self.read_bytes(reference).await?).map_err(|_| RepositoryError::Invalid)
    }
    async fn read_json<T: for<'a> Deserialize<'a>>(
        &self,
        reference: &ImmutableFileRef,
    ) -> Result<T, RepositoryError> {
        serde_json::from_slice(&self.read_bytes(reference).await?)
            .map_err(|_| RepositoryError::Invalid)
    }
    async fn read_bytes(&self, reference: &ImmutableFileRef) -> Result<Vec<u8>, RepositoryError> {
        let bytes = self
            .storage
            .read_bounded(
                &format!("{}/{}", self.prefix, reference.path),
                MAX_OBJECT_BYTES,
            )
            .await
            .map_err(|error| map_storage(error.kind))?
            .bytes;
        if digest(&bytes) != reference.sha256 {
            return Err(RepositoryError::Invalid);
        }
        Ok(bytes)
    }
}
fn digest(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}
fn digest_json<T: Serialize>(value: &T) -> Result<String, RepositoryError> {
    serde_json::to_vec(value)
        .map(|bytes| digest(&bytes))
        .map_err(|_| RepositoryError::Invalid)
}
fn snapshot_id(operation: &str, generation: u64) -> String {
    format!("s{}{}", generation, &digest(operation.as_bytes())[..32])
}
fn map_storage(kind: CommitErrorKind) -> RepositoryError {
    match kind {
        CommitErrorKind::Conflict => RepositoryError::Conflict,
        CommitErrorKind::Missing => RepositoryError::Missing,
        CommitErrorKind::Retryable
        | CommitErrorKind::Other
        | CommitErrorKind::Unsupported
        | CommitErrorKind::LimitExceeded => RepositoryError::Indeterminate,
        _ => RepositoryError::Storage,
    }
}
