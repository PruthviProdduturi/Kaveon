#![deny(clippy::all)]

pub mod product_commit;
pub mod product_manifest;
pub mod product_metrics;
pub mod product_transaction;

use kaveon_core::{
    CatalogDefinition, CatalogId, CatalogRevision, KaveonError, Result, SchemaDefinition, SchemaId,
    TableDefinition, TableId, TableStatistics,
};
use rusqlite::{Connection, OptionalExtension, Transaction, TransactionBehavior, params};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    path::Path,
    sync::{Mutex, MutexGuard},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

const DATABASE_BUSY_TIMEOUT: Duration = Duration::from_secs(5);
const MIGRATION_VERSION: i64 = 2;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CascadePolicy {
    Restrict,
    Cascade,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AuditEvent {
    pub id: i64,
    pub occurred_at_unix_ms: u64,
    pub actor: String,
    pub action: String,
    pub object_type: String,
    pub object_id: String,
    pub revision: CatalogRevision,
    pub details: BTreeMap<String, String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CatalogReplicaSnapshot {
    pub identity: String,
    pub catalogs: Vec<CatalogDefinition>,
    pub schemas: Vec<SchemaDefinition>,
    pub tables: Vec<TableDefinition>,
}

const MAX_REPLICA_CATALOGS: usize = 100;
const MAX_REPLICA_SCHEMAS: usize = 10_000;
const MAX_REPLICA_TABLES: usize = 100_000;

pub struct CatalogStore {
    connection: Mutex<Connection>,
}

impl CatalogStore {
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        Self::initialize(Connection::open(path).map_err(db_error)?)
    }
    pub fn open_in_memory() -> Result<Self> {
        Self::initialize(Connection::open_in_memory().map_err(db_error)?)
    }
    fn initialize(mut connection: Connection) -> Result<Self> {
        connection
            .busy_timeout(DATABASE_BUSY_TIMEOUT)
            .map_err(db_error)?;
        connection
            .execute_batch("PRAGMA foreign_keys = ON; PRAGMA journal_mode = WAL;")
            .map_err(db_error)?;
        migrate(&connection)?;
        let transaction = immediate(&mut connection)?;
        refresh_snapshot_identity(&transaction)?;
        transaction.commit().map_err(db_error)?;
        Ok(Self {
            connection: Mutex::new(connection),
        })
    }

    pub fn create_catalog(&self, actor: &str, value: &CatalogDefinition) -> Result<()> {
        self.insert(
            actor,
            "catalogs",
            "catalog",
            value.id().as_str(),
            None,
            value.name(),
            value.revision(),
            value,
        )
    }
    pub fn catalog(&self, id: &CatalogId) -> Result<Option<CatalogDefinition>> {
        self.load("catalogs", id.as_str())
    }
    pub fn catalog_by_name(&self, name: &str) -> Result<Option<CatalogDefinition>> {
        self.load_by_name("catalogs", name)
    }
    pub fn list_catalogs(&self) -> Result<Vec<CatalogDefinition>> {
        self.list("SELECT definition_json FROM catalogs ORDER BY name", [])
    }
    pub fn replace_catalog(
        &self,
        actor: &str,
        expected: CatalogRevision,
        value: &CatalogDefinition,
    ) -> Result<()> {
        self.replace(
            actor,
            "catalogs",
            "catalog",
            value.id().as_str(),
            value.name(),
            expected,
            value.revision(),
            value,
        )
    }
    pub fn delete_catalog(
        &self,
        actor: &str,
        id: &CatalogId,
        expected: CatalogRevision,
        policy: CascadePolicy,
    ) -> Result<()> {
        self.delete_parent(
            actor,
            "catalogs",
            "catalog",
            id.as_str(),
            expected,
            policy,
            "schemas",
            "catalog_id",
        )
    }

    pub fn create_schema(&self, actor: &str, value: &SchemaDefinition) -> Result<()> {
        self.insert(
            actor,
            "schemas",
            "schema",
            value.id().as_str(),
            Some(("catalog_id", value.catalog_id().as_str())),
            value.name(),
            value.revision(),
            value,
        )
    }
    pub fn schema(&self, id: &SchemaId) -> Result<Option<SchemaDefinition>> {
        self.load("schemas", id.as_str())
    }
    pub fn list_schemas(&self, catalog: &CatalogId) -> Result<Vec<SchemaDefinition>> {
        self.list(
            "SELECT definition_json FROM schemas WHERE catalog_id = ?1 ORDER BY name",
            [catalog.as_str()],
        )
    }
    pub fn replace_schema(
        &self,
        actor: &str,
        expected: CatalogRevision,
        value: &SchemaDefinition,
    ) -> Result<()> {
        self.replace(
            actor,
            "schemas",
            "schema",
            value.id().as_str(),
            value.name(),
            expected,
            value.revision(),
            value,
        )
    }
    pub fn delete_schema(
        &self,
        actor: &str,
        id: &SchemaId,
        expected: CatalogRevision,
        policy: CascadePolicy,
    ) -> Result<()> {
        self.delete_parent(
            actor,
            "schemas",
            "schema",
            id.as_str(),
            expected,
            policy,
            "tables",
            "schema_id",
        )
    }

    pub fn create_table(&self, actor: &str, value: &TableDefinition) -> Result<()> {
        self.insert(
            actor,
            "tables",
            "table",
            value.id().as_str(),
            Some(("schema_id", value.schema_id().as_str())),
            value.name(),
            value.revision(),
            value,
        )
    }
    pub fn table(&self, id: &TableId) -> Result<Option<TableDefinition>> {
        self.load("tables", id.as_str())
    }
    pub fn list_tables(&self, schema: &SchemaId) -> Result<Vec<TableDefinition>> {
        self.list(
            "SELECT definition_json FROM tables WHERE schema_id = ?1 ORDER BY name",
            [schema.as_str()],
        )
    }
    pub fn replace_table(
        &self,
        actor: &str,
        expected: CatalogRevision,
        value: &TableDefinition,
    ) -> Result<()> {
        self.replace(
            actor,
            "tables",
            "table",
            value.id().as_str(),
            value.name(),
            expected,
            value.revision(),
            value,
        )
    }
    pub fn delete_table(&self, actor: &str, id: &TableId, expected: CatalogRevision) -> Result<()> {
        let mut connection = self.connection()?;
        let transaction = immediate(&mut connection)?;
        delete_row(&transaction, "tables", id.as_str(), expected)?;
        audit(
            &transaction,
            actor,
            "delete",
            "table",
            id.as_str(),
            expected,
        )?;
        refresh_snapshot_identity(&transaction)?;
        transaction.commit().map_err(db_error)
    }

    /// The table named `catalog.schema.table`, whatever its lifecycle: the
    /// way a resolved reference finds its durable id.
    pub fn table_by_name(
        &self,
        catalog: &str,
        schema: &str,
        table: &str,
    ) -> Result<Option<TableDefinition>> {
        let connection = self.connection()?;
        let value: Option<String> = connection
            .query_row(
                "SELECT t.definition_json FROM tables t \
                 JOIN schemas s ON s.id = t.schema_id \
                 JOIN catalogs c ON c.id = s.catalog_id \
                 WHERE c.name = ?1 AND s.name = ?2 AND t.name = ?3",
                params![catalog, schema, table],
                |row| row.get(0),
            )
            .optional()
            .map_err(db_error)?;
        value.map(|json| decode(&json)).transpose()
    }

    /// Store a table's statistics beside its definition, replacing the
    /// statistics of any earlier source version. The table must exist; its
    /// statistics are deleted with it. Statistics do not enter the catalog
    /// snapshot identity: they describe the data, not the definitions.
    pub fn put_table_statistics(&self, actor: &str, value: &TableStatistics) -> Result<()> {
        validate_actor(actor)?;
        let document = value.to_json_bytes()?;
        let mut connection = self.connection()?;
        let transaction = immediate(&mut connection)?;
        let revision: Option<u64> = transaction
            .query_row(
                "SELECT revision FROM tables WHERE id = ?1",
                [value.table_id.as_str()],
                |row| row.get(0),
            )
            .optional()
            .map_err(db_error)?;
        let Some(revision) = revision else {
            return Err(catalog_error(format!(
                "catalog object '{}' not found",
                value.table_id.as_str()
            )));
        };
        let depth = encode(&value.depth)?.trim_matches('"').to_owned();
        transaction
            .execute(
                "INSERT INTO table_statistics(table_id, source_version, computed_at_ms, depth, document) \
                 VALUES (?1, ?2, ?3, ?4, ?5) \
                 ON CONFLICT(table_id) DO UPDATE SET source_version = excluded.source_version, \
                 computed_at_ms = excluded.computed_at_ms, depth = excluded.depth, document = excluded.document",
                params![
                    value.table_id.as_str(),
                    value.source_version.identity_sha256,
                    value.computed_at_ms,
                    depth,
                    document,
                ],
            )
            .map_err(db_error)?;
        let details = BTreeMap::from([
            (
                "source_version".to_owned(),
                value.source_version.identity_sha256.clone(),
            ),
            ("depth".to_owned(), depth),
            ("rows".to_owned(), value.rows.to_string()),
            ("files".to_owned(), value.files.to_string()),
        ]);
        audit_with_details(
            &transaction,
            actor,
            "statistics",
            "table",
            value.table_id.as_str(),
            CatalogRevision::new(revision)?,
            &details,
        )?;
        transaction.commit().map_err(db_error)
    }

    /// The table's stored statistics, whatever source version they describe.
    pub fn table_statistics(&self, id: &TableId) -> Result<Option<TableStatistics>> {
        let connection = self.connection()?;
        let document: Option<Vec<u8>> = connection
            .query_row(
                "SELECT document FROM table_statistics WHERE table_id = ?1",
                [id.as_str()],
                |row| row.get(0),
            )
            .optional()
            .map_err(db_error)?;
        document
            .map(|bytes| TableStatistics::from_json_bytes(&bytes))
            .transpose()
    }

    /// The source version the table's stored statistics describe, without
    /// decoding the document.
    pub fn table_statistics_version(&self, id: &TableId) -> Result<Option<String>> {
        let connection = self.connection()?;
        connection
            .query_row(
                "SELECT source_version FROM table_statistics WHERE table_id = ?1",
                [id.as_str()],
                |row| row.get(0),
            )
            .optional()
            .map_err(db_error)
    }

    /// Remove a table's statistics; `Ok(false)` when it had none.
    pub fn delete_table_statistics(&self, actor: &str, id: &TableId) -> Result<bool> {
        validate_actor(actor)?;
        let mut connection = self.connection()?;
        let transaction = immediate(&mut connection)?;
        let changed = transaction
            .execute(
                "DELETE FROM table_statistics WHERE table_id = ?1",
                [id.as_str()],
            )
            .map_err(db_error)?;
        if changed == 1 {
            let revision: u64 = transaction
                .query_row(
                    "SELECT revision FROM tables WHERE id = ?1",
                    [id.as_str()],
                    |row| row.get(0),
                )
                .map_err(db_error)?;
            audit(
                &transaction,
                actor,
                "statistics_delete",
                "table",
                id.as_str(),
                CatalogRevision::new(revision)?,
            )?;
        }
        transaction.commit().map_err(db_error)?;
        Ok(changed == 1)
    }

    /// The actor of an object's `create` audit event, when the object was
    /// created through this store.
    pub fn creator(&self, object_type: &str, object_id: &str) -> Result<Option<String>> {
        let connection = self.connection()?;
        let mut statement = connection
            .prepare(
                "SELECT actor FROM audit_events WHERE object_type = ?1 AND object_id = ?2 AND action = 'create' ORDER BY id LIMIT 1",
            )
            .map_err(db_error)?;
        let mut rows = statement
            .query(params![object_type, object_id])
            .map_err(db_error)?;
        match rows.next().map_err(db_error)? {
            Some(row) => Ok(Some(row.get::<_, String>(0).map_err(db_error)?)),
            None => Ok(None),
        }
    }

    pub fn audit_events(&self, after_id: Option<i64>, limit: usize) -> Result<Vec<AuditEvent>> {
        if limit == 0 {
            return Ok(Vec::new());
        }
        let limit = i64::try_from(limit).map_err(|_| catalog_error("audit limit is too large"))?;
        let connection = self.connection()?;
        let mut statement = connection.prepare("SELECT id, occurred_at_ms, actor, action, object_type, object_id, revision, details_json FROM audit_events WHERE id > ?1 ORDER BY id LIMIT ?2").map_err(db_error)?;
        collect(
            statement
                .query_map(params![after_id.unwrap_or(0), limit], audit_row)
                .map_err(db_error)?,
        )
    }

    /// Returns the durable identity of the current catalog definition set.
    /// Updated atomically with each successful catalog mutation.
    pub fn snapshot_identity(&self) -> Result<String> {
        let connection = self.connection()?;
        connection
            .query_row(
                "SELECT identity FROM catalog_snapshot WHERE singleton = 1",
                [],
                |row| row.get(0),
            )
            .map_err(db_error)
    }

    pub fn export_replica_snapshot(&self) -> Result<CatalogReplicaSnapshot> {
        let connection = self.connection()?;
        let catalogs = list_on_connection(
            &connection,
            "SELECT definition_json FROM catalogs ORDER BY id",
        )?;
        let schemas = list_on_connection(
            &connection,
            "SELECT definition_json FROM schemas ORDER BY id",
        )?;
        let tables = list_on_connection(
            &connection,
            "SELECT definition_json FROM tables ORDER BY id",
        )?;
        let identity = connection
            .query_row(
                "SELECT identity FROM catalog_snapshot WHERE singleton = 1",
                [],
                |row| row.get(0),
            )
            .map_err(db_error)?;
        Ok(CatalogReplicaSnapshot {
            identity,
            catalogs,
            schemas,
            tables,
        })
    }

    pub fn install_replica_snapshot(&self, snapshot: &CatalogReplicaSnapshot) -> Result<()> {
        if snapshot.catalogs.len() > MAX_REPLICA_CATALOGS
            || snapshot.schemas.len() > MAX_REPLICA_SCHEMAS
            || snapshot.tables.len() > MAX_REPLICA_TABLES
        {
            return Err(catalog_error("replica snapshot exceeds object bounds"));
        }
        let mut connection = self.connection()?;
        let transaction = immediate(&mut connection)?;
        transaction
            .execute("DELETE FROM tables", [])
            .map_err(db_error)?;
        transaction
            .execute("DELETE FROM schemas", [])
            .map_err(db_error)?;
        transaction
            .execute("DELETE FROM catalogs", [])
            .map_err(db_error)?;
        for value in &snapshot.catalogs {
            transaction
                .execute(
                    "INSERT INTO catalogs(id,name,revision,definition_json) VALUES (?1,?2,?3,?4)",
                    params![
                        value.id().as_str(),
                        value.name(),
                        value.revision().value(),
                        encode(value)?
                    ],
                )
                .map_err(db_error)?;
        }
        for value in &snapshot.schemas {
            transaction.execute(
                "INSERT INTO schemas(id,catalog_id,name,revision,definition_json) VALUES (?1,?2,?3,?4,?5)",
                params![value.id().as_str(), value.catalog_id().as_str(), value.name(), value.revision().value(), encode(value)?],
            ).map_err(db_error)?;
        }
        for value in &snapshot.tables {
            transaction.execute(
                "INSERT INTO tables(id,schema_id,name,revision,definition_json) VALUES (?1,?2,?3,?4,?5)",
                params![value.id().as_str(), value.schema_id().as_str(), value.name(), value.revision().value(), encode(value)?],
            ).map_err(db_error)?;
        }
        let actual = refresh_snapshot_identity(&transaction)?;
        if actual != snapshot.identity {
            return Err(catalog_error(format!(
                "replica snapshot digest mismatch: expected {}, computed {actual}",
                snapshot.identity
            )));
        }
        transaction.commit().map_err(db_error)
    }

    #[allow(clippy::too_many_arguments)]
    fn insert<T: Serialize>(
        &self,
        actor: &str,
        table: &str,
        object_type: &str,
        id: &str,
        parent: Option<(&str, &str)>,
        name: &str,
        revision: CatalogRevision,
        value: &T,
    ) -> Result<()> {
        validate_actor(actor)?;
        let mut connection = self.connection()?;
        let transaction = immediate(&mut connection)?;
        let sql = match parent {
            Some((column, _)) => format!(
                "INSERT INTO {table}(id, {column}, name, revision, definition_json) VALUES (?1, ?2, ?3, ?4, ?5)"
            ),
            None => format!(
                "INSERT INTO {table}(id, name, revision, definition_json) VALUES (?1, ?2, ?3, ?4)"
            ),
        };
        match parent {
            Some((_, parent_id)) => transaction.execute(
                &sql,
                params![id, parent_id, name, revision.value(), encode(value)?],
            ),
            None => transaction.execute(&sql, params![id, name, revision.value(), encode(value)?]),
        }
        .map_err(db_error)?;
        audit(&transaction, actor, "create", object_type, id, revision)?;
        refresh_snapshot_identity(&transaction)?;
        transaction.commit().map_err(db_error)
    }

    #[allow(clippy::too_many_arguments)]
    fn replace<T: Serialize>(
        &self,
        actor: &str,
        table: &str,
        object_type: &str,
        id: &str,
        name: &str,
        expected: CatalogRevision,
        actual: CatalogRevision,
        value: &T,
    ) -> Result<()> {
        let next = expected.next()?;
        if actual != next {
            return Err(catalog_error(format!(
                "replacement revision must be {}, received {}",
                next.value(),
                actual.value()
            )));
        }
        let mut connection = self.connection()?;
        let transaction = immediate(&mut connection)?;
        let sql = format!(
            "UPDATE {table} SET name = ?1, revision = ?2, definition_json = ?3 WHERE id = ?4 AND revision = ?5"
        );
        let changed = transaction
            .execute(
                &sql,
                params![name, actual.value(), encode(value)?, id, expected.value()],
            )
            .map_err(db_error)?;
        require_changed(&transaction, table, id, expected, changed)?;
        audit(&transaction, actor, "update", object_type, id, actual)?;
        refresh_snapshot_identity(&transaction)?;
        transaction.commit().map_err(db_error)
    }

    #[allow(clippy::too_many_arguments)]
    fn delete_parent(
        &self,
        actor: &str,
        table: &str,
        object_type: &str,
        id: &str,
        expected: CatalogRevision,
        policy: CascadePolicy,
        child_table: &str,
        child_column: &str,
    ) -> Result<()> {
        let mut connection = self.connection()?;
        let transaction = immediate(&mut connection)?;
        let sql = format!("SELECT COUNT(*) FROM {child_table} WHERE {child_column} = ?1");
        let children: i64 = transaction
            .query_row(&sql, [id], |row| row.get(0))
            .map_err(db_error)?;
        if policy == CascadePolicy::Restrict && children != 0 {
            return Err(catalog_error(format!("{object_type} '{id}' is not empty")));
        }
        delete_row(&transaction, table, id, expected)?;
        let details = if policy == CascadePolicy::Cascade {
            BTreeMap::from([
                ("cascade".to_owned(), "true".to_owned()),
                ("direct_children_deleted".to_owned(), children.to_string()),
            ])
        } else {
            BTreeMap::new()
        };
        audit_with_details(
            &transaction,
            actor,
            "delete",
            object_type,
            id,
            expected,
            &details,
        )?;
        refresh_snapshot_identity(&transaction)?;
        transaction.commit().map_err(db_error)
    }

    fn load<T: DeserializeOwned>(&self, table: &str, id: &str) -> Result<Option<T>> {
        let connection = self.connection()?;
        let sql = format!("SELECT definition_json FROM {table} WHERE id = ?1");
        let value: Option<String> = connection
            .query_row(&sql, [id], |row| row.get(0))
            .optional()
            .map_err(db_error)?;
        value.map(|json| decode(&json)).transpose()
    }
    fn load_by_name<T: DeserializeOwned>(&self, table: &str, name: &str) -> Result<Option<T>> {
        let connection = self.connection()?;
        let sql = format!("SELECT definition_json FROM {table} WHERE name = ?1");
        let value: Option<String> = connection
            .query_row(&sql, [name], |row| row.get(0))
            .optional()
            .map_err(db_error)?;
        value.map(|json| decode(&json)).transpose()
    }
    fn list<T: DeserializeOwned, P: rusqlite::Params>(
        &self,
        sql: &str,
        parameters: P,
    ) -> Result<Vec<T>> {
        let connection = self.connection()?;
        let mut statement = connection.prepare(sql).map_err(db_error)?;
        collect(
            statement
                .query_map(parameters, |row| {
                    let json: String = row.get(0)?;
                    decode_sql(&json)
                })
                .map_err(db_error)?,
        )
    }
    fn connection(&self) -> Result<MutexGuard<'_, Connection>> {
        self.connection
            .lock()
            .map_err(|_| catalog_error("database lock is poisoned"))
    }
}

