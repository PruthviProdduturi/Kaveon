//! Catalog access: which principal may see, query and change which catalog.
//!
//! One evaluator answers every path — catalog listing, schema, table and
//! column discovery, the metadata statements, statement binding and the
//! definitions API — from one immutable [`GrantSet`] loaded from the
//! `catalog_grants` typed family of the KaveonDB product transaction
//! authority. The family is committed through the same product
//! transaction machinery as every other product record: one conditional
//! publication per change, a revision on every row (CAS on update and
//! delete) and an audit line naming the actor. Nothing about a grant is
//! written to PostgreSQL or to the coordinator's SQLite catalog.
//!
//! The policy is default deny: a principal without a grant sees no
//! catalog. An Admin's access is a property of the role, never of a grant,
//! so no grant or revoke can reduce it and the last Admin cannot be locked
//! out. `KaveonDB` is reserved: it is never grantable, and it stays hidden
//! from everyone — Admins included — until its read-only `product` and
//! `catalog` views exist ([`kaveondb_views_available`]).
//!
//! Enforcement works by projection, not by a second check at each site: a
//! principal's statement, metadata call and definitions read all resolve
//! against [`Scope::restrict`], a view of the published catalog that holds
//! only the catalogs the principal may see. A hidden catalog therefore fails
//! exactly as an absent one does (`catalog 'x' not found`), at bind time,
//! and the text never confirms that the catalog exists.
use std::{
    collections::{BTreeMap, HashSet},
    sync::{Arc, RwLock},
};

use kaveon_catalog::{
    product_commit::{CommitOutcome, ProductCatalogCommit},
    product_manifest::{
        CatalogChange, CatalogSnapshot, TypedColumnType, TypedRow, TypedTableSchema, TypedValue,
    },
    product_transaction::ProductTransaction,
};
use serde::{Deserialize, Serialize};
use tokio::sync::Mutex;

use crate::{
    PublishedCatalog,
    audit::{AuditLedger, AuditRecord, KIND_CATALOG_ACCESS_GRANT, KIND_CATALOG_ACCESS_REVOKE},
    security::{Identity, Role},
};

/// The typed family in the KaveonDB transaction authority that holds the
/// grants. Reserved: the generic transaction API refuses to stage a change
/// naming it, so a grant is only ever written by [`CatalogAccess`].
pub const GRANTS_TABLE: &str = "catalog_grants";

/// The transactional application and catalog authority. Never grantable;
/// hidden until its read-only SQL views exist.
pub const RESERVED_CATALOG: &str = "KaveonDB";

const MAX_PRINCIPAL_LEN: usize = 96;
const MAX_CATALOG_LEN: usize = 64;

/// Whether the read-only `KaveonDB.system` projection is available to
/// administrators. The local materializer publishes the transactional
/// families as immutable Parquet snapshots; mutations refresh the snapshot
/// before the next catalog publication.
pub const fn kaveondb_views_available() -> bool {
    true
}

/// Whether `name` is the reserved transactional catalog. Compared without
/// case so a differently-cased registration cannot stand in for it.
pub fn is_reserved(name: &str) -> bool {
    name.eq_ignore_ascii_case(RESERVED_CATALOG)
}

/// What a grant allows on a catalog, in increasing order.
///
/// | Level | Allows |
/// |---|---|
/// | `browse` | list the catalog, its schemas, tables and columns; `DESCRIBE`; `SHOW …` |
/// | `query` | `browse` and SQL statements that read the catalog |
/// | `manage` | `query` and catalog DDL inside it: schemas, tables, their locations, layouts and shapes |
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Access {
    Browse,
    Query,
    Manage,
}

impl Access {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Browse => "browse",
            Self::Query => "query",
            Self::Manage => "manage",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "browse" => Some(Self::Browse),
            "query" => Some(Self::Query),
            "manage" => Some(Self::Manage),
            _ => None,
        }
    }
}

/// The most a role reaches on a granted catalog; `None` for Admin, whose
/// access does not go through grants. `reader` browses; `analyst` — the
/// Engine role both the platform's Analyst and Editor arrive as — reaches
/// `manage`, which is what it could do before grants existed. The platform
/// keeps its own Editor gate on the registration routes; nothing here
/// widens a role.
pub const fn role_ceiling(role: Role) -> Option<Access> {
    match role {
        Role::Reader => Some(Access::Browse),
        Role::Analyst => Some(Access::Manage),
        Role::Admin => None,
    }
}

/// One row of the family.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Grant {
    pub principal: String,
    pub catalog: String,
    pub access: Access,
    /// The row's revision: what an update or revoke must send back.
    pub revision: u64,
    pub granted_by: String,
    pub granted_at_ms: u64,
}

impl Grant {
    fn primary_key(catalog: &str, principal: &str) -> String {
        format!("{catalog}/{principal}")
    }

