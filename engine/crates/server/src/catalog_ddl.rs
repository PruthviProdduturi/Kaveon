//! Catalog statements on the coordinator. `CREATE`, `DROP` and `ALTER`
//! lower onto the same durable definitions the `/v1/catalog` API manages —
//! the same identifiers, revisions, lifecycle and audit trail — and every
//! mutation republishes the planning snapshot. `SHOW` and `DESCRIBE` read
//! the published snapshot, which is what queries see.
//!
//! A table is registered in three steps: its definition is created as a
//! `Draft`, the location is probed through the storage layer (metadata
//! only: the Delta log, the Iceberg metadata pointer, or the Parquet
//! footers), and only a readable location is activated. A probe failure
//! deletes the draft and reports the storage error, so nothing is left
//! half-registered.

use crate::AppState;
use crate::api::ColumnInfo;
use crate::security::{Identity, Role};
use axum::http::StatusCode;
use kaveon_catalog::{CascadePolicy, CatalogStore};
use kaveon_core::{
    AccessPattern, CatalogAdapter, CatalogDefinition, CatalogId, CatalogLifecycle,
    ColumnDefinition, CredentialKind, CredentialReference, DataFormat, KaveonError, ResolvedTable,
    SchemaDefinition, SchemaId, StorageType, TableDefinition, TableId, TableMeta,
};
use kaveon_sql::ddl::{
    CatalogStatement, CatalogStorageSpec, ColumnSpec, CredentialSpec, QualifiedName, format_name,
    quote_identifier, render_create_table, sql_type_name,
};
use serde_json::{Value, json};
use std::sync::Arc;

/// The small result a catalog statement returns, like any statement.
pub(crate) struct CatalogStatementResult {
    pub(crate) columns: Vec<ColumnInfo>,
    pub(crate) rows: Vec<Vec<Value>>,
}

/// A refused or failed catalog statement: the HTTP status, a stable code
/// and a message naming what is wrong.
#[derive(Debug)]
pub(crate) struct CatalogStatementError {
    pub(crate) status: StatusCode,
    pub(crate) code: &'static str,
    pub(crate) message: String,
}

impl CatalogStatementError {
    fn forbidden(message: impl Into<String>) -> Self {
        Self {
            status: StatusCode::FORBIDDEN,
            code: "FORBIDDEN",
            message: message.into(),
        }
    }
    fn not_found(code: &'static str, message: impl Into<String>) -> Self {
        Self {
            status: StatusCode::BAD_REQUEST,
            code,
            message: message.into(),
        }
    }
    fn conflict(message: impl Into<String>) -> Self {
        Self {
            status: StatusCode::CONFLICT,
            code: "CATALOG_CONFLICT",
            message: message.into(),
        }
    }
    fn invalid(message: impl Into<String>) -> Self {
        Self {
            status: StatusCode::BAD_REQUEST,
            code: "CATALOG_INVALID",
            message: message.into(),
        }
    }
    fn unreadable(message: impl Into<String>) -> Self {
        Self {
            status: StatusCode::BAD_REQUEST,
            code: "TABLE_NOT_READABLE",
            message: message.into(),
        }
    }
    fn store(error: KaveonError) -> Self {
        let message = error.to_string();
        if message.contains("already exists") {
            return Self::conflict(message);
        }
        if message.contains("revision conflict") {
            return Self::conflict(format!(
                "{message}; the definition changed while the statement ran, retry"
            ));
        }
        if message.contains("not empty") {
            return Self::conflict(message);
        }
        if message.contains("not found") {
            return Self::not_found("CATALOG_NOT_FOUND", message);
        }
        if message.starts_with("catalog: database") || message.starts_with("catalog: metadata") {
            return Self {
                status: StatusCode::INTERNAL_SERVER_ERROR,
                code: "CATALOG_UNAVAILABLE",
                message,
            };
        }
        Self::invalid(message)
    }
    fn publish(error: anyhow::Error) -> Self {
        Self {
            status: StatusCode::INTERNAL_SERVER_ERROR,
            code: "CATALOG_UNAVAILABLE",
            message: format!("catalog snapshot publication failed: {error}"),
        }
    }
}

type DdlResult<T> = Result<T, CatalogStatementError>;

/// Run one catalog statement for `identity` with the request's session
/// catalog and schema as the defaults for unqualified names.
pub(crate) async fn execute_catalog_statement(
    state: &Arc<AppState>,
    identity: &Identity,
    context_catalog: &str,
    context_schema: &str,
    statement: CatalogStatement,
) -> DdlResult<CatalogStatementResult> {
    if statement.is_catalog_mutation() && identity.role != Role::Admin {
        return Err(CatalogStatementError::forbidden(
            "CREATE CATALOG and DROP CATALOG require the admin role",
        ));
    }
    if statement.is_mutation() && identity.role == Role::Reader {
        return Err(CatalogStatementError::forbidden(
            "catalog changes require the analyst or admin role",
        ));
    }
    let actor = identity.principal.as_str();
    let store = &state.catalog_store;
    let result = match statement {
        CatalogStatement::CreateCatalog {
            name,
            if_not_exists,
            storage,
            credential,
        } => create_catalog(store, actor, name, if_not_exists, storage, credential)?,
        CatalogStatement::DropCatalog {
            name,
            if_exists,
            cascade,
        } => drop_catalog(store, actor, name, if_exists, cascade)?,
        CatalogStatement::CreateSchema {
            name,
            if_not_exists,
        } => {
            let (catalog, schema) = schema_target(&name, context_catalog);
            create_schema(store, actor, catalog, schema, if_not_exists)?
        }
        CatalogStatement::DropSchema {
            name,
            if_exists,
            cascade,
        } => {
            let (catalog, schema) = schema_target(&name, context_catalog);
            drop_schema(store, actor, catalog, schema, if_exists, cascade)?
        }
        CatalogStatement::CreateTable {
            name,
            if_not_exists,
            columns,
            location,
            format,
            access,
            partitioned_by,
        } => {
            let target = table_target(&name, context_catalog, context_schema);
            create_table(
                store,
                actor,
                target,
                if_not_exists,
                columns,
                location,
                format,
                access,
                partitioned_by,
            )
            .await?
        }
        CatalogStatement::DropTable { name, if_exists } => {
            let target = table_target(&name, context_catalog, context_schema);
            drop_table(store, actor, target, if_exists)?
        }
        CatalogStatement::AlterTableSetLocation {
            name,
            if_exists,
            location,
        } => {
            let target = table_target(&name, context_catalog, context_schema);
            set_table_location(store, actor, target, if_exists, location).await?
        }
        CatalogStatement::ShowCreateTable { name } => {
            let target = table_target(&name, context_catalog, context_schema);
            return show_create_table(store, target);
        }
        CatalogStatement::Describe { name } => {
            let target = table_target(&name, context_catalog, context_schema);
            return describe(state, target).await;
        }
        CatalogStatement::ShowCatalogs { like } => return show_catalogs(state, like).await,
        CatalogStatement::ShowSchemas { catalog, like } => {
            let catalog = catalog.unwrap_or_else(|| context_catalog.to_owned());
            return show_schemas(state, catalog, like).await;
        }
        CatalogStatement::ShowTables { schema, like } => {
            let (catalog, schema) = match schema {
                Some(name) => schema_target(&name, context_catalog),
                None => (context_catalog.to_owned(), context_schema.to_owned()),
            };
            return show_tables(state, catalog, schema, like).await;
        }
    };
    crate::api::publish_catalog_snapshot(state)
        .await
        .map_err(CatalogStatementError::publish)?;
    Ok(result)
}