fn migrate(connection: &Connection) -> Result<()> {
    connection.execute_batch(&format!(r#"
        CREATE TABLE IF NOT EXISTS schema_migrations(version INTEGER PRIMARY KEY);
        CREATE TABLE IF NOT EXISTS catalogs(id TEXT PRIMARY KEY, name TEXT NOT NULL UNIQUE, revision INTEGER NOT NULL, definition_json TEXT NOT NULL);
        CREATE TABLE IF NOT EXISTS schemas(id TEXT PRIMARY KEY, catalog_id TEXT NOT NULL, name TEXT NOT NULL, revision INTEGER NOT NULL, definition_json TEXT NOT NULL, UNIQUE(catalog_id,name), FOREIGN KEY(catalog_id) REFERENCES catalogs(id) ON DELETE CASCADE);
        CREATE TABLE IF NOT EXISTS tables(id TEXT PRIMARY KEY, schema_id TEXT NOT NULL, name TEXT NOT NULL, revision INTEGER NOT NULL, definition_json TEXT NOT NULL, UNIQUE(schema_id,name), FOREIGN KEY(schema_id) REFERENCES schemas(id) ON DELETE CASCADE);
        CREATE TABLE IF NOT EXISTS audit_events(id INTEGER PRIMARY KEY AUTOINCREMENT, occurred_at_ms INTEGER NOT NULL, actor TEXT NOT NULL, action TEXT NOT NULL, object_type TEXT NOT NULL, object_id TEXT NOT NULL, revision INTEGER NOT NULL, details_json TEXT NOT NULL);
        CREATE TABLE IF NOT EXISTS catalog_snapshot(singleton INTEGER PRIMARY KEY CHECK(singleton = 1), identity TEXT NOT NULL);
        CREATE TABLE IF NOT EXISTS table_statistics(table_id TEXT PRIMARY KEY, source_version TEXT NOT NULL, computed_at_ms INTEGER NOT NULL, depth TEXT NOT NULL, document BLOB NOT NULL, FOREIGN KEY(table_id) REFERENCES tables(id) ON DELETE CASCADE);
        INSERT OR IGNORE INTO catalog_snapshot(singleton, identity) VALUES (1, 'sha256:e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855');
        CREATE INDEX IF NOT EXISTS idx_audit_object ON audit_events(object_type, object_id, id);
        INSERT OR IGNORE INTO schema_migrations(version) VALUES ({MIGRATION_VERSION});
    "#)).map_err(db_error)
}

fn immediate(connection: &mut Connection) -> Result<Transaction<'_>> {
    connection
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(db_error)
}

fn refresh_snapshot_identity(transaction: &Transaction<'_>) -> Result<String> {
    let mut digest = Sha256::new();
    digest.update(b"kaveon-durable-catalog-v1");
    for table in ["catalogs", "schemas", "tables"] {
        digest_field(&mut digest, table.as_bytes());
        let mut statement = transaction
            .prepare(&format!(
                "SELECT id, definition_json FROM {table} ORDER BY id"
            ))
            .map_err(db_error)?;
        let rows = statement
            .query_map([], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
            })
            .map_err(db_error)?;
        for row in rows {
            let (id, definition) = row.map_err(db_error)?;
            digest_field(&mut digest, id.as_bytes());
            digest_field(&mut digest, definition.as_bytes());
        }
    }
    let identity = format!("sha256:{:x}", digest.finalize());
    transaction
        .execute(
            "UPDATE catalog_snapshot SET identity = ?1 WHERE singleton = 1",
            [&identity],
        )
        .map_err(db_error)?;
    Ok(identity)
}