    fn row(&self) -> TypedRow {
        TypedRow {
            primary_key: Self::primary_key(&self.catalog, &self.principal),
            revision: self.revision,
            columns: BTreeMap::from([
                (
                    "principal".to_owned(),
                    TypedValue::String(self.principal.clone()),
                ),
                (
                    "catalog".to_owned(),
                    TypedValue::String(self.catalog.clone()),
                ),
                (
                    "access".to_owned(),
                    TypedValue::String(self.access.as_str().to_owned()),
                ),
                (
                    "granted_by".to_owned(),
                    TypedValue::String(self.granted_by.clone()),
                ),
                (
                    "granted_at_ms".to_owned(),
                    TypedValue::Integer(i64::try_from(self.granted_at_ms).unwrap_or(i64::MAX)),
                ),
            ]),
            unique_keys: BTreeMap::new(),
        }
    }

    fn from_row(row: &TypedRow) -> Result<Self, String> {
        let text = |name: &str| match row.columns.get(name) {
            Some(TypedValue::String(value)) => Ok(value.clone()),
            _ => Err(format!("catalog grant row is missing '{name}'")),
        };
        let access = text("access")?;
        Ok(Self {
            principal: text("principal")?,
            catalog: text("catalog")?,
            access: Access::parse(&access)
                .ok_or_else(|| format!("catalog grant row has unknown access '{access}'"))?,
            revision: row.revision,
            granted_by: text("granted_by")?,
            granted_at_ms: match row.columns.get("granted_at_ms") {
                Some(TypedValue::Integer(value)) => u64::try_from(*value).unwrap_or_default(),
                _ => return Err("catalog grant row is missing 'granted_at_ms'".into()),
            },
        })
    }
}

fn schema() -> TypedTableSchema {
    TypedTableSchema {
        columns: BTreeMap::from([
            ("principal".to_owned(), TypedColumnType::String),
            ("catalog".to_owned(), TypedColumnType::String),
            ("access".to_owned(), TypedColumnType::String),
            ("granted_by".to_owned(), TypedColumnType::String),
            ("granted_at_ms".to_owned(), TypedColumnType::Integer),
        ]),
        primary_key: "primary_key".into(),
        unique_keys: Default::default(),
    }
}

/// The grants in force, read from one immutable snapshot of the authority.
#[derive(Clone, Debug, Default)]
pub struct GrantSet {
    /// By `(principal, catalog)`.
    grants: BTreeMap<(String, String), Grant>,
    /// The snapshot the grants were read from; `None` before the store is
    /// read or when it is not configured.
    pub generation: Option<u64>,
    pub snapshot_id: Option<String>,
}

impl GrantSet {
    pub fn from_snapshot(snapshot: &CatalogSnapshot) -> Result<Self, String> {
        let mut grants = BTreeMap::new();
        if let Some(rows) = snapshot.typed_rows.get(GRANTS_TABLE) {
            for row in rows.values() {
                let grant = Grant::from_row(row)?;
                grants.insert((grant.principal.clone(), grant.catalog.clone()), grant);
            }
        }
        Ok(Self {
            grants,
            generation: Some(snapshot.generation),
            snapshot_id: Some(snapshot.snapshot_id.clone()),
        })
    }

    pub fn get(&self, principal: &str, catalog: &str) -> Option<&Grant> {
        self.grants.get(&(principal.to_owned(), catalog.to_owned()))
    }

    pub fn iter(&self) -> impl Iterator<Item = &Grant> {
        self.grants.values()
    }

    pub fn for_principal<'a>(&'a self, principal: &'a str) -> impl Iterator<Item = &'a Grant> {
        self.grants
            .range((principal.to_owned(), String::new())..)
            .take_while(move |((owner, _), _)| owner == principal)
            .map(|(_, grant)| grant)
    }

    pub fn len(&self) -> usize {
        self.grants.len()
    }

    pub fn is_empty(&self) -> bool {
        self.grants.is_empty()
    }
}

/// The evaluator's answer for one identity: what it may do on each catalog.
#[derive(Clone, Debug)]
pub struct Scope {
    role: Role,
    /// Effective level per granted catalog; unused for Admin.
    levels: BTreeMap<String, Access>,
}

impl Scope {
    fn new(identity: &Identity, grants: &GrantSet) -> Self {
        let levels = match role_ceiling(identity.role) {
            None => BTreeMap::new(),
            Some(ceiling) => grants
                .for_principal(&identity.principal)
                .filter(|grant| !is_reserved(&grant.catalog))
                .map(|grant| (grant.catalog.clone(), grant.access.min(ceiling)))
                .collect(),
        };
        Self {
            role: identity.role,
            levels,
        }
    }

    /// The level on `catalog`, or `None` when it must stay invisible.
    pub fn level(&self, catalog: &str) -> Option<Access> {
        if is_reserved(catalog) {
            return (self.role == Role::Admin && kaveondb_views_available())
                .then_some(Access::Query);
        }
        if self.role == Role::Admin {
            return Some(Access::Manage);
        }
        self.levels.get(catalog).copied()
    }