// --- name resolution ---

struct TableTarget {
    catalog: String,
    schema: String,
    table: String,
}

impl TableTarget {
    fn qualified(&self) -> String {
        QualifiedName(vec![
            self.catalog.clone(),
            self.schema.clone(),
            self.table.clone(),
        ])
        .to_string()
    }
}

fn schema_target(name: &QualifiedName, context_catalog: &str) -> (String, String) {
    match name.parts() {
        [schema] => (context_catalog.to_owned(), schema.clone()),
        [catalog, schema] => (catalog.clone(), schema.clone()),
        parts => (
            context_catalog.to_owned(),
            parts.last().cloned().unwrap_or_default(),
        ),
    }
}

fn table_target(name: &QualifiedName, context_catalog: &str, context_schema: &str) -> TableTarget {
    match name.parts() {
        [table] => TableTarget {
            catalog: context_catalog.to_owned(),
            schema: context_schema.to_owned(),
            table: table.clone(),
        },
        [schema, table] => TableTarget {
            catalog: context_catalog.to_owned(),
            schema: schema.clone(),
            table: table.clone(),
        },
        [catalog, schema, table] => TableTarget {
            catalog: catalog.clone(),
            schema: schema.clone(),
            table: table.clone(),
        },
        parts => TableTarget {
            catalog: context_catalog.to_owned(),
            schema: context_schema.to_owned(),
            table: parts.last().cloned().unwrap_or_default(),
        },
    }
}

fn qualified_schema(catalog: &str, schema: &str) -> String {
    QualifiedName(vec![catalog.to_owned(), schema.to_owned()]).to_string()
}

// --- durable lookups ---

fn require_catalog(store: &CatalogStore, name: &str) -> DdlResult<CatalogDefinition> {
    store
        .catalog_by_name(name)
        .map_err(CatalogStatementError::store)?
        .ok_or_else(|| {
            CatalogStatementError::not_found(
                "CATALOG_NOT_FOUND",
                format!("catalog '{name}' not found"),
            )
        })
}

fn find_schema(
    store: &CatalogStore,
    catalog: &CatalogDefinition,
    name: &str,
) -> DdlResult<Option<SchemaDefinition>> {
    Ok(store
        .list_schemas(catalog.id())
        .map_err(CatalogStatementError::store)?
        .into_iter()
        .find(|schema| schema.name() == name))
}

fn require_schema(
    store: &CatalogStore,
    catalog: &CatalogDefinition,
    name: &str,
) -> DdlResult<SchemaDefinition> {
    find_schema(store, catalog, name)?.ok_or_else(|| {
        CatalogStatementError::not_found(
            "SCHEMA_NOT_FOUND",
            format!(
                "schema {} not found",
                qualified_schema(catalog.name(), name)
            ),
        )
    })
}

fn find_table(
    store: &CatalogStore,
    schema: &SchemaDefinition,
    name: &str,
) -> DdlResult<Option<TableDefinition>> {
    Ok(store
        .list_tables(schema.id())
        .map_err(CatalogStatementError::store)?
        .into_iter()
        .find(|table| table.name() == name))
}

/// The catalog, schema and table definitions a table statement names, or
/// `None` for the table when it does not exist (its parents must).
fn locate_table(
    store: &CatalogStore,
    target: &TableTarget,
) -> DdlResult<(CatalogDefinition, SchemaDefinition, Option<TableDefinition>)> {
    let catalog = require_catalog(store, &target.catalog)?;
    let schema = require_schema(store, &catalog, &target.schema)?;
    let table = find_table(store, &schema, &target.table)?;
    Ok((catalog, schema, table))
}

/// A stable identifier in the bootstrap convention (`catalog:name`,
/// `schema:catalog:name`, `table:catalog:schema:name`); when that identifier
/// belongs to another object (a definition registered through the HTTP API
/// with the same identifier and a different name), a random suffix keeps
/// the new one distinct.
fn unique_id<T>(
    canonical: String,
    existing: impl Fn(&str) -> kaveon_core::Result<Option<T>>,
) -> DdlResult<String> {
    if existing(&canonical)
        .map_err(CatalogStatementError::store)?
        .is_none()
    {
        return Ok(canonical);
    }
    let suffix = uuid::Uuid::new_v4().simple().to_string();
    Ok(format!("{canonical}:{}", &suffix[..8]))
}

// --- results ---

fn outcome(kind: &str, object: String, result: &str) -> CatalogStatementResult {
    CatalogStatementResult {
        columns: vec![
            ColumnInfo {
                name: kind.to_owned(),
                data_type: "Utf8".into(),
            },
            ColumnInfo {
                name: "result".into(),
                data_type: "Utf8".into(),
            },
        ],
        rows: vec![vec![json!(object), json!(result)]],
    }
}

fn names(header: &str, mut values: Vec<String>, like: Option<&str>) -> CatalogStatementResult {
    values.sort();
    values.dedup();
    CatalogStatementResult {
        columns: vec![ColumnInfo {
            name: header.to_owned(),
            data_type: "Utf8".into(),
        }],
        rows: values
            .into_iter()
            .filter(|value| like.is_none_or(|pattern| sql_like(value, pattern)))
            .map(|value| vec![json!(value)])
            .collect(),
    }
}

/// SQL `LIKE` over a name: `%` any run, `_` one character, case-sensitive.
fn sql_like(value: &str, pattern: &str) -> bool {
    let value: Vec<char> = value.chars().collect();
    let pattern: Vec<char> = pattern.chars().collect();
    let mut previous = vec![false; value.len() + 1];
    previous[0] = true;
    for character in pattern {
        let mut current = vec![false; value.len() + 1];
        match character {
            '%' => {
                current[0] = previous[0];
                for index in 1..=value.len() {
                    current[index] = previous[index] || current[index - 1];
                }
            }
            '_' => current[1..].copy_from_slice(&previous[..value.len()]),
            character => {
                for index in 1..=value.len() {
                    current[index] = previous[index - 1] && value[index - 1] == character;
                }
            }
        }
        previous = current;
    }
    previous[value.len()]
}

// --- catalogs ---

