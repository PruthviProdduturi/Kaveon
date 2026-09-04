#![deny(clippy::all)]

use kaveon_core::{
    CatalogDefinition, CatalogId, CatalogRevision, KaveonError, Result, SchemaDefinition, SchemaId,
    TableDefinition, TableId,
};
use rusqlite::{Connection, OptionalExtension, Transaction, TransactionBehavior, params};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use std::{
    collections::BTreeMap,
    path::Path,
    sync::{Mutex, MutexGuard},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

const DATABASE_BUSY_TIMEOUT: Duration = Duration::from_secs(5);
const MIGRATION_VERSION: i64 = 1;

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
    fn initialize(connection: Connection) -> Result<Self> {
        connection
            .busy_timeout(DATABASE_BUSY_TIMEOUT)
            .map_err(db_error)?;
        connection
            .execute_batch("PRAGMA foreign_keys = ON; PRAGMA journal_mode = WAL;")
            .map_err(db_error)?;
        migrate(&connection)?;
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
        transaction.commit().map_err(db_error)
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
        CREATE INDEX IF NOT EXISTS idx_audit_object ON audit_events(object_type, object_id, id);
        INSERT OR IGNORE INTO schema_migrations(version) VALUES ({MIGRATION_VERSION});
    "#)).map_err(db_error)
}

fn immediate(connection: &mut Connection) -> Result<Transaction<'_>> {
    connection
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(db_error)
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
}