    /// List the catalog and discover its schemas, tables and columns.
    pub fn can_see(&self, catalog: &str) -> bool {
        self.level(catalog).is_some()
    }

    /// Run statements that read the catalog.
    pub fn can_query(&self, catalog: &str) -> bool {
        self.level(catalog) >= Some(Access::Query)
    }

    /// Change schemas and tables inside the catalog.
    pub fn can_manage(&self, catalog: &str) -> bool {
        self.level(catalog) >= Some(Access::Manage)
    }

    pub const fn role(&self) -> Role {
        self.role
    }

    /// The names in `registered` this identity may see.
    pub fn visible<'a>(&self, registered: impl IntoIterator<Item = &'a str>) -> Vec<String> {
        registered
            .into_iter()
            .filter(|name| self.can_see(name))
            .map(str::to_owned)
            .collect()
    }

    /// The published catalog as this identity sees it: the same providers
    /// and snapshot identity, exposing only its visible catalogs. The
    /// binder, planner and metadata statements resolve against this, so an
    /// invisible catalog fails as an absent one.
    pub fn restrict(&self, published: &Arc<PublishedCatalog>) -> Arc<PublishedCatalog> {
        let registered = published.manager.registered_catalog_names();
        let visible = self.visible(registered.iter().map(String::as_str));
        if visible.len() == registered.len() {
            return Arc::clone(published);
        }
        Arc::new(PublishedCatalog {
            manager: published.manager.restricted(visible),
            snapshot_id: published.snapshot_id.clone(),
        })
    }
}

/// What a grant or revoke asks for.
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GrantRequest {
    pub principal: String,
    pub catalog: String,
    pub access: Access,
    /// The current revision when changing an existing grant; absent for a
    /// new one. A mismatch is a conflict.
    #[serde(default)]
    pub revision: Option<u64>,
}

#[derive(Debug)]
pub enum AccessError {
    /// The KaveonDB transaction authority is not configured on this node.
    Disabled,
    Reserved,
    Invalid(String),
    /// The revision sent does not match the row, or the head moved.
    Conflict(String),
    Unavailable(String),
    /// The publication may or may not have landed; reload before retrying.
    Indeterminate,
}

impl std::fmt::Display for AccessError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Disabled => formatter.write_str(
                "catalog access grants need the KaveonDB transaction store, which is not configured on this Engine",
            ),
            Self::Reserved => write!(
                formatter,
                "{RESERVED_CATALOG} is the transactional authority: administrators only, never granted"
            ),
            Self::Invalid(message) | Self::Conflict(message) | Self::Unavailable(message) => {
                formatter.write_str(message)
            }
            Self::Indeterminate => formatter.write_str(
                "the grant's publication is neither confirmed committed nor confirmed rolled back; reload and compare before retrying",
            ),
        }
    }
}

/// The result of a committed grant.
#[derive(Debug, Serialize)]
pub struct GrantOutcome {
    pub grant: Grant,
    pub revision_before: Option<u64>,
    pub generation: u64,
}

/// The store and evaluator.
pub struct CatalogAccess {
    store: Option<ProductCatalogCommit>,
    current: RwLock<Arc<GrantSet>>,
    /// Local mutations one at a time, so two grants from this coordinator
    /// never race each other for the head.
    writes: Mutex<()>,
}

impl CatalogAccess {
    /// No authority: no grants can exist, so only Admins see any catalog.
    pub fn disabled() -> Self {
        Self {
            store: None,
            current: RwLock::new(Arc::new(GrantSet::default())),
            writes: Mutex::new(()),
        }
    }

    /// Over the configured authority, with the family loaded.
    pub async fn open(store: ProductCatalogCommit) -> Result<Self, AccessError> {
        let access = Self {
            store: Some(store),
            current: RwLock::new(Arc::new(GrantSet::default())),
            writes: Mutex::new(()),
        };
        access.reload().await?;
        Ok(access)
    }

    pub fn is_enabled(&self) -> bool {
        self.store.is_some()
    }