fn create_catalog(
    store: &CatalogStore,
    actor: &str,
    name: String,
    if_not_exists: bool,
    storage: CatalogStorageSpec,
    credential: Option<CredentialSpec>,
) -> DdlResult<CatalogStatementResult> {
    if store
        .catalog_by_name(&name)
        .map_err(CatalogStatementError::store)?
        .is_some()
    {
        if if_not_exists {
            return Ok(outcome("catalog", quote_identifier(&name), "exists"));
        }
        return Err(CatalogStatementError::conflict(format!(
            "catalog '{name}' already exists"
        )));
    }
    let storage = match storage {
        CatalogStorageSpec::Local { base_path } => {
            let path = std::path::PathBuf::from(&base_path);
            if !path.is_absolute() {
                return Err(CatalogStatementError::invalid(format!(
                    "base_path '{base_path}' must be an absolute path on the coordinator"
                )));
            }
            if !path.is_dir() {
                return Err(CatalogStatementError::unreadable(format!(
                    "base_path '{base_path}' is not a directory on the coordinator"
                )));
            }
            StorageType::Local { base_path: path }
        }
        CatalogStorageSpec::AdlsGen2 {
            account,
            container,
            root_path,
        } => StorageType::AdlsGen2 {
            account,
            container,
            root_path,
        },
        CatalogStorageSpec::S3 {
            bucket,
            region,
            prefix,
        } => StorageType::S3 {
            bucket,
            region,
            prefix,
        },
    };
    let id = unique_id(format!("catalog:{name}"), |id| {
        store.catalog(&CatalogId::new(id)?)
    })?;
    let mut definition = CatalogDefinition::new(
        CatalogId::new(id).map_err(CatalogStatementError::invalid_error)?,
        &name,
        CatalogAdapter::Native,
        storage,
    )
    .map_err(CatalogStatementError::invalid_error)?;
    if let Some(credential) = credential {
        let kind = match credential.kind.as_str() {
            "managed-identity" | "managed_identity" => CredentialKind::ManagedIdentity,
            "workload-identity" | "workload_identity" => CredentialKind::WorkloadIdentity,
            "environment" => CredentialKind::Environment,
            "secret-store" | "secret_store" => CredentialKind::SecretStore,
            other => {
                return Err(CatalogStatementError::invalid(format!(
                    "credential kind '{other}' is not supported; use managed-identity, workload-identity, environment or secret-store"
                )));
            }
        };
        definition = definition.with_credential(
            CredentialReference::new(kind, credential.reference)
                .map_err(CatalogStatementError::invalid_error)?,
        );
    }
    let active = definition
        .transition(CatalogLifecycle::Active)
        .map_err(CatalogStatementError::invalid_error)?;
    store
        .create_catalog(actor, &active)
        .map_err(CatalogStatementError::store)?;
    Ok(outcome("catalog", quote_identifier(&name), "created"))
}

fn drop_catalog(
    store: &CatalogStore,
    actor: &str,
    name: String,
    if_exists: bool,
    cascade: bool,
) -> DdlResult<CatalogStatementResult> {
    let Some(catalog) = store
        .catalog_by_name(&name)
        .map_err(CatalogStatementError::store)?
    else {
        if if_exists {
            return Ok(outcome("catalog", quote_identifier(&name), "absent"));
        }
        return Err(CatalogStatementError::not_found(
            "CATALOG_NOT_FOUND",
            format!("catalog '{name}' not found"),
        ));
    };
    let policy = if cascade {
        CascadePolicy::Cascade
    } else {
        CascadePolicy::Restrict
    };
    store
        .delete_catalog(actor, catalog.id(), catalog.revision(), policy)
        .map_err(|error| {
            if error.to_string().contains("not empty") {
                CatalogStatementError::conflict(format!(
                    "catalog '{name}' is not empty; drop its schemas first or use DROP CATALOG {} CASCADE",
                    quote_identifier(&name)
                ))
            } else {
                CatalogStatementError::store(error)
            }
        })?;
    Ok(outcome("catalog", quote_identifier(&name), "dropped"))
}

// --- schemas ---

fn create_schema(
    store: &CatalogStore,
    actor: &str,
    catalog_name: String,
    schema_name: String,
    if_not_exists: bool,
) -> DdlResult<CatalogStatementResult> {
    let catalog = require_catalog(store, &catalog_name)?;
    let qualified = qualified_schema(&catalog_name, &schema_name);
    if find_schema(store, &catalog, &schema_name)?.is_some() {
        if if_not_exists {
            return Ok(outcome("schema", qualified, "exists"));
        }
        return Err(CatalogStatementError::conflict(format!(
            "schema {qualified} already exists"
        )));
    }
    let id = unique_id(format!("schema:{catalog_name}:{schema_name}"), |id| {
        store.schema(&SchemaId::new(id)?)
    })?;
    let definition = SchemaDefinition::new(
        SchemaId::new(id).map_err(CatalogStatementError::invalid_error)?,
        catalog.id().clone(),
        &schema_name,
    )
    .map_err(CatalogStatementError::invalid_error)?
    .transition(CatalogLifecycle::Active)
    .map_err(CatalogStatementError::invalid_error)?;
    store
        .create_schema(actor, &definition)
        .map_err(CatalogStatementError::store)?;
    Ok(outcome("schema", qualified, "created"))
}

fn drop_schema(
    store: &CatalogStore,
    actor: &str,
    catalog_name: String,
    schema_name: String,
    if_exists: bool,
    cascade: bool,
) -> DdlResult<CatalogStatementResult> {
    let qualified = qualified_schema(&catalog_name, &schema_name);
    let catalog = match store
        .catalog_by_name(&catalog_name)
        .map_err(CatalogStatementError::store)?
    {
        Some(catalog) => catalog,
        None if if_exists => return Ok(outcome("schema", qualified, "absent")),
        None => return Err(require_catalog(store, &catalog_name).unwrap_err()),
    };
    let Some(schema) = find_schema(store, &catalog, &schema_name)? else {
        if if_exists {
            return Ok(outcome("schema", qualified, "absent"));
        }
        return Err(CatalogStatementError::not_found(
            "SCHEMA_NOT_FOUND",
            format!("schema {qualified} not found"),
        ));
    };
    let policy = if cascade {
        CascadePolicy::Cascade
    } else {
        CascadePolicy::Restrict
    };
    store
        .delete_schema(actor, schema.id(), schema.revision(), policy)
        .map_err(|error| {
            if error.to_string().contains("not empty") {
                CatalogStatementError::conflict(format!(
                    "schema {qualified} is not empty; drop its tables first or use DROP SCHEMA {qualified} CASCADE"
                ))
            } else {
                CatalogStatementError::store(error)
            }
        })?;
    Ok(outcome("schema", qualified, "dropped"))
}

// --- tables ---

/// The source's schema and row count after a metadata-only read of the
/// location, or the storage error naming why it cannot be read.
async fn probe_location(
    catalog: &CatalogDefinition,
    schema_name: &str,
    table_name: &str,
    location: &str,
    format: DataFormat,
) -> DdlResult<kaveon_storage::SourceStatistics> {
    let resolved = ResolvedTable {
        catalog: catalog.name().to_owned(),
        schema: schema_name.to_owned(),
        table: Arc::new(TableMeta {
            name: table_name.to_owned(),
            arrow_schema: Arc::new(arrow::datatypes::Schema::empty()),
            location: location.to_owned(),
            access: AccessPattern::Shortcut,
            format,
        }),
        storage: catalog.storage().clone(),
    };
    let full_path = resolved.full_path();
    let probed =
        tokio::task::spawn_blocking(move || kaveon_storage::analyze_source(&full_path, format))
            .await
            .map_err(|error| {
                CatalogStatementError::unreadable(format!(
                    "location probe did not complete: {error}"
                ))
            })?;
    probed.map_err(|error| {
        CatalogStatementError::unreadable(format!(
            "location '{location}' is not readable as {}: {error}",
            format_name(format)
        ))
    })
}