fn digest_field(digest: &mut Sha256, value: &[u8]) {
    digest.update((value.len() as u64).to_be_bytes());
    digest.update(value);
}
fn delete_row(
    transaction: &Transaction<'_>,
    table: &str,
    id: &str,
    expected: CatalogRevision,
) -> Result<()> {
    let sql = format!("DELETE FROM {table} WHERE id=?1 AND revision=?2");
    let changed = transaction
        .execute(&sql, params![id, expected.value()])
        .map_err(db_error)?;
    require_changed(transaction, table, id, expected, changed)
}
fn require_changed(
    transaction: &Transaction<'_>,
    table: &str,
    id: &str,
    expected: CatalogRevision,
    changed: usize,
) -> Result<()> {
    if changed == 1 {
        return Ok(());
    }
    let sql = format!("SELECT revision FROM {table} WHERE id=?1");
    let actual: Option<u64> = transaction
        .query_row(&sql, [id], |row| row.get(0))
        .optional()
        .map_err(db_error)?;
    match actual {
        Some(value) => Err(catalog_error(format!(
            "revision conflict for '{id}': expected {}, current {value}",
            expected.value()
        ))),
        None => Err(catalog_error(format!("catalog object '{id}' not found"))),
    }
}
fn audit(
    transaction: &Transaction<'_>,
    actor: &str,
    action: &str,
    object_type: &str,
    object_id: &str,
    revision: CatalogRevision,
) -> Result<()> {
    audit_with_details(
        transaction,
        actor,
        action,
        object_type,
        object_id,
        revision,
        &BTreeMap::new(),
    )
}