    /// An evaluator over `grants` with no authority behind it, for tests
    /// of the paths that consult it; nothing can be granted or revoked.
    #[cfg(test)]
    pub(crate) fn preset<'a>(grants: impl IntoIterator<Item = (&'a str, &'a str, Access)>) -> Self {
        let mut set = GrantSet::default();
        for (principal, catalog, access) in grants {
            set.grants.insert(
                (principal.to_owned(), catalog.to_owned()),
                Grant {
                    principal: principal.to_owned(),
                    catalog: catalog.to_owned(),
                    access,
                    revision: 1,
                    granted_by: "preset".into(),
                    granted_at_ms: 0,
                },
            );
        }
        Self {
            store: None,
            current: RwLock::new(Arc::new(set)),
            writes: Mutex::new(()),
        }
    }

    pub fn current(&self) -> Arc<GrantSet> {
        Arc::clone(&self.current.read().expect("grant set lock is not poisoned"))
    }

    fn publish(&self, snapshot: &CatalogSnapshot) -> Result<Arc<GrantSet>, AccessError> {
        let set = Arc::new(GrantSet::from_snapshot(snapshot).map_err(AccessError::Unavailable)?);
        *self
            .current
            .write()
            .expect("grant set lock is not poisoned") = Arc::clone(&set);
        Ok(set)
    }

    /// Re-read the family from the authority's head: at start, after every
    /// local change, and periodically for changes another coordinator made.
    pub async fn reload(&self) -> Result<Arc<GrantSet>, AccessError> {
        let store = self.store.as_ref().ok_or(AccessError::Disabled)?;
        let snapshot = store.read_current().await.map_err(|error| {
            AccessError::Unavailable(format!("cannot read the catalog grants: {error:?}"))
        })?;
        self.publish(&snapshot)
    }

    // --- the evaluator ---

    pub fn evaluate(&self, identity: &Identity) -> Scope {
        Scope::new(identity, &self.current())
    }

    pub fn can_see(&self, identity: &Identity, catalog: &str) -> bool {
        self.evaluate(identity).can_see(catalog)
    }

    pub fn can_query(&self, identity: &Identity, catalog: &str) -> bool {
        self.evaluate(identity).can_query(catalog)
    }

    pub fn can_manage(&self, identity: &Identity, catalog: &str) -> bool {
        self.evaluate(identity).can_manage(catalog)
    }

    // --- the store ---

    /// Create or change one grant. `actor` must be an Admin (the route
    /// checks); the audit line names them.
    pub async fn grant(
        &self,
        audit: &AuditLedger,
        actor: &Identity,
        request: GrantRequest,
    ) -> Result<GrantOutcome, AccessError> {
        let store = self.store.as_ref().ok_or(AccessError::Disabled)?;
        validate_names(&request.principal, &request.catalog)?;
        let _serialized = self.writes.lock().await;
        let mut transaction = begin(store, "grant").await?;
        ensure_schema(&mut transaction)?;
        let existing = transaction
            .snapshot()
            .typed_rows
            .get(GRANTS_TABLE)
            .and_then(|rows| rows.get(&Grant::primary_key(&request.catalog, &request.principal)))
            .map(Grant::from_row)
            .transpose()
            .map_err(AccessError::Unavailable)?;
        let now = crate::audit::unix_ms();
        let change = match (&existing, request.revision) {
            (None, None) => CatalogChange::InsertTypedRow {
                table: GRANTS_TABLE.into(),
                row: Grant {
                    principal: request.principal.clone(),
                    catalog: request.catalog.clone(),
                    access: request.access,
                    revision: 1,
                    granted_by: actor.principal.clone(),
                    granted_at_ms: now,
                }
                .row(),
            },
            (None, Some(revision)) => {
                return Err(AccessError::Conflict(format!(
                    "no grant for {} on {} at revision {revision}; it was revoked, reload",
                    request.principal, request.catalog
                )));
            }
            (Some(current), None) => {
                return Err(AccessError::Conflict(format!(
                    "{} already has {} on {} at revision {}; send that revision to change it",
                    current.principal,
                    current.access.as_str(),
                    current.catalog,
                    current.revision
                )));
            }
            (Some(current), Some(revision)) => {
                if current.revision != revision {
                    return Err(AccessError::Conflict(format!(
                        "the grant for {} on {} is at revision {}, not {revision}; reload",
                        current.principal, current.catalog, current.revision
                    )));
                }
                CatalogChange::UpdateTypedRow {
                    table: GRANTS_TABLE.into(),
                    expected_revision: revision,
                    row: Grant {
                        principal: request.principal.clone(),
                        catalog: request.catalog.clone(),
                        access: request.access,
                        revision: revision + 1,
                        granted_by: actor.principal.clone(),
                        granted_at_ms: now,
                    }
                    .row(),
                }
            }
        };
        stage(&mut transaction, change)?;
        let snapshot = commit(transaction, actor, "grant").await?;
        let set = self.publish(&snapshot)?;
        let grant = set
            .get(&request.principal, &request.catalog)
            .cloned()
            .ok_or_else(|| {
                AccessError::Unavailable(
                    "the committed grant is not in the published family".into(),
                )
            })?;
        let revision_before = existing.as_ref().map(|current| current.revision);
        audit.record(AuditRecord {
            catalog: Some(grant.catalog.clone()),
            object_type: Some("catalog_grant".into()),
            object_id: Some(Grant::primary_key(&grant.catalog, &grant.principal)),
            revision_before,
            revision_after: Some(grant.revision),
            details: Some(serde_json::json!({
                "principal": grant.principal,
                "access": grant.access,
                "access_before": existing.as_ref().map(|current| current.access),
                "generation": snapshot.generation,
            })),
            ..AuditRecord::new(KIND_CATALOG_ACCESS_GRANT).by(actor)
        });
        Ok(GrantOutcome {
            grant,
            revision_before,
            generation: snapshot.generation,
        })
    }

    /// Remove one grant at `expected_revision`.
    pub async fn revoke(
        &self,
        audit: &AuditLedger,
        actor: &Identity,
        principal: &str,
        catalog: &str,
        expected_revision: u64,
    ) -> Result<Grant, AccessError> {
        let store = self.store.as_ref().ok_or(AccessError::Disabled)?;
        validate_names(principal, catalog)?;
        let _serialized = self.writes.lock().await;
        let mut transaction = begin(store, "revoke").await?;
        let primary_key = Grant::primary_key(catalog, principal);
        let Some(current) = transaction
            .snapshot()
            .typed_rows
            .get(GRANTS_TABLE)
            .and_then(|rows| rows.get(&primary_key))
            .map(Grant::from_row)
            .transpose()
            .map_err(AccessError::Unavailable)?
        else {
            return Err(AccessError::Conflict(format!(
                "no grant for {principal} on {catalog}; it may already be revoked, reload"
            )));
        };
        if current.revision != expected_revision {
            return Err(AccessError::Conflict(format!(
                "the grant for {principal} on {catalog} is at revision {}, not {expected_revision}; reload",
                current.revision
            )));
        }
        stage(
            &mut transaction,
            CatalogChange::DeleteTypedRow {
                table: GRANTS_TABLE.into(),
                primary_key,
                expected_revision,
            },
        )?;
        let snapshot = commit(transaction, actor, "revoke").await?;
        self.publish(&snapshot)?;
        audit.record(AuditRecord {
            catalog: Some(current.catalog.clone()),
            object_type: Some("catalog_grant".into()),
            object_id: Some(Grant::primary_key(&current.catalog, &current.principal)),
            revision_before: Some(current.revision),
            revision_after: None,
            details: Some(serde_json::json!({
                "principal": current.principal,
                "access_before": current.access,
                "generation": snapshot.generation,
            })),
            ..AuditRecord::new(KIND_CATALOG_ACCESS_REVOKE).by(actor)
        });
        Ok(current)
    }

    /// Record several new grants in one publication, skipping pairs that
    /// already have one. What the reconciliation of an open deployment
    /// applies once the Admin has reviewed the proposal; each grant is
    /// audited on its own.
    pub async fn grant_many(
        &self,
        audit: &AuditLedger,
        actor: &Identity,
        requests: Vec<GrantRequest>,
    ) -> Result<Vec<Grant>, AccessError> {
        let store = self.store.as_ref().ok_or(AccessError::Disabled)?;
        for request in &requests {
            validate_names(&request.principal, &request.catalog)?;
            if request.revision.is_some() {
                return Err(AccessError::Invalid(
                    "an import records new grants only; change an existing grant on its own".into(),
                ));
            }
        }
        let _serialized = self.writes.lock().await;
        let mut transaction = begin(store, "import").await?;
        ensure_schema(&mut transaction)?;
        let now = crate::audit::unix_ms();
        let mut seen = HashSet::new();
        let mut recorded = Vec::new();
        for request in requests {
            let primary_key = Grant::primary_key(&request.catalog, &request.principal);
            if !seen.insert(primary_key.clone())
                || transaction
                    .snapshot()
                    .typed_rows
                    .get(GRANTS_TABLE)
                    .is_some_and(|rows| rows.contains_key(&primary_key))
            {
                continue;
            }
            let grant = Grant {
                principal: request.principal,
                catalog: request.catalog,
                access: request.access,
                revision: 1,
                granted_by: actor.principal.clone(),
                granted_at_ms: now,
            };
            stage(
                &mut transaction,
                CatalogChange::InsertTypedRow {
                    table: GRANTS_TABLE.into(),
                    row: grant.row(),
                },
            )?;
            recorded.push(grant);
        }
        if recorded.is_empty() {
            return Ok(recorded);
        }
        let snapshot = commit(transaction, actor, "import").await?;
        self.publish(&snapshot)?;
        for grant in &recorded {
            audit.record(AuditRecord {
                catalog: Some(grant.catalog.clone()),
                object_type: Some("catalog_grant".into()),
                object_id: Some(Grant::primary_key(&grant.catalog, &grant.principal)),
                revision_before: None,
                revision_after: Some(grant.revision),
                details: Some(serde_json::json!({
                    "principal": grant.principal,
                    "access": grant.access,
                    "imported": true,
                    "generation": snapshot.generation,
                })),
                ..AuditRecord::new(KIND_CATALOG_ACCESS_GRANT).by(actor)
            });
        }
        Ok(recorded)
    }
}