/// The columns a table stores: the declared ones, each checked to exist in
/// the source, or the source's own schema.
fn resolve_columns(
    declared: Option<Vec<ColumnSpec>>,
    source: &arrow::datatypes::Schema,
    location: &str,
) -> DdlResult<Vec<ColumnDefinition>> {
    let columns = match declared {
        Some(declared) => {
            for column in &declared {
                if source.field_with_name(&column.name).is_err() {
                    let available = source
                        .fields()
                        .iter()
                        .map(|field| field.name().as_str())
                        .collect::<Vec<_>>()
                        .join(", ");
                    return Err(CatalogStatementError::unreadable(format!(
                        "column '{}' is not in the table at '{location}'; it has: {available}",
                        column.name
                    )));
                }
            }
            declared
                .into_iter()
                .map(|column| ColumnDefinition::new(column.name, column.data_type, column.nullable))
                .collect::<kaveon_core::Result<Vec<_>>>()
        }
        None => source
            .fields()
            .iter()
            .map(|field| {
                ColumnDefinition::new(field.name(), field.data_type().clone(), field.is_nullable())
            })
            .collect::<kaveon_core::Result<Vec<_>>>(),
    };
    columns.map_err(CatalogStatementError::invalid_error)
}

/// The partition columns a table definition records: the keys the probe
/// discovered in the location's paths, in path order, each typed as the
/// table's column of that name. A declaration must name exactly those keys
/// in that order; a declaration over a location without keys, or keys
/// under no declaration, are both taken from the location.
fn resolve_partitions(
    declared: Option<&[String]>,
    probed: &kaveon_storage::SourceStatistics,
    columns: &[ColumnDefinition],
    location: &str,
) -> DdlResult<Vec<kaveon_core::PartitionColumn>> {
    let discovered = probed
        .parquet_listing
        .as_ref()
        .map(|listing| {
            listing
                .partitions
                .iter()
                .map(|column| column.name().to_owned())
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    if let Some(declared) = declared
        && declared != discovered.as_slice()
    {
        let describe = |keys: &[String]| {
            if keys.is_empty() {
                "no key=value directories".to_owned()
            } else {
                keys.join("/")
            }
        };
        return Err(CatalogStatementError::unreadable(format!(
            "partitioned_by declares {} but the files at '{location}' lie under {}; the \
             declaration must name the path's keys in their order",
            describe(declared),
            describe(&discovered)
        )));
    }
    discovered
        .iter()
        .map(|key| {
            let column = columns
                .iter()
                .find(|column| column.name() == key)
                .ok_or_else(|| {
                    CatalogStatementError::invalid(format!(
                        "partition column '{key}' of '{location}' is not among the table's \
                         columns; declare it in the column list or omit the list"
                    ))
                })?;
            kaveon_core::PartitionColumn::new(key, column.data_type().clone())
                .map_err(CatalogStatementError::invalid_error)
        })
        .collect()
}

#[allow(clippy::too_many_arguments)]
async fn create_table(
    store: &CatalogStore,
    actor: &str,
    target: TableTarget,
    if_not_exists: bool,
    columns: Option<Vec<ColumnSpec>>,
    location: String,
    format: DataFormat,
    access: AccessPattern,
    partitioned_by: Option<Vec<String>>,
) -> DdlResult<CatalogStatementResult> {
    let qualified = target.qualified();
    let (catalog, schema, existing) = locate_table(store, &target)?;
    if let Some(existing) = existing {
        if if_not_exists {
            return Ok(outcome("table", qualified, "exists"));
        }
        return Err(CatalogStatementError::conflict(format!(
            "table {qualified} already exists ({:?}, revision {})",
            existing.lifecycle(),
            existing.revision().value()
        )));
    }
    // The draft reserves the name; the probe decides whether it becomes
    // active. Declared columns are stored as declared once the probe finds
    // them in the source.
    let id = unique_id(
        format!(
            "table:{}:{}:{}",
            target.catalog, target.schema, target.table
        ),
        |id| store.table(&TableId::new(id)?),
    )?;
    let placeholder = match &columns {
        Some(declared) => declared
            .iter()
            .map(|column| {
                ColumnDefinition::new(
                    column.name.clone(),
                    column.data_type.clone(),
                    column.nullable,
                )
            })
            .collect::<kaveon_core::Result<Vec<_>>>()
            .map_err(CatalogStatementError::invalid_error)?,
        None => vec![
            ColumnDefinition::new("__kaveon_pending", arrow::datatypes::DataType::Null, true)
                .map_err(CatalogStatementError::invalid_error)?,
        ],
    };
    let draft = TableDefinition::new(
        TableId::new(id).map_err(CatalogStatementError::invalid_error)?,
        schema.id().clone(),
        &target.table,
        &location,
        access,
        format,
        placeholder,
    )
    .map_err(CatalogStatementError::invalid_error)?;
    store
        .create_table(actor, &draft)
        .map_err(CatalogStatementError::store)?;

    let activated = async {
        let probed =
            probe_location(&catalog, &target.schema, &target.table, &location, format).await?;
        let columns = resolve_columns(columns, &probed.schema, &location)?;
        let partitions =
            resolve_partitions(partitioned_by.as_deref(), &probed, &columns, &location)?;
        let active = TableDefinition::new(
            draft.id().clone(),
            schema.id().clone(),
            &target.table,
            &location,
            access,
            format,
            columns,
        )
        .map_err(CatalogStatementError::invalid_error)?
        .partitioned_by(partitions)
        .map_err(CatalogStatementError::invalid_error)?
        .transition(CatalogLifecycle::Active)
        .map_err(CatalogStatementError::invalid_error)?;
        store
            .replace_table(actor, draft.revision(), &active)
            .map_err(CatalogStatementError::store)
    }
    .await;
    if let Err(error) = activated {
        // Nothing half-registered: the draft goes with the failure. A draft
        // that cannot be removed is reported with the cause.
        if let Err(cleanup) = store.delete_table(actor, draft.id(), draft.revision()) {
            return Err(CatalogStatementError {
                status: error.status,
                code: error.code,
                message: format!(
                    "{}; the draft definition '{}' could not be removed: {cleanup}",
                    error.message,
                    draft.id().as_str()
                ),
            });
        }
        return Err(error);
    }
    Ok(outcome("table", qualified, "created"))
}

fn drop_table(
    store: &CatalogStore,
    actor: &str,
    target: TableTarget,
    if_exists: bool,
) -> DdlResult<CatalogStatementResult> {
    let qualified = target.qualified();
    let located = match locate_table(store, &target) {
        Ok(located) => located,
        Err(error) if if_exists && error.code.ends_with("_NOT_FOUND") => {
            return Ok(outcome("table", qualified, "absent"));
        }
        Err(error) => return Err(error),
    };
    let Some(table) = located.2 else {
        if if_exists {
            return Ok(outcome("table", qualified, "absent"));
        }
        return Err(CatalogStatementError::not_found(
            "TABLE_NOT_FOUND",
            format!("table {qualified} not found"),
        ));
    };
    store
        .delete_table(actor, table.id(), table.revision())
        .map_err(CatalogStatementError::store)?;
    Ok(outcome("table", qualified, "dropped"))
}

async fn set_table_location(
    store: &CatalogStore,
    actor: &str,
    target: TableTarget,
    if_exists: bool,
    location: String,
) -> DdlResult<CatalogStatementResult> {
    let qualified = target.qualified();
    let located = match locate_table(store, &target) {
        Ok(located) => located,
        Err(error) if if_exists && error.code.ends_with("_NOT_FOUND") => {
            return Ok(outcome("table", qualified, "absent"));
        }
        Err(error) => return Err(error),
    };
    let (catalog, _, table) = located;
    let Some(table) = table else {
        if if_exists {
            return Ok(outcome("table", qualified, "absent"));
        }
        return Err(CatalogStatementError::not_found(
            "TABLE_NOT_FOUND",
            format!("table {qualified} not found"),
        ));
    };
    if table.location() == location {
        return Ok(outcome("table", qualified, "unchanged"));
    }
    // The stored columns must all be present at the new location; a table
    // that moved to a different shape is a new table.
    let probed = probe_location(
        &catalog,
        &target.schema,
        &target.table,
        &location,
        table.format(),
    )
    .await?;
    let declared = table
        .columns()
        .iter()
        .map(|column| ColumnSpec {
            name: column.name().to_owned(),
            data_type: column.data_type().clone(),
            nullable: column.nullable(),
        })
        .collect();
    resolve_columns(Some(declared), &probed.schema, &location)?;
    // A partitioned table stays partitioned the same way: the new
    // location's keys must be the stored ones.
    let stored_keys = table
        .partitions()
        .iter()
        .map(|column| column.name().to_owned())
        .collect::<Vec<_>>();
    resolve_partitions(Some(&stored_keys), &probed, table.columns(), &location)?;
    let relocated = table
        .with_location(&location)
        .map_err(CatalogStatementError::invalid_error)?;
    store
        .replace_table(actor, table.revision(), &relocated)
        .map_err(CatalogStatementError::store)?;
    Ok(outcome("table", qualified, "relocated"))
}

// --- metadata reads ---

fn show_create_table(
    store: &CatalogStore,
    target: TableTarget,
) -> DdlResult<CatalogStatementResult> {
    let qualified = target.qualified();
    let (_, _, table) = locate_table(store, &target)?;
    let Some(table) = table else {
        return Err(CatalogStatementError::not_found(
            "TABLE_NOT_FOUND",
            format!("table {qualified} not found"),
        ));
    };
    let columns = table
        .columns()
        .iter()
        .map(|column| ColumnSpec {
            name: column.name().to_owned(),
            data_type: column.data_type().clone(),
            nullable: column.nullable(),
        })
        .collect::<Vec<_>>();
    let partitioned_by = table
        .partitions()
        .iter()
        .map(|column| column.name().to_owned())
        .collect::<Vec<_>>();
    let statement = render_create_table(
        &QualifiedName(vec![target.catalog, target.schema, target.table]),
        &columns,
        table.location(),
        table.format(),
        table.access(),
        &partitioned_by,
    );
    Ok(CatalogStatementResult {
        columns: vec![ColumnInfo {
            name: "Create Table".into(),
            data_type: "Utf8".into(),
        }],
        rows: vec![vec![json!(statement)]],
    })
}

async fn describe(state: &AppState, target: TableTarget) -> DdlResult<CatalogStatementResult> {
    let qualified = target.qualified();
    let published = state.catalog.read().await.clone();
    let reference = kaveon_core::TableReference::Full {
        catalog: target.catalog,
        schema: target.schema,
        table: target.table,
    };
    let resolved = published.resolve_table(&reference).map_err(|error| {
        CatalogStatementError::not_found("TABLE_NOT_FOUND", format!("{error} ({qualified})"))
    })?;
    Ok(CatalogStatementResult {
        columns: ["Column", "Type", "Nullable"]
            .into_iter()
            .map(|name| ColumnInfo {
                name: name.into(),
                data_type: "Utf8".into(),
            })
            .collect(),
        rows: resolved
            .table
            .arrow_schema
            .fields()
            .iter()
            .map(|field| {
                vec![
                    json!(field.name()),
                    json!(sql_type_name(field.data_type())),
                    json!(if field.is_nullable() { "YES" } else { "NO" }),
                ]
            })
            .collect(),
    })
}

async fn show_catalogs(
    state: &AppState,
    like: Option<String>,
) -> DdlResult<CatalogStatementResult> {
    let published = state.catalog.read().await.clone();
    Ok(names("Catalog", published.catalog_names(), like.as_deref()))
}

async fn show_schemas(
    state: &AppState,
    catalog: String,
    like: Option<String>,
) -> DdlResult<CatalogStatementResult> {
    let published = state.catalog.read().await.clone();
    let provider = published.catalog(&catalog).ok_or_else(|| {
        CatalogStatementError::not_found(
            "CATALOG_NOT_FOUND",
            format!("catalog '{catalog}' not found"),
        )
    })?;
    Ok(names("Schema", provider.schema_names(), like.as_deref()))
}

async fn show_tables(
    state: &AppState,
    catalog: String,
    schema: String,
    like: Option<String>,
) -> DdlResult<CatalogStatementResult> {
    let published = state.catalog.read().await.clone();
    let provider = published.catalog(&catalog).ok_or_else(|| {
        CatalogStatementError::not_found(
            "CATALOG_NOT_FOUND",
            format!("catalog '{catalog}' not found"),
        )
    })?;
    let tables = provider
        .table_names(&schema)
        .map_err(|error| CatalogStatementError::not_found("SCHEMA_NOT_FOUND", error.to_string()))?;
    Ok(names("Table", tables, like.as_deref()))
}

impl CatalogStatementError {
    fn invalid_error(error: KaveonError) -> Self {
        Self::invalid(error.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::security::Identity;
    use arrow::array::{ArrayRef, Int64Array, StringArray};
    use arrow::datatypes::{DataType, Field, Schema};
    use arrow::record_batch::RecordBatch;
    use kaveon_sql::ddl::parse_catalog_statement;
    use parquet::arrow::ArrowWriter;
    use std::fs::File;

    fn identity(role: Role) -> Identity {
        Identity {
            principal: match role {
                Role::Admin => "admin@example.com".into(),
                Role::Analyst => "analyst@example.com".into(),
                Role::Reader => "reader@example.com".into(),
            },
            display_identity: None,
            role,
        }
    }

    fn temporary_directory(label: &str) -> std::path::PathBuf {
        let directory = std::env::temp_dir().join(format!(
            "kaveon-ddl-{label}-{}-{}",
            std::process::id(),
            uuid::Uuid::new_v4().simple()
        ));
        std::fs::create_dir_all(&directory).unwrap();
        directory
    }

    fn write_parquet(path: &std::path::Path, rows: usize) {
        let schema = Arc::new(Schema::new(vec![
            Field::new("id", DataType::Int64, false),
            Field::new("name", DataType::Utf8, true),
        ]));
        let batch = RecordBatch::try_new(
            schema.clone(),
            vec![
                Arc::new(Int64Array::from_iter_values((0..rows).map(|v| v as i64))) as ArrayRef,
                Arc::new(StringArray::from_iter_values(
                    (0..rows).map(|v| format!("r{v}")),
                )) as ArrayRef,
            ],
        )
        .unwrap();
        let mut writer = ArrowWriter::try_new(File::create(path).unwrap(), schema, None).unwrap();
        writer.write(&batch).unwrap();
        writer.close().unwrap();
    }

    fn write_delta(directory: &std::path::Path) {
        let log = directory.join("_delta_log");
        std::fs::create_dir_all(&log).unwrap();
        write_parquet(&directory.join("part-0.parquet"), 3);
        write_parquet(&directory.join("part-1.parquet"), 4);
        std::fs::write(
            log.join("00000000000000000000.json"),
            "{\"add\":{\"path\":\"part-0.parquet\"}}\n{\"add\":{\"path\":\"part-1.parquet\"}}",
        )
        .unwrap();
    }

    async fn run(
        state: &Arc<AppState>,
        who: &Identity,
        sql: &str,
    ) -> Result<CatalogStatementResult, CatalogStatementError> {
        let statement = parse_catalog_statement(sql)
            .unwrap()
            .unwrap_or_else(|| panic!("{sql} is not a catalog statement"));
        execute_catalog_statement(state, who, "lake", "sales", statement).await
    }

    async fn ok(state: &Arc<AppState>, who: &Identity, sql: &str) -> Vec<Vec<Value>> {
        run(state, who, sql)
            .await
            .unwrap_or_else(|error| panic!("{sql}: {} {}", error.code, error.message))
            .rows
    }

    async fn err(state: &Arc<AppState>, who: &Identity, sql: &str) -> CatalogStatementError {
        match run(state, who, sql).await {
            Ok(result) => panic!("{sql} succeeded: {:?}", result.rows),
            Err(error) => error,
        }
    }

    fn state_with_local_catalog(base: &std::path::Path) -> Arc<AppState> {
        let state = crate::api::catalog_test_state();
        let catalog = CatalogDefinition::new(
            CatalogId::new("catalog:lake").unwrap(),
            "lake",
            CatalogAdapter::Native,
            StorageType::Local {
                base_path: base.to_path_buf(),
            },
        )
        .unwrap()
        .transition(CatalogLifecycle::Active)
        .unwrap();
        state
            .catalog_store
            .create_catalog("test", &catalog)
            .unwrap();
        Arc::new(state)
    }

    async fn published_tables(state: &Arc<AppState>, catalog: &str, schema: &str) -> Vec<String> {
        let published = state.catalog.read().await.clone();
        published
            .catalog(catalog)
            .and_then(|provider| provider.table_names(schema).ok())
            .unwrap_or_default()
    }

    #[tokio::test]
    async fn ddl_round_trip_over_a_parquet_file_and_a_delta_table() {
        let base = temporary_directory("roundtrip");
        write_parquet(&base.join("orders.parquet"), 5);
        write_delta(&base.join("events"));
        let state = state_with_local_catalog(&base);
        let analyst = identity(Role::Analyst);

        assert_eq!(
            ok(&state, &analyst, "CREATE SCHEMA lake.sales").await,
            vec![vec![json!("lake.sales"), json!("created")]]
        );
        assert_eq!(
            ok(&state, &analyst, "CREATE SCHEMA IF NOT EXISTS sales").await,
            vec![vec![json!("lake.sales"), json!("exists")]]
        );
        assert_eq!(
            err(&state, &analyst, "CREATE SCHEMA sales").await.status,
            StatusCode::CONFLICT
        );

        // Inferred columns come from the Parquet footer.
        assert_eq!(
            ok(
                &state,
                &analyst,
                "CREATE TABLE orders WITH (location = 'orders.parquet', format = 'parquet')"
            )
            .await,
            vec![vec![json!("lake.sales.orders"), json!("created")]]
        );
        let described = ok(&state, &analyst, "DESCRIBE lake.sales.orders").await;
        assert_eq!(
            described,
            vec![
                vec![json!("id"), json!("bigint"), json!("NO")],
                vec![json!("name"), json!("varchar"), json!("YES")],
            ]
        );
        assert_eq!(published_tables(&state, "lake", "sales").await, ["orders"]);

        // The Delta table through the Trino procedure spelling.
        assert_eq!(
            ok(
                &state,
                &analyst,
                "CALL lake.system.register_table(schema_name => 'sales', table_name => 'events', table_location => 'events')"
            )
            .await,
            vec![vec![json!("lake.sales.events"), json!("created")]]
        );
        let create = ok(&state, &analyst, "SHOW CREATE TABLE events").await;
        let rendered = create[0][0].as_str().unwrap();
        assert!(
            rendered.contains(
                "CREATE TABLE lake.sales.events (\n   id bigint NOT NULL,\n   name varchar\n)"
            ),
            "{rendered}"
        );
        assert!(rendered.contains("location = 'events'"), "{rendered}");
        assert!(rendered.contains("format = 'delta'"), "{rendered}");
        // What SHOW CREATE TABLE prints registers the same table again.
        assert_eq!(
            ok(&state, &analyst, "DROP TABLE events").await,
            vec![vec![json!("lake.sales.events"), json!("dropped")]]
        );
        assert_eq!(
            ok(&state, &analyst, rendered).await,
            vec![vec![json!("lake.sales.events"), json!("created")]]
        );

        let mut listed = ok(&state, &analyst, "SHOW TABLES").await;
        listed.sort_by_key(|row| row[0].to_string());
        assert_eq!(listed, vec![vec![json!("events")], vec![json!("orders")]]);
        assert_eq!(
            ok(&state, &analyst, "SHOW TABLES IN lake.sales LIKE 'ev%'").await,
            vec![vec![json!("events")]]
        );
        assert_eq!(
            ok(&state, &analyst, "SHOW SCHEMAS FROM lake").await,
            vec![vec![json!("sales")]]
        );
        assert_eq!(
            ok(&state, &analyst, "SHOW CATALOGS").await,
            vec![vec![json!("lake")]]
        );

        // The durable definitions carry the lifecycle and revisions the
        // HTTP API would have produced: Draft revision 1, Active revision 2.
        let catalog = state
            .catalog_store
            .catalog_by_name("lake")
            .unwrap()
            .unwrap();
        let schema = state
            .catalog_store
            .list_schemas(catalog.id())
            .unwrap()
            .remove(0);
        assert_eq!(schema.id().as_str(), "schema:lake:sales");
        let tables = state.catalog_store.list_tables(schema.id()).unwrap();
        assert!(tables.iter().all(|table| {
            table.lifecycle() == CatalogLifecycle::Active && table.revision().value() == 2
        }));
        assert_eq!(
            state
                .catalog_store
                .audit_events(None, 100)
                .unwrap()
                .iter()
                .filter(|event| event.actor == "analyst@example.com")
                .count(),
            8
        );

        // The published snapshot is what queries plan against.
        let published = state.catalog.read().await.clone();
        let resolved = published
            .resolve_table(&kaveon_core::TableReference::parse("lake.sales.orders"))
            .unwrap();
        assert_eq!(resolved.table.format, DataFormat::Parquet);
        assert_eq!(resolved.table.arrow_schema.fields().len(), 2);

        assert_eq!(
            err(&state, &analyst, "DROP SCHEMA sales").await.status,
            StatusCode::CONFLICT
        );
        assert_eq!(
            ok(&state, &analyst, "DROP SCHEMA lake.sales CASCADE").await,
            vec![vec![json!("lake.sales"), json!("dropped")]]
        );
        assert!(published_tables(&state, "lake", "sales").await.is_empty());
        assert_eq!(
            ok(&state, &analyst, "DROP SCHEMA IF EXISTS sales").await,
            vec![vec![json!("lake.sales"), json!("absent")]]
        );
        assert_eq!(
            ok(&state, &analyst, "DROP TABLE IF EXISTS sales.orders").await,
            vec![vec![json!("lake.sales.orders"), json!("absent")]]
        );
        std::fs::remove_dir_all(base).unwrap();
    }

    #[tokio::test]
    async fn an_unreadable_location_leaves_nothing_registered() {
        let base = temporary_directory("probe");
        write_parquet(&base.join("orders.parquet"), 2);
        let state = state_with_local_catalog(&base);
        let analyst = identity(Role::Analyst);
        ok(&state, &analyst, "CREATE SCHEMA sales").await;

        let missing = err(
            &state,
            &analyst,
            "CREATE TABLE ghosts WITH (location = 'ghosts.parquet', format = 'parquet')",
        )
        .await;
        assert_eq!(missing.code, "TABLE_NOT_READABLE");
        assert!(
            missing.message.contains("ghosts.parquet"),
            "{}",
            missing.message
        );

        // A Parquet file is not a Delta table.
        let wrong_format = err(
            &state,
            &analyst,
            "CREATE TABLE orders WITH (location = 'orders.parquet', format = 'delta')",
        )
        .await;
        assert_eq!(wrong_format.code, "TABLE_NOT_READABLE");

        // Declared columns must exist in the source.
        let wrong_column = err(
            &state,
            &analyst,
            "CREATE TABLE orders (id BIGINT, region VARCHAR) WITH (location = 'orders.parquet', format = 'parquet')",
        )
        .await;
        assert_eq!(wrong_column.code, "TABLE_NOT_READABLE");
        assert!(
            wrong_column.message.contains("'region'") && wrong_column.message.contains("id, name"),
            "{}",
            wrong_column.message
        );

        let catalog = state
            .catalog_store
            .catalog_by_name("lake")
            .unwrap()
            .unwrap();
        let schema = state
            .catalog_store
            .list_schemas(catalog.id())
            .unwrap()
            .remove(0);
        assert!(
            state
                .catalog_store
                .list_tables(schema.id())
                .unwrap()
                .is_empty()
        );
        assert!(published_tables(&state, "lake", "sales").await.is_empty());
        // The draft's creation and removal are both on the audit trail.
        let events = state.catalog_store.audit_events(None, 100).unwrap();
        assert_eq!(
            events
                .iter()
                .filter(|event| event.object_type == "table")
                .count(),
            6
        );

        // Declared columns that do exist are stored as declared.
        ok(
            &state,
            &analyst,
            "CREATE TABLE orders (id BIGINT NOT NULL) WITH (location = 'orders.parquet', format = 'parquet')",
        )
        .await;
        assert_eq!(
            ok(&state, &analyst, "DESCRIBE orders").await,
            vec![vec![json!("id"), json!("bigint"), json!("NO")]]
        );
        std::fs::remove_dir_all(base).unwrap();
    }

    /// A Hive-partitioned directory registers with its keys as columns —
    /// inferred, or declared with `partitioned_by` and the column list —
    /// and the definition records them; a declaration that does not match
    /// the paths, a key that is also a file column, and a relocation to a
    /// differently partitioned directory are refused.
    #[tokio::test]
    async fn partitioned_directories_register_their_keys() {
        let base = temporary_directory("partitioned");
        for partition in [
            "sales/dt=2026-09-01/region=eu",
            "sales/dt=2026-09-02/region=us",
            "sales/dt=__HIVE_DEFAULT_PARTITION__/region=eu",
            "flat/dt=2026-09-01",
        ] {
            std::fs::create_dir_all(base.join(partition)).unwrap();
            write_parquet(&base.join(partition).join("part-0.parquet"), 3);
        }
        let state = state_with_local_catalog(&base);
        let analyst = identity(Role::Analyst);
        ok(&state, &analyst, "CREATE SCHEMA sales").await;

        // Inferred: the file columns, then the keys typed from their values.
        ok(
            &state,
            &analyst,
            "CREATE TABLE sales WITH (location = 'sales', format = 'parquet')",
        )
        .await;
        assert_eq!(
            ok(&state, &analyst, "DESCRIBE sales").await,
            vec![
                vec![json!("id"), json!("bigint"), json!("NO")],
                vec![json!("name"), json!("varchar"), json!("YES")],
                vec![json!("dt"), json!("date"), json!("YES")],
                vec![json!("region"), json!("varchar"), json!("YES")],
            ]
        );
        let shown = ok(&state, &analyst, "SHOW CREATE TABLE sales").await;
        let statement = shown[0][0].as_str().unwrap();
        assert!(
            statement.contains("partitioned_by = ARRAY['dt', 'region']"),
            "{statement}"
        );
        let catalog = state
            .catalog_store
            .catalog_by_name("lake")
            .unwrap()
            .unwrap();
        let schema = state
            .catalog_store
            .list_schemas(catalog.id())
            .unwrap()
            .remove(0);
        let stored = state
            .catalog_store
            .list_tables(schema.id())
            .unwrap()
            .into_iter()
            .find(|table| table.name() == "sales")
            .unwrap();
        assert_eq!(
            stored
                .partitions()
                .iter()
                .map(|column| (column.name(), column.data_type().clone()))
                .collect::<Vec<_>>(),
            [("dt", DataType::Date32), ("region", DataType::Utf8)]
        );
        // The rendered statement re-registers the same definition.
        ok(&state, &analyst, "DROP TABLE sales").await;
        ok(&state, &analyst, statement).await;
        assert_eq!(
            ok(&state, &analyst, "DESCRIBE sales").await.len(),
            4,
            "{statement}"
        );
        ok(&state, &analyst, "DROP TABLE sales").await;

        // Declared: the keys named, the date read as text by the column
        // list's type.
        ok(
            &state,
            &analyst,
            "CREATE TABLE sales (id BIGINT, dt VARCHAR, region VARCHAR) WITH (location = 'sales', format = 'parquet', partitioned_by = ARRAY['dt', 'region'])",
        )
        .await;
        assert_eq!(
            ok(&state, &analyst, "DESCRIBE sales").await,
            vec![
                vec![json!("id"), json!("bigint"), json!("YES")],
                vec![json!("dt"), json!("varchar"), json!("YES")],
                vec![json!("region"), json!("varchar"), json!("YES")],
            ]
        );
        ok(&state, &analyst, "DROP TABLE sales").await;

        // A declaration must match the paths' keys and their order.
        for (sql, expected) in [
            (
                "CREATE TABLE sales WITH (location = 'sales', format = 'parquet', partitioned_by = ARRAY['region', 'dt'])",
                "region/dt",
            ),
            (
                "CREATE TABLE sales WITH (location = 'sales', format = 'parquet', partitioned_by = ARRAY['dt'])",
                "dt/region",
            ),
            (
                "CREATE TABLE flat WITH (location = 'flat', format = 'parquet', partitioned_by = ARRAY['dt', 'region'])",
                "lie under dt",
            ),
        ] {
            let failure = err(&state, &analyst, sql).await;
            assert_eq!(failure.code, "TABLE_NOT_READABLE", "{sql}");
            assert!(
                failure.message.contains(expected),
                "{sql}: {}",
                failure.message
            );
        }
        assert!(published_tables(&state, "lake", "sales").await.is_empty());

        // A key that is also a column inside the files has two sources.
        std::fs::create_dir_all(base.join("dup/name=x")).unwrap();
        write_parquet(&base.join("dup/name=x/part-0.parquet"), 2);
        let failure = err(
            &state,
            &analyst,
            "CREATE TABLE dup WITH (location = 'dup', format = 'parquet')",
        )
        .await;
        assert_eq!(failure.code, "TABLE_NOT_READABLE");
        assert!(
            failure.message.contains("'name'") && failure.message.contains("name=x/part-0.parquet"),
            "{}",
            failure.message
        );

        // A relocation keeps the partitioning: the new location must carry
        // the same keys.
        ok(
            &state,
            &analyst,
            "CREATE TABLE flat WITH (location = 'flat', format = 'parquet')",
        )
        .await;
        let failure = err(&state, &analyst, "ALTER TABLE flat SET LOCATION 'sales'").await;
        assert_eq!(failure.code, "TABLE_NOT_READABLE");
        assert!(failure.message.contains("dt/region"), "{}", failure.message);
        std::fs::remove_dir_all(base).unwrap();
    }

    #[tokio::test]
    async fn alter_table_set_location_is_a_probed_revision() {
        let base = temporary_directory("relocate");
        write_parquet(&base.join("v1.parquet"), 2);
        write_parquet(&base.join("v2.parquet"), 9);
        write_delta(&base.join("other"));
        let state = state_with_local_catalog(&base);
        let analyst = identity(Role::Analyst);
        ok(&state, &analyst, "CREATE SCHEMA sales").await;
        ok(
            &state,
            &analyst,
            "CREATE TABLE orders WITH (location = 'v1.parquet', format = 'parquet')",
        )
        .await;
        assert_eq!(
            ok(
                &state,
                &analyst,
                "ALTER TABLE orders SET LOCATION 'v2.parquet'"
            )
            .await,
            vec![vec![json!("lake.sales.orders"), json!("relocated")]]
        );
        assert_eq!(
            ok(
                &state,
                &analyst,
                "ALTER TABLE orders SET LOCATION 'v2.parquet'"
            )
            .await,
            vec![vec![json!("lake.sales.orders"), json!("unchanged")]]
        );
        let missing = err(
            &state,
            &analyst,
            "ALTER TABLE orders SET LOCATION 'v3.parquet'",
        )
        .await;
        assert_eq!(missing.code, "TABLE_NOT_READABLE");
        assert_eq!(
            err(
                &state,
                &analyst,
                "ALTER TABLE nothing SET LOCATION 'v2.parquet'"
            )
            .await
            .code,
            "TABLE_NOT_FOUND"
        );
        assert_eq!(
            ok(
                &state,
                &analyst,
                "ALTER TABLE IF EXISTS nothing SET LOCATION 'v2.parquet'"
            )
            .await,
            vec![vec![json!("lake.sales.nothing"), json!("absent")]]
        );
        let catalog = state
            .catalog_store
            .catalog_by_name("lake")
            .unwrap()
            .unwrap();
        let schema = state
            .catalog_store
            .list_schemas(catalog.id())
            .unwrap()
            .remove(0);
        let table = state
            .catalog_store
            .list_tables(schema.id())
            .unwrap()
            .remove(0);
        assert_eq!(table.location(), "v2.parquet");
        assert_eq!(table.revision().value(), 3);
        assert_eq!(table.lifecycle(), CatalogLifecycle::Active);
        let published = state.catalog.read().await.clone();
        let resolved = published
            .resolve_table(&kaveon_core::TableReference::parse("lake.sales.orders"))
            .unwrap();
        assert!(resolved.full_path().ends_with("v2.parquet"));
        std::fs::remove_dir_all(base).unwrap();
    }

    #[tokio::test]
    async fn roles_gate_catalog_and_schema_changes() {
        let base = temporary_directory("roles");
        let state = state_with_local_catalog(&base);
        let admin = identity(Role::Admin);
        let analyst = identity(Role::Analyst);
        let reader = identity(Role::Reader);

        let sql = format!(
            "CREATE CATALOG staging WITH (storage = 'local', base_path = '{}')",
            base.display().to_string().replace('\'', "''")
        );
        assert_eq!(
            err(&state, &analyst, &sql).await.status,
            StatusCode::FORBIDDEN
        );
        assert_eq!(
            ok(&state, &admin, &sql).await,
            vec![vec![json!("staging"), json!("created")]]
        );
        assert_eq!(
            ok(&state, &admin, "SHOW CATALOGS LIKE 'st%'").await,
            vec![vec![json!("staging")]]
        );
        assert_eq!(
            err(&state, &reader, "CREATE SCHEMA staging.raw")
                .await
                .status,
            StatusCode::FORBIDDEN
        );
        assert!(
            run(&state, &reader, "SHOW SCHEMAS FROM staging")
                .await
                .is_ok()
        );
        ok(&state, &analyst, "CREATE SCHEMA staging.raw").await;
        assert_eq!(
            err(&state, &analyst, "DROP CATALOG staging").await.status,
            StatusCode::FORBIDDEN
        );
        let not_empty = err(&state, &admin, "DROP CATALOG staging").await;
        assert_eq!(not_empty.status, StatusCode::CONFLICT);
        assert!(
            not_empty.message.contains("CASCADE"),
            "{}",
            not_empty.message
        );
        assert_eq!(
            ok(&state, &admin, "DROP CATALOG staging CASCADE").await,
            vec![vec![json!("staging"), json!("dropped")]]
        );
        assert_eq!(
            ok(&state, &admin, "DROP CATALOG IF EXISTS staging").await,
            vec![vec![json!("staging"), json!("absent")]]
        );
        assert!(
            state
                .catalog
                .read()
                .await
                .catalog_names()
                .iter()
                .all(|name| name != "staging")
        );
        std::fs::remove_dir_all(base).unwrap();
    }

    #[tokio::test]
    async fn create_catalog_maps_adls_storage_and_a_credential_reference() {
        let base = temporary_directory("adls");
        let state = state_with_local_catalog(&base);
        let admin = identity(Role::Admin);
        ok(
            &state,
            &admin,
            "CREATE CATALOG Benchmarks WITH (storage = 'adls', account = 'kvtest', container = 'opensource', root = 'benchmarks', credential = 'workload-identity:kaveon-test-reader')",
        )
        .await;
        let definition = state
            .catalog_store
            .catalog_by_name("Benchmarks")
            .unwrap()
            .unwrap();
        assert_eq!(definition.id().as_str(), "catalog:Benchmarks");
        assert_eq!(definition.lifecycle(), CatalogLifecycle::Active);
        assert_eq!(
            definition.storage(),
            &StorageType::AdlsGen2 {
                account: "kvtest".into(),
                container: "opensource".into(),
                root_path: "benchmarks".into(),
            }
        );
        let credential = definition.credential().unwrap();
        assert_eq!(credential.kind(), CredentialKind::WorkloadIdentity);
        assert_eq!(credential.reference(), "kaveon-test-reader");
        let bad = err(
            &state,
            &admin,
            "CREATE CATALOG other WITH (storage = 'adls', account = 'a', container = 'c', credential = 'password:hunter2')",
        )
        .await;
        assert_eq!(bad.code, "CATALOG_INVALID");
        let relative = err(
            &state,
            &admin,
            "CREATE CATALOG local WITH (storage = 'local', base_path = 'relative/path')",
        )
        .await;
        assert_eq!(relative.code, "CATALOG_INVALID");
        std::fs::remove_dir_all(base).unwrap();
    }

    #[test]
    fn like_patterns_follow_sql_semantics() {
        assert!(sql_like("orders_2026", "order%"));
        assert!(sql_like("orders", "_rders"));
        assert!(!sql_like("Orders", "orders"));
        assert!(sql_like("x", "%"));
        assert!(!sql_like("", "_"));
    }
}