fn audit_with_details(
    transaction: &Transaction<'_>,
    actor: &str,
    action: &str,
    object_type: &str,
    object_id: &str,
    revision: CatalogRevision,
    details: &BTreeMap<String, String>,
) -> Result<()> {
    validate_actor(actor)?;
    transaction.execute("INSERT INTO audit_events(occurred_at_ms,actor,action,object_type,object_id,revision,details_json) VALUES (?1,?2,?3,?4,?5,?6,'{}')", params![now_ms()?,actor,action,object_type,object_id,revision.value()]).map_err(db_error)?;
    if !details.is_empty() {
        transaction
            .execute(
                "UPDATE audit_events SET details_json = ?1 WHERE id = last_insert_rowid()",
                [encode(details)?],
            )
            .map_err(db_error)?;
    }
    Ok(())
}
fn audit_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<AuditEvent> {
    let revision = CatalogRevision::new(row.get(6)?).map_err(core_sql_error)?;
    let details: String = row.get(7)?;
    Ok(AuditEvent {
        id: row.get(0)?,
        occurred_at_unix_ms: row.get(1)?,
        actor: row.get(2)?,
        action: row.get(3)?,
        object_type: row.get(4)?,
        object_id: row.get(5)?,
        revision,
        details: decode_sql(&details)?,
    })
}
fn collect<T>(
    rows: rusqlite::MappedRows<'_, impl FnMut(&rusqlite::Row<'_>) -> rusqlite::Result<T>>,
) -> Result<Vec<T>> {
    rows.collect::<rusqlite::Result<Vec<_>>>().map_err(db_error)
}
fn list_on_connection<T: DeserializeOwned>(connection: &Connection, sql: &str) -> Result<Vec<T>> {
    let mut statement = connection.prepare(sql).map_err(db_error)?;
    collect(
        statement
            .query_map([], |row| {
                let json: String = row.get(0)?;
                decode_sql(&json)
            })
            .map_err(db_error)?,
    )
}
fn encode<T: Serialize>(value: &T) -> Result<String> {
    serde_json::to_string(value).map_err(json_error)
}
fn decode<T: DeserializeOwned>(value: &str) -> Result<T> {
    serde_json::from_str(value).map_err(json_error)
}
fn decode_sql<T: DeserializeOwned>(value: &str) -> rusqlite::Result<T> {
    serde_json::from_str(value).map_err(|error| {
        rusqlite::Error::FromSqlConversionFailure(0, rusqlite::types::Type::Text, Box::new(error))
    })
}
fn core_sql_error(error: KaveonError) -> rusqlite::Error {
    rusqlite::Error::FromSqlConversionFailure(0, rusqlite::types::Type::Integer, Box::new(error))
}
fn validate_actor(actor: &str) -> Result<()> {
    if actor.trim().is_empty() {
        Err(catalog_error("audit actor cannot be empty"))
    } else {
        Ok(())
    }
}
fn now_ms() -> Result<u64> {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|error| catalog_error(format!("clock: {error}")))?
        .as_millis()
        .try_into()
        .map_err(|_| catalog_error("timestamp overflow"))
}
fn db_error(error: rusqlite::Error) -> KaveonError {
    if matches!(
        &error,
        rusqlite::Error::SqliteFailure(_, Some(message))
            if message.starts_with("UNIQUE constraint failed")
    ) {
        return catalog_error("object already exists");
    }
    catalog_error(format!("database: {error}"))
}
fn json_error(error: serde_json::Error) -> KaveonError {
    catalog_error(format!("metadata serialization: {error}"))
}
fn catalog_error(message: impl Into<String>) -> KaveonError {
    KaveonError::Execution(format!("catalog: {}", message.into()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use kaveon_core::{
        AccessPattern, CatalogAdapter, CatalogLifecycle, ColumnDefinition, DataFormat, StorageType,
    };
    use std::fs;
    fn values() -> (CatalogDefinition, SchemaDefinition, TableDefinition) {
        let catalog_id = CatalogId::new("catalog-1").unwrap();
        let schema_id = SchemaId::new("schema-1").unwrap();
        (
            CatalogDefinition::new(
                catalog_id.clone(),
                "local",
                CatalogAdapter::Native,
                StorageType::Local {
                    base_path: "data".into(),
                },
            )
            .unwrap(),
            SchemaDefinition::new(schema_id.clone(), catalog_id, "default").unwrap(),
            TableDefinition::new(
                TableId::new("table-1").unwrap(),
                schema_id,
                "orders",
                "orders",
                AccessPattern::Shortcut,
                DataFormat::Delta,
                vec![ColumnDefinition::new("id", arrow_schema::DataType::Int64, false).unwrap()],
            )
            .unwrap(),
        )
    }
    fn seed(store: &CatalogStore) -> (CatalogDefinition, SchemaDefinition, TableDefinition) {
        let value = values();
        store.create_catalog("test", &value.0).unwrap();
        store.create_schema("test", &value.1).unwrap();
        store.create_table("test", &value.2).unwrap();
        value
    }
    #[test]
    fn restart_persists_metadata() {
        let directory =
            std::env::temp_dir().join(format!("kaveon-catalog-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&directory).unwrap();
        let path = directory.join("catalog.db");
        let id = {
            let store = CatalogStore::open(&path).unwrap();
            seed(&store).2.id().clone()
        };
        let store = CatalogStore::open(&path).unwrap();
        assert_eq!(store.table(&id).unwrap().unwrap().name(), "orders");
        assert_eq!(store.audit_events(None, 10).unwrap().len(), 3);
        drop(store);
        fs::remove_dir_all(directory).unwrap();
    }
    fn statistics_for(table: &TableDefinition, identity: &str, rows: u64) -> TableStatistics {
        use kaveon_core::{
            ColumnStatistics, HllSketch, SourceVersion, SourceVersionKind, StatValue,
            StatisticsDepth,
        };
        let mut distinct = HllSketch::default_precision();
        for value in 0..rows {
            distinct.insert_text(&value.to_string());
        }
        TableStatistics {
            version: kaveon_core::statistics::TABLE_STATISTICS_VERSION,
            table_id: table.id().clone(),
            source_version: SourceVersion {
                identity_sha256: identity.into(),
                kind: SourceVersionKind::DeltaVersion { version: rows },
            },
            computed_at_ms: 1,
            depth: StatisticsDepth::Full,
            rows,
            bytes: rows * 10,
            files: 1,
            columns: vec![ColumnStatistics {
                name: "id".into(),
                data_type: arrow_schema::DataType::Int64,
                null_count: Some(0),
                min: Some(StatValue::Int(0)),
                max: Some(StatValue::Int(rows as i128)),
                bounds_exact: true,
                distinct: Some(distinct),
                distinct_exact: None,
                quantiles: None,
                bytes: None,
            }],
            per_file: Vec::new(),
            per_file_complete: false,
        }
    }

    #[test]
    fn table_statistics_are_stored_beside_the_definition_versioned_and_cascade() {
        let directory =
            std::env::temp_dir().join(format!("kaveon-catalog-stats-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&directory).unwrap();
        let path = directory.join("catalog.db");
        let table = {
            let store = CatalogStore::open(&path).unwrap();
            let (_, _, table) = seed(&store);
            assert_eq!(store.table_statistics(table.id()).unwrap(), None);
            assert_eq!(store.table_statistics_version(table.id()).unwrap(), None);
            let identity_before = store.snapshot_identity().unwrap();
            let first = statistics_for(&table, "v1", 100);
            store.put_table_statistics("analyzer", &first).unwrap();
            assert_eq!(store.table_statistics(table.id()).unwrap(), Some(first));
            assert_eq!(
                store
                    .table_statistics_version(table.id())
                    .unwrap()
                    .as_deref(),
                Some("v1")
            );
            // Statistics do not move the definition identity.
            assert_eq!(store.snapshot_identity().unwrap(), identity_before);
            // A newer source version replaces the document.
            let second = statistics_for(&table, "v2", 250);
            store.put_table_statistics("analyzer", &second).unwrap();
            let stored = store.table_statistics(table.id()).unwrap().unwrap();
            assert_eq!(stored.source_version.identity_sha256, "v2");
            assert_eq!(stored.rows, 250);
            assert!((stored.columns[0].distinct_count().unwrap() as i64 - 250).abs() <= 5);
            let events = store.audit_events(None, 100).unwrap();
            let statistics_events = events
                .iter()
                .filter(|event| event.action == "statistics")
                .collect::<Vec<_>>();
            assert_eq!(statistics_events.len(), 2);
            assert_eq!(statistics_events[1].details["source_version"], "v2");
            assert_eq!(statistics_events[1].details["depth"], "full");
            assert_eq!(statistics_events[1].details["rows"], "250");
            assert_eq!(
                store.table_by_name("local", "default", "orders").unwrap(),
                Some(table.clone())
            );
            assert_eq!(
                store.table_by_name("local", "default", "nope").unwrap(),
                None
            );
            // An unknown table has nowhere to keep statistics.
            let orphan = TableStatistics {
                table_id: TableId::new("table-x").unwrap(),
                ..statistics_for(&table, "v3", 1)
            };
            assert!(store.put_table_statistics("analyzer", &orphan).is_err());
            table
        };
        // Durable across a reopen.
        let store = CatalogStore::open(&path).unwrap();
        assert_eq!(
            store.table_statistics(table.id()).unwrap().unwrap().rows,
            250
        );
        assert!(
            store
                .delete_table_statistics("analyzer", table.id())
                .unwrap()
        );
        assert!(
            !store
                .delete_table_statistics("analyzer", table.id())
                .unwrap()
        );
        store
            .put_table_statistics("analyzer", &statistics_for(&table, "v3", 7))
            .unwrap();
        // Deleting the table deletes its statistics.
        store
            .delete_table("test", table.id(), table.revision())
            .unwrap();
        assert_eq!(store.table_statistics(table.id()).unwrap(), None);
        drop(store);
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn duplicate_catalog_is_reported_as_conflict_semantics() {
        let store = CatalogStore::open_in_memory().unwrap();
        let (catalog, _, _) = values();
        store.create_catalog("test", &catalog).unwrap();
        assert!(
            store
                .create_catalog("test", &catalog)
                .unwrap_err()
                .to_string()
                .contains("already exists")
        );
    }
    #[test]
    fn stale_revision_rolls_back() {
        let store = CatalogStore::open_in_memory().unwrap();
        let (catalog, _, _) = seed(&store);
        let active = catalog.transition(CatalogLifecycle::Active).unwrap();
        store
            .replace_catalog("test", catalog.revision(), &active)
            .unwrap();
        let before = store.audit_events(None, 100).unwrap().len();
        assert!(
            store
                .replace_catalog("test", catalog.revision(), &active)
                .unwrap_err()
                .to_string()
                .contains("revision conflict")
        );
        assert_eq!(store.audit_events(None, 100).unwrap().len(), before);
    }
    #[test]
    fn restrict_and_cascade_are_explicit() {
        let store = CatalogStore::open_in_memory().unwrap();
        let (catalog, schema, table) = seed(&store);
        assert!(
            store
                .delete_schema(
                    "test",
                    schema.id(),
                    schema.revision(),
                    CascadePolicy::Restrict
                )
                .is_err()
        );
        assert!(store.table(table.id()).unwrap().is_some());
        store
            .delete_catalog(
                "test",
                catalog.id(),
                catalog.revision(),
                CascadePolicy::Cascade,
            )
            .unwrap();
        assert!(store.schema(schema.id()).unwrap().is_none());
        assert!(store.table(table.id()).unwrap().is_none());
        let events = store.audit_events(None, 10).unwrap();
        let deletion = events.last().unwrap();
        assert_eq!(deletion.details["cascade"], "true");
        assert_eq!(deletion.details["direct_children_deleted"], "1");
    }
    #[test]
    fn orphan_creation_rolls_back() {
        let store = CatalogStore::open_in_memory().unwrap();
        let (_, schema, _) = values();
        assert!(store.create_schema("test", &schema).is_err());
        assert!(store.audit_events(None, 10).unwrap().is_empty());
    }

    #[test]
    fn snapshot_identity_is_durable_and_content_addressed() {
        let directory =
            std::env::temp_dir().join(format!("kaveon-catalog-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&directory).unwrap();
        let path = directory.join("catalog.db");
        let identity = {
            let store = CatalogStore::open(&path).unwrap();
            let empty = store.snapshot_identity().unwrap();
            seed(&store);
            let populated = store.snapshot_identity().unwrap();
            assert_ne!(empty, populated);
            populated
        };
        assert_eq!(
            CatalogStore::open(&path)
                .unwrap()
                .snapshot_identity()
                .unwrap(),
            identity
        );
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn equivalent_catalogs_have_the_same_snapshot_identity() {
        let first = CatalogStore::open_in_memory().unwrap();
        let second = CatalogStore::open_in_memory().unwrap();
        seed(&first);
        seed(&second);
        assert_eq!(
            first.snapshot_identity().unwrap(),
            second.snapshot_identity().unwrap()
        );
    }

    #[test]
    fn failed_mutation_does_not_advance_snapshot_identity() {
        let store = CatalogStore::open_in_memory().unwrap();
        let (catalog, _, _) = seed(&store);
        let before = store.snapshot_identity().unwrap();
        let active = catalog.transition(CatalogLifecycle::Active).unwrap();
        store
            .replace_catalog("test", catalog.revision(), &active)
            .unwrap();
        let committed = store.snapshot_identity().unwrap();
        assert_ne!(before, committed);
        assert!(
            store
                .replace_catalog("test", catalog.revision(), &active)
                .is_err()
        );
        assert_eq!(store.snapshot_identity().unwrap(), committed);
    }

    #[test]
    fn replica_snapshot_installs_atomically_and_rejects_digest_tampering() {
        let coordinator = CatalogStore::open_in_memory().unwrap();
        let (_, _, table) = seed(&coordinator);
        let snapshot = coordinator.export_replica_snapshot().unwrap();
        let worker = CatalogStore::open_in_memory().unwrap();
        worker.install_replica_snapshot(&snapshot).unwrap();
        assert_eq!(worker.snapshot_identity().unwrap(), snapshot.identity);
        assert_eq!(worker.table(table.id()).unwrap().unwrap().name(), "orders");

        let before = worker.snapshot_identity().unwrap();
        let mut tampered = snapshot;
        tampered.identity = "sha256:tampered".into();
        assert!(worker.install_replica_snapshot(&tampered).is_err());
        assert_eq!(worker.snapshot_identity().unwrap(), before);
        assert!(worker.table(table.id()).unwrap().is_some());
    }
}