fn validate_names(principal: &str, catalog: &str) -> Result<(), AccessError> {
    if is_reserved(catalog) {
        return Err(AccessError::Reserved);
    }
    let clean = |value: &str, what: &str, limit: usize| {
        if value.is_empty()
            || value != value.trim()
            || value.len() > limit
            || value.chars().any(|c| c.is_control() || c.is_whitespace())
        {
            return Err(AccessError::Invalid(format!(
                "{what} must be a non-empty name of at most {limit} characters with no whitespace"
            )));
        }
        Ok(())
    };
    clean(principal, "principal", MAX_PRINCIPAL_LEN)?;
    clean(catalog, "catalog", MAX_CATALOG_LEN)?;
    if catalog.contains('/') {
        return Err(AccessError::Invalid(
            "catalog names cannot contain '/'".into(),
        ));
    }
    Ok(())
}

async fn begin(
    store: &ProductCatalogCommit,
    action: &str,
) -> Result<ProductTransaction, AccessError> {
    let id = uuid::Uuid::new_v4().simple().to_string();
    ProductTransaction::begin(
        store.clone(),
        format!("snapshot-{id}"),
        format!("operation-catalog-access-{action}-{id}"),
        "0".repeat(64),
    )
    .await
    .map_err(|error| AccessError::Unavailable(format!("cannot open the catalog grants: {error}")))
}

fn ensure_schema(transaction: &mut ProductTransaction) -> Result<(), AccessError> {
    if transaction
        .snapshot()
        .typed_schemas
        .contains_key(GRANTS_TABLE)
    {
        return Ok(());
    }
    stage(
        transaction,
        CatalogChange::DefineTypedSchema {
            table: GRANTS_TABLE.into(),
            schema: schema(),
        },
    )
}

fn stage(transaction: &mut ProductTransaction, change: CatalogChange) -> Result<(), AccessError> {
    transaction.stage(change).map_err(|error| {
        let message = error.to_string();
        if message.contains("stale") {
            AccessError::Conflict(message)
        } else {
            AccessError::Invalid(message)
        }
    })
}

async fn commit(
    mut transaction: ProductTransaction,
    actor: &Identity,
    action: &str,
) -> Result<CatalogSnapshot, AccessError> {
    transaction
        .bind_request_digest(format!("catalog-access:{action}:{}", actor.principal).as_bytes())
        .map_err(|error| AccessError::Invalid(error.to_string()))?;
    match transaction.commit().await {
        Ok(CommitOutcome::Committed(snapshot) | CommitOutcome::Replayed(snapshot)) => Ok(snapshot),
        Ok(CommitOutcome::Conflict) => Err(AccessError::Conflict(
            "the catalog grants changed under this request; reload and retry".into(),
        )),
        Ok(CommitOutcome::Rejected) => Err(AccessError::Invalid(
            "the transaction authority rejected the grant".into(),
        )),
        Ok(CommitOutcome::Indeterminate) => Err(AccessError::Indeterminate),
        Err(error) => Err(AccessError::Unavailable(error.to_string())),
    }
}

/// Whether a client-staged change would touch the grants family.
pub fn touches_grants(change: &CatalogChange) -> bool {
    match change {
        CatalogChange::InsertTypedRow { table, .. }
        | CatalogChange::UpdateTypedRow { table, .. }
        | CatalogChange::DeleteTypedRow { table, .. }
        | CatalogChange::DefineTypedSchema { table, .. } => table == GRANTS_TABLE,
        _ => false,
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use kaveon_catalog::product_metrics::TransactionMetrics;
    use kaveon_storage::AdlsConditionalCommit;
    use object_store::memory::InMemory;

    pub(crate) fn identity(principal: &str, role: Role) -> Identity {
        Identity {
            principal: principal.into(),
            display_identity: None,
            role,
        }
    }

    /// An in-memory authority with the genesis snapshot committed.
    pub(crate) async fn memory_store() -> ProductCatalogCommit {
        let store = ProductCatalogCommit::new(
            AdlsConditionalCommit::new(Arc::new(InMemory::new())),
            "product",
            Arc::new(TransactionMetrics::default()),
        )
        .unwrap();
        store
            .initialize(CatalogSnapshot::empty("genesis").unwrap())
            .await;
        store
    }

    fn request(
        principal: &str,
        catalog: &str,
        access: Access,
        revision: Option<u64>,
    ) -> GrantRequest {
        GrantRequest {
            principal: principal.into(),
            catalog: catalog.into(),
            access,
            revision,
        }
    }

    #[tokio::test]
    async fn default_deny_and_the_role_mapping() {
        let access = CatalogAccess::open(memory_store().await).await.unwrap();
        let audit = AuditLedger::disabled();
        let admin = identity("root", Role::Admin);
        let analyst = identity("ana", Role::Analyst);
        let reader = identity("ray", Role::Reader);
        // Nothing granted: no non-admin sees anything; the Admin sees all
        // but the reserved catalog.
        for who in [&analyst, &reader] {
            assert!(!access.can_see(who, "OpenSource"));
            assert!(!access.can_query(who, "OpenSource"));
            assert!(!access.can_manage(who, "OpenSource"));
        }
        assert!(access.can_manage(&admin, "OpenSource"));
        assert!(access.can_see(&admin, RESERVED_CATALOG));
        assert!(access.can_query(&admin, RESERVED_CATALOG));
        assert!(access.can_see(&admin, "kaveondb"));

        access
            .grant(
                &audit,
                &admin,
                request("ana", "OpenSource", Access::Query, None),
            )
            .await
            .unwrap();
        access
            .grant(
                &audit,
                &admin,
                request("ana", "Kaveon", Access::Manage, None),
            )
            .await
            .unwrap();
        access
            .grant(
                &audit,
                &admin,
                request("ray", "OpenSource", Access::Manage, None),
            )
            .await
            .unwrap();
        let ana = access.evaluate(&analyst);
        assert_eq!(ana.level("OpenSource"), Some(Access::Query));
        assert!(ana.can_query("OpenSource") && !ana.can_manage("OpenSource"));
        assert!(ana.can_manage("Kaveon"));
        assert!(!ana.can_see("Private"));
        // The reader's ceiling is browse whatever the grant says.
        let ray = access.evaluate(&reader);
        assert_eq!(ray.level("OpenSource"), Some(Access::Browse));
        assert!(ray.can_see("OpenSource") && !ray.can_query("OpenSource"));
        assert!(!ray.can_see("Kaveon"));
        assert_eq!(
            ana.visible(["Kaveon", "OpenSource", "Private", RESERVED_CATALOG]),
            vec!["Kaveon".to_owned(), "OpenSource".into()]
        );
    }

    #[tokio::test]
    async fn revisions_conflict_when_stale_and_the_reserved_catalog_is_refused() {
        let access = CatalogAccess::open(memory_store().await).await.unwrap();
        let audit = AuditLedger::disabled();
        let admin = identity("root", Role::Admin);
        let first = access
            .grant(
                &audit,
                &admin,
                request("ana", "OpenSource", Access::Browse, None),
            )
            .await
            .unwrap();
        assert_eq!(first.grant.revision, 1);
        assert_eq!(first.revision_before, None);
        // A second create without the revision, a change with the wrong
        // revision, and a revoke at the wrong revision all conflict.
        for (request, expected) in [
            (
                request("ana", "OpenSource", Access::Query, None),
                "revision 1",
            ),
            (
                request("ana", "OpenSource", Access::Query, Some(7)),
                "at revision 1, not 7",
            ),
            (
                request("bob", "OpenSource", Access::Query, Some(1)),
                "revoked",
            ),
        ] {
            let error = access.grant(&audit, &admin, request).await.unwrap_err();
            assert!(
                matches!(&error, AccessError::Conflict(message) if message.contains(expected)),
                "{error}"
            );
        }
        let changed = access
            .grant(
                &audit,
                &admin,
                request("ana", "OpenSource", Access::Query, Some(1)),
            )
            .await
            .unwrap();
        assert_eq!(changed.grant.revision, 2);
        assert_eq!(changed.revision_before, Some(1));
        assert_eq!(changed.grant.access, Access::Query);
        let stale = access
            .revoke(&audit, &admin, "ana", "OpenSource", 1)
            .await
            .unwrap_err();
        assert!(matches!(stale, AccessError::Conflict(_)));
        let removed = access
            .revoke(&audit, &admin, "ana", "OpenSource", 2)
            .await
            .unwrap();
        assert_eq!(removed.revision, 2);
        assert!(access.current().is_empty());
        assert!(matches!(
            access
                .revoke(&audit, &admin, "ana", "OpenSource", 2)
                .await
                .unwrap_err(),
            AccessError::Conflict(_)
        ));
        for reserved in [RESERVED_CATALOG, "kaveondb", "KAVEONDB"] {
            assert!(matches!(
                access
                    .grant(
                        &audit,
                        &admin,
                        request("ana", reserved, Access::Browse, None)
                    )
                    .await
                    .unwrap_err(),
                AccessError::Reserved
            ));
        }
        assert!(matches!(
            access
                .grant(
                    &audit,
                    &admin,
                    request(" ana", "OpenSource", Access::Browse, None)
                )
                .await
                .unwrap_err(),
            AccessError::Invalid(_)
        ));
        assert!(matches!(
            access
                .grant(
                    &audit,
                    &admin,
                    request("ana", "Open/Source", Access::Browse, None)
                )
                .await
                .unwrap_err(),
            AccessError::Invalid(_)
        ));
    }

    #[tokio::test]
    async fn the_family_survives_a_restart_and_reloads_another_writers_changes() {
        let store = memory_store().await;
        let audit = AuditLedger::disabled();
        let admin = identity("root", Role::Admin);
        let first = CatalogAccess::open(store.clone()).await.unwrap();
        first
            .grant(
                &audit,
                &admin,
                request("ana", "OpenSource", Access::Query, None),
            )
            .await
            .unwrap();
        // A new coordinator over the same authority loads the grant.
        let restarted = CatalogAccess::open(store.clone()).await.unwrap();
        assert!(restarted.can_query(&identity("ana", Role::Analyst), "OpenSource"));
        assert_eq!(restarted.current().generation, first.current().generation);
        // The first coordinator's cache is behind after the other writes.
        // Its next change begins from the durable head, so it is applied
        // to the current family, not the stale cache, and publishes it.
        restarted
            .grant(
                &audit,
                &admin,
                request("bob", "Kaveon", Access::Browse, None),
            )
            .await
            .unwrap();
        assert!(!first.can_see(&identity("bob", Role::Reader), "Kaveon"));
        first
            .grant(
                &audit,
                &admin,
                request("ana", "OpenSource", Access::Manage, Some(1)),
            )
            .await
            .unwrap();
        assert!(first.can_see(&identity("bob", Role::Reader), "Kaveon"));
        // A periodic reload alone also catches up.
        let third = CatalogAccess::open(store.clone()).await.unwrap();
        restarted
            .grant(&audit, &admin, request("cy", "Kaveon", Access::Query, None))
            .await
            .unwrap();
        assert!(!third.can_see(&identity("cy", Role::Analyst), "Kaveon"));
        third.reload().await.unwrap();
        assert!(third.can_query(&identity("cy", Role::Analyst), "Kaveon"));
        assert!(first.can_see(&identity("bob", Role::Reader), "Kaveon"));
        assert!(first.can_manage(&identity("ana", Role::Analyst), "OpenSource"));
    }

    #[tokio::test]
    async fn an_admin_is_never_locked_out_by_grants() {
        let access = CatalogAccess::open(memory_store().await).await.unwrap();
        let audit = AuditLedger::disabled();
        let admin = identity("root", Role::Admin);
        // A grant naming the Admin at the lowest level does not reduce them.
        let outcome = access
            .grant(
                &audit,
                &admin,
                request("root", "OpenSource", Access::Browse, None),
            )
            .await
            .unwrap();
        assert!(access.can_manage(&admin, "OpenSource"));
        assert!(access.can_manage(&admin, "Anything"));
        // Revoking it, including their own, leaves the Admin whole.
        access
            .revoke(&audit, &admin, "root", "OpenSource", outcome.grant.revision)
            .await
            .unwrap();
        assert!(access.current().is_empty());
        assert!(access.can_manage(&admin, "OpenSource"));
        // With no store at all, Admins still reach every catalog.
        let none = CatalogAccess::disabled();
        assert!(!none.is_enabled());
        assert!(none.can_manage(&admin, "OpenSource"));
        assert!(!none.can_see(&identity("ana", Role::Analyst), "OpenSource"));
        assert!(matches!(
            none.grant(
                &audit,
                &admin,
                request("ana", "OpenSource", Access::Browse, None)
            )
            .await
            .unwrap_err(),
            AccessError::Disabled
        ));
    }

    #[tokio::test]
    async fn imports_record_only_new_pairs() {
        let access = CatalogAccess::open(memory_store().await).await.unwrap();
        let audit = AuditLedger::disabled();
        let admin = identity("root", Role::Admin);
        access
            .grant(
                &audit,
                &admin,
                request("ana", "OpenSource", Access::Query, None),
            )
            .await
            .unwrap();
        let recorded = access
            .grant_many(
                &audit,
                &admin,
                vec![
                    request("ana", "OpenSource", Access::Manage, None),
                    request("ana", "Kaveon", Access::Manage, None),
                    request("ray", "OpenSource", Access::Browse, None),
                    request("ray", "OpenSource", Access::Browse, None),
                ],
            )
            .await
            .unwrap();
        assert_eq!(
            recorded
                .iter()
                .map(|grant| (grant.principal.as_str(), grant.catalog.as_str()))
                .collect::<Vec<_>>(),
            vec![("ana", "Kaveon"), ("ray", "OpenSource")]
        );
        // The existing grant kept its level; the family holds three rows.
        assert_eq!(
            access.current().get("ana", "OpenSource").unwrap().access,
            Access::Query
        );
        assert_eq!(access.current().len(), 3);
        assert!(
            access
                .grant_many(&audit, &admin, Vec::new())
                .await
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn client_changes_naming_the_family_are_recognised() {
        assert!(touches_grants(&CatalogChange::DefineTypedSchema {
            table: GRANTS_TABLE.into(),
            schema: schema(),
        }));
        assert!(touches_grants(&CatalogChange::DeleteTypedRow {
            table: GRANTS_TABLE.into(),
            primary_key: "OpenSource/ana".into(),
            expected_revision: 1,
        }));
        assert!(!touches_grants(&CatalogChange::DeleteTypedRow {
            table: "notes".into(),
            primary_key: "x".into(),
            expected_revision: 1,
        }));
    }
}
