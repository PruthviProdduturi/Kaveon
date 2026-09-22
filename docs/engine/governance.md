# Engine governance: catalog access, resource groups and the audit ledger

Which catalogs a principal may reach and how far, what a principal may use
of the coordinator, decided at admission by a named policy, and a durable
record of who did what. Grants live in the KaveonDB transaction authority;
the rest lives on the coordinator's own store — the configuration file,
the state directory — never in PostgreSQL. Sources:
`engine/crates/server/src/catalog_access.rs` (the evaluator and the
store), `engine/crates/server/src/catalog_access_api.rs`,
`engine/crates/server/src/resource_groups.rs`,
`engine/crates/core/src/memory.rs` (the admission order),
`engine/crates/server/src/audit.rs`, `engine/crates/server/src/api.rs`.

## Catalog access

Which principal may see, query and change which catalog. The policy is
**default deny**: a principal with no grant sees no catalog — not on
`GET /v1/catalog`, not in `SHOW CATALOGS`, not in the definitions API,
not in a statement. An administrator reaches every catalog by role; that
access is not a grant and no grant or revoke can reduce it, so the last
administrator cannot be locked out.

### The grant

A grant is one row of the `catalog_grants` typed family in the KaveonDB
product transaction authority — the same store, commit protocol and
revision discipline as every product record: one conditional publication
per change, a revision on every row that an update or revoke must send
back (a mismatch is a conflict), an audit line naming the actor. Nothing
about a grant is written to PostgreSQL or to the coordinator's SQLite
catalog. The generic transaction API refuses to stage a change naming the
family, so a session cannot record a grant for its owner; only the catalog
access routes, administrators only, write it.

| Column | Type | Meaning |
|---|---|---|
| `principal` | string | The principal as the security layer resolves it: the platform's verified email through the bridge, a static principal's name, an Entra object id. Never a client claim. |
| `catalog` | string | The catalog by name. `KaveonDB` is refused. |
| `access` | `browse` \| `query` \| `manage` | The level, below. |
| `granted_by`, `granted_at_ms` | string, integer | The administrator and the time. |
| revision | integer | Starts at 1; advances by one per change; the primary key is `<catalog>/<principal>`. |

The coordinator loads the family at start, republishes it after every
change it makes, and re-reads it on the 30-second maintenance loop for a
change another coordinator committed; a failed re-read keeps the last
family rather than widening anything. Without a configured authority
(`KAVEON_PRODUCT_TRANSACTIONS_ENABLED` off) no grant can exist: catalogs
are visible to administrators only, and the routes answer 503
`ACCESS_STORE_DISABLED`.

### Levels and roles

| Level | Allows |
|---|---|
| `browse` | list the catalog; discover its schemas, tables and columns; `SHOW SCHEMAS`, `SHOW TABLES`, `DESCRIBE`, `SHOW CREATE TABLE`, `DESCRIBE DETAIL`, `SHOW STATS FOR` |
| `query` | `browse`, and SQL statements that read the catalog |
| `manage` | `query`, and catalog DDL inside it: `CREATE|DROP SCHEMA`, `CREATE|DROP TABLE`, `ALTER TABLE … SET LOCATION|CLUSTERED BY|SHAPE`; the platform's schema and table registration routes |

A grant never widens a role. The effective level on a catalog is the
lesser of the grant and the role's ceiling:

| Platform role | Engine role | Ceiling | So a grant of `manage` gives |
|---|---|---|---|
| Viewer | `reader` | `browse` | `browse` |
| Analyst | `analyst` | `manage` | `manage` on the Engine; the platform's SQL Lab still runs only `SELECT`/`WITH`, and its registration routes still require Editor |
| Editor | `analyst` | `manage` | `manage`; the platform's registration routes additionally require `manage` on the catalog |
| Admin | `admin` | every catalog, every level, by role | not through grants |

The Engine cannot tell the platform's Analyst from its Editor (both arrive
as `analyst`); that split is the platform's, enforced on its own routes as
it was before grants existed. `CREATE CATALOG` and `DROP CATALOG` stay
administrators only, and a catalog an administrator creates is granted to
nobody until they grant it. A `reader` browses through the REST metadata
routes — SQL Lab's browser and autocomplete, the CLI's Tab completion,
`GET /v1/catalog…` — because `POST /v1/statement` was never open to
`reader` and still is not; the statement-form `SHOW …` and `DESCRIBE`
are the analyst's and the administrator's, as before.

### Where it is enforced

One evaluator (`CatalogAccess::evaluate`, a `Scope` per identity)
answers every path, and enforcement is by projection: the published
catalog is restricted to the identity's visible set
(`Scope::restrict`), so the binder, the planner, the statistics
statements and the metadata statements all resolve against a manager in
which a hidden catalog does not exist. A reference to an ungranted
catalog therefore fails at bind with the same text an absent catalog gets
— `catalog 'x' not found` — and never confirms that the catalog exists.

| Path | What the identity sees |
|---|---|
| `GET /v1/catalog`, `/v1/catalog/{c}/schema`, `/v1/catalog/{c}/schema/{s}/table` | granted catalogs; an ungranted one is 404 `CATALOG_NOT_FOUND` |
| `GET /v1/catalog/definitions…`, `/v1/catalog/schemas/{id}…`, `/v1/catalog/tables/{id}…` | definitions in granted catalogs, each catalog carrying the identity's `access`; anything else 404. The catalog service credential (no identity) keeps the whole view. |
| `POST /v1/statement` | the session catalog must be visible (400 `CATALOG_NOT_FOUND` otherwise); every scanned table must bind (400 `ANALYSIS_ERROR`, `catalog 'x' not found`); reading needs `query` (403 `ACCESS_DENIED`); DDL inside a catalog needs `manage` (403); `SHOW CATALOGS|SCHEMAS|TABLES`, `DESCRIBE`, `SHOW CREATE TABLE`, `SHOW STATS FOR`, `DESCRIBE DETAIL`, `ANALYZE` resolve against the restricted view |
| The result cache | consulted only after the statement binds against the identity's view, so a cached result is never served for a catalog the identity cannot resolve |
| The platform | `GET /api/v1/lab/engine/sources` keeps only registry sources whose catalog is in the Engine's list for the verified principal, so SQL Lab's picker, its autocomplete, the catalog browser and the chart builder show granted catalogs; schema and table registration require `manage` on the catalog |
| The CLI | `kaveon catalog list`, `schema list`, `table list`, `describe`, Tab completion: the coordinator's answers for the session's identity |

### `KaveonDB`

The transactional application and catalog authority is reserved: it is
never grantable (400 `RESERVED_CATALOG`), and it is hidden from every
role — administrators included — until its read-only `product` and
`catalog` views exist. They do not yet
(`docs/engineering/unified-kaveondb-metadata.md`, "Current state"), so
the evaluator fails closed on the name whatever its case rather than
expose internals. When the projection ships, administrators get the
read-only views and `system`; nobody else sees it.

### Managing grants

`GET /v1/admin/catalog-access` (admin) is the document: the store's
standing (`enabled`, `generation`, `snapshot_id`), the grantable
`catalogs`, the `reserved` authority, the role ceilings and every grant.
`PUT /v1/admin/catalog-access/grants` with
`{"principal", "catalog", "access"[, "revision"]}` creates a grant (no
revision) or changes one at its current revision; `DELETE` on the same
path with `{"principal", "catalog", "revision"}` revokes it. A stale or
missing revision is 409 `REVISION_CONFLICT` naming the current one; an
unregistered catalog 404; the reserved one 400 `RESERVED_CATALOG`; a bad
name 400 `INVALID_GRANT`. `GET /v1/admin/catalog-access/effective/{principal}`
is the effective view: each grant with the level every Engine role would
reach on it, and the catalogs the principal is not granted. A request
body never names the actor; the security layer's identity is the actor
and the ledger records it. Studio's **Settings → Catalog access** page
manages the same document and hands a conflict back as a reload;
`kaveon catalog access …` does the same from the command line.

### Reconciling an open deployment

Before grants existed every signed-in principal saw every catalog.
`POST /v1/admin/catalog-access/import` with `{"source": "open"}` (admin)
proposes that policy as explicit grants: one per principal the audit
ledger has seen submit a statement, per grantable catalog, at the ceiling
of the highest non-admin role it held (`reader` → `browse`, `analyst` →
`manage`), skipping pairs already granted and administrators, whose
access is the role's. The answer is the proposal — `principals_seen`,
`catalogs`, `proposed` — and nothing is recorded; with `"apply": true`
the proposal is recorded in one publication, each grant audited under
the administrator, and `recorded` lists them. Leaving it unrecorded keeps
the default: deny. A ledger that is off (`ledger_enabled: false`)
proposes nothing. Studio's page previews and records it; the CLI is
`kaveon catalog access import --from-open [--apply]`.

### Audit

`catalog_access.grant` and `catalog_access.revoke` lines carry the
administrator (`principal`, `role`), `object_type` `catalog_grant`,
`object_id` `<catalog>/<principal>`, `catalog`, `revision_before` and
`revision_after` (absent for a create and a revoke respectively) and
`details` (`principal`, `access`, `access_before` when changed,
`imported` for the reconciliation, the authority's `generation`).

## Resource groups

A resource group is a named policy. Every statement is admitted through
exactly one group, chosen by the ordered selector list at admission, and
the group's limits are on the statement's record
(`context.resource_group`) and in the refusal when it is refused.

| Key | Default | Bound | What it does |
|---|---|---|---|
| `name` | required | nonempty, trimmed, unique, ≤ 128 characters | The group's identity. A `default` group must exist. |
| `max_memory_bytes` | the whole admission pool | 1 to `KAVEON_MEMORY_ADMISSION_LIMIT_BYTES` | The sum of the group's admitted query pools. A statement whose own pool (`KAVEON_QUERY_MEMORY_LIMIT_BYTES`, or its `query_memory_limit_bytes`) exceeds it can never be admitted through the group and is refused on arrival. |
| `max_concurrent` | required | 1 to 10000 | Statements of the group running at once. |
| `max_queued` | 16 | 0 to 10000 | Statements of the group waiting at once; `KAVEON_MEMORY_ADMISSION_QUEUE` bounds every group together. |
| `max_queue_wait_seconds` | 60 | 1 to 86400 | How long a statement of the group waits before HTTP 429. `KAVEON_MEMORY_ADMISSION_WAIT_SECONDS` and the request's `admission_wait_seconds` can only shorten the wait. |
| `max_local_parallelism` | the node's | ≥ 1 | Caps the statement's `local_parallelism` (aggregator threads per task); the request's own value is lowered to it, and the effective value is on the record's `settings`. |
| `priority` | 1 | 1 to 1000 | The group's weight in the cross-group admission order below. |
| `rate` | none | `{"max_statements": 1 to 100000, "per_seconds": 1 to 2678400, "count": "live"}`; needs `demo.enabled`; `per_seconds` at most the audit ledger's retention | The demo posture's quota: each principal of the group may run `max_statements` live statements in any rolling window of `per_seconds` ([below](#the-demo-quota)). Admins are exempt. |
| `default_settings` | none | the [per-request settings](settings.md#per-request-settings), validated as a request's own | Applied for every key the request left unset, whether the request used the `settings` object or `SET SESSION`. |

The document's top level also carries `demo` (`{"enabled": false}` unless
said otherwise): the coordinator's half of the demo posture. Off, a `rate`
on any group is refused at validation, so a self-hosted install cannot
carry the quota unknowingly; on, every `rate` is enforced and
`GET /v1/quota` reports it. The platform's half — read-only for every
role below Admin — is `KAVEON_DEMO_MODE` on the API
([settings](settings.md#the-demo-posture)).

Selectors are evaluated in order; the first that matches names the group;
a statement no selector matches goes to `default`. A selector may carry
any of `principal` (exact), `principal_prefix`, `role` (`reader`,
`analyst`, `admin`) and `client_tag` (the request's `client_tags` contains
it); every matcher given must hold. A selector with no matcher is the
catch-all and must be last: anything after it is unreachable and refused
at validation.

```json
{
  "groups": [
    {"name": "interactive", "max_concurrent": 8, "max_queued": 32, "max_queue_wait_seconds": 30,
     "max_memory_bytes": 3221225472, "priority": 4},
    {"name": "etl", "max_concurrent": 2, "max_queued": 64, "max_queue_wait_seconds": 1800,
     "max_local_parallelism": 2, "priority": 1, "default_settings": {"result_cache": false}},
    {"name": "default", "max_concurrent": 4}
  ],
  "selectors": [
    {"principal_prefix": "svc-", "client_tag": "etl", "group": "etl"},
    {"role": "analyst", "group": "interactive"},
    {"principal": "alice", "group": "interactive"}
  ]
}
```

### Where the configuration comes from

In order of precedence, the first that exists:

1. The file named by `KAVEON_RESOURCE_GROUPS` (JSON, or TOML by the
   `.toml` extension; the shape above at the top level). A runtime
   replacement is written back to this file.
2. `<state dir>/resource-groups.json`, the coordinator's durable copy of
   the last `PUT /v1/admin/resource-groups`. Delete it to return to the
   configuration file's section.
3. The `[resource_groups]` section of the configuration file
   (`[[resource_groups.groups]]`, `[[resource_groups.selectors]]`).
4. `security.resource_groups` in `KAVEON_SECURITY_JSON`, the list the
   Engine accepted before 2026-09-19, translated: `max_running` is
   `max_concurrent`, `queue_timeout_ms` rounds up to
   `max_queue_wait_seconds`, each principal is an exact selector and `*`
   the catch-all; a `default` group is added when the list names none.
   Configuring both this list and the section is refused at start.
5. The built-in configuration: one `default` group with
   `KAVEON_PRINCIPAL_QUERY_LIMIT` (default 4) running slots, the node's
   queue and wait, no memory share, priority 1. `KAVEON_PRINCIPAL_QUERY_LIMIT`
   is kept as the alias of that group's `max_concurrent`; before
   2026-09-19 it was a limit per principal, now it bounds the `default`
   group as a whole, and a principal that needs its own bound gets its own
   group through a selector.

The state directory is `KAVEON_STATE_DIR` (`node.state_dir`), by default
the directory of the catalog store (`/state/catalog` on the qualification
cluster, `/var/lib/kaveon` on the local Compose stack).

`GET /v1/admin/resource-groups` (admin) reports the configuration in
force, its `source` (`environment`, `runtime`, `config_file`,
`legacy_security`, `builtin`), the `store_path` a replacement is written
to, `demo`, and every group's counters. `PUT /v1/admin/resource-groups` (admin)
replaces every group and selector at once after validation — the
controller's policies first, then the durable copy, then the selectors —
so it takes effect for the next admission decision, waiting statements
included, and survives a restart; an invalid document is HTTP 400
`INVALID_RESOURCE_GROUPS` naming the group or selector and nothing
changes. Studio's Settings → Governance page edits the same document.

### The admission order

A statement needs its group's running slot, its group's memory share and
the node's admission pool. All three are decided by one queue per group in
the coordinator's admission controller, so a statement waits once and its
`admission_wait_ms` is the whole wait.

- **Within a group: strict arrival order.** The head is granted first and
  only when its whole budget fits; nothing behind it is admitted ahead of
  it. One team's statements run in the order they were submitted.
- **Across groups: weighted fair share by admitted bytes.** On every
  release, the groups whose head is *eligible* — a running slot free and
  the head within the group's `max_memory_bytes` — are ranked by
  `admitted_bytes / priority`, lowest first, ties to the older head. The
  group furthest below its weighted share of the pool is next. Two groups
  of priority 2 and 1 under sustained contention hold two thirds and one
  third of the pool.
- **The pool is held for the group that is next.** When that group's head
  does not fit the pool, nothing from any other group is admitted until it
  does. A group under its share is therefore never starved by smaller
  statements of groups over theirs; every lease is released when its
  statement ends and every budget is at most the pool, so the head fits
  eventually. The cost is that a large head of an under-served group can
  make a small statement of an over-served group wait; the wait is bounded
  by the group's `max_queue_wait_seconds`.
- **A group at its own limit is skipped.** A group whose head would exceed
  its `max_concurrent` or `max_memory_bytes` is not entitled to more and is
  passed over, so it cannot stall the others; it is considered again at
  the next release.
- The decision is a function of the admitted bytes at that moment: no
  clocks, no virtual time, O(groups) per release, reproducible from the
  counters.

An arrival that asked not to wait (`admission_wait_seconds: 0`) is
admitted only when it would be the next grant under the same order, else
refused at once.

### Refusals

HTTP 429 in every case; the body carries `resource_group`,
`admission_wait_ms` (how long the statement waited before the refusal)
and, when the binding limit is the group's, `limit` with the bound:

| `code` | When | `limit` |
|---|---|---|
| `RESOURCE_GROUP_REJECTED` | the statement's pool exceeds `max_memory_bytes` (refused on arrival, no record) | `{"max_memory_bytes": …}` |
| `RESOURCE_GROUP_REJECTED` | the group's queue holds `max_queued` (refused on arrival, no record) | `{"max_queued": …}` |
| `RESOURCE_GROUP_REJECTED` | the group's `max_queue_wait_seconds` expired first (the record stays `FAILED` with the wait) | `{"max_queue_wait_seconds": …}` |
| `MEMORY_ADMISSION_REJECTED` | the node's queue is full, no wait was allowed and the arrival is not next, or the node's wait expired first | absent |

```json
{
  "error": "resource group 'etl' admits at most 1073741824 bytes per statement share; 3221225472 bytes requested",
  "code": "RESOURCE_GROUP_REJECTED",
  "resource_group": "etl",
  "limit": {"max_memory_bytes": 1073741824},
  "admission_wait_ms": 0
}
```

### The demo quota

A public demo cluster bounds what one person can make the workers do:
`rate` on a group is a per-principal quota of *live* statements over a
rolling window, enforced on the coordinator so it holds for every client
— Studio through the platform bridge, the CLI, a direct HTTP caller.

- **What counts.** A statement is charged when it reaches the row path:
  after its plan is bound, after the result cache and the statistics have
  declined to answer it, before the first task is scheduled or the first
  batch read. `execution.mode` of a charged statement is `distributed` or
  `coordinator`. A `cache` hit, a `context` answer, a catalog statement,
  a statement refused before it ran (parse error, admission, the quota
  itself) and an admin's statement are not charged. A charged statement
  that then fails or is cancelled stays charged: its rows were read.
- **The decision.** The window holds the charges of the last
  `per_seconds`; a statement arriving when it holds `max_statements` is
  refused with HTTP 429 and nothing is charged. The record of the refused
  statement is `FAILED` with `error_code: RATE_LIMITED` and the message,
  so a paged submission whose record already exists reports the refusal
  through the record. The charge is on the admitted statement's record as
  `context.quota_charge` (`charged_at_ms`, `used`, `remaining`,
  `max_statements`, `per_seconds`).
- **Durability.** The charge travels to the ledger on the statement's
  terminal line as `quota_charged_at_ms`. The in-memory window index —
  per principal, oldest first — is read from those lines at the first
  decision after a start or a `PUT /v1/admin/resource-groups`, over the
  widest window any group carries, and a statement already indexed is not
  counted again. The ledger's retention must therefore cover
  `per_seconds` (validated), and a coordinator without a ledger cannot
  carry a `rate`.

```json
{
  "error": "5 live queries per 6 hours in this demo; the next is allowed at 2026-09-22T14:20:00Z",
  "code": "RATE_LIMITED",
  "message": "5 live queries per 6 hours in this demo; the next is allowed at 2026-09-22T14:20:00Z",
  "retry_after_seconds": 12034,
  "next_allowed_at": "2026-09-22T14:20:00Z",
  "next_allowed_at_ms": 1789827600000,
  "resource_group": "demo",
  "limit": {"max_statements": 5, "per_seconds": 21600, "count": "live"}
}
```

The response carries `Retry-After` with the same seconds. The platform
passes the body through unchanged (`/api/v1/sql/engine`, `/api/v1/lab/query`,
`/api/v1/dlm/reproduce`), and Studio shows it as a notice with the time,
not as a failure.

`GET /v1/quota` (any authenticated role, coordinator only) answers the
caller's own standing: `demo`, `principal`, the `resource_group` the
selectors pick for them with no client tags, `exempt` (an admin in a group
with a `rate`) and `quota` — `max_statements`, `per_seconds`, `count`,
`resource_group`, `used`, `remaining`, `resets_at` (when the oldest charge
leaves the window), `next_allowed_at` (now while `remaining` is above
zero) — or `null` when no quota applies. The platform proxies it as
`GET /api/v1/engine/quota` for Studio's counters (`3 of 5 live queries
left · resets 14:20` on the SQL Lab run button and under the ask box).

```json
{
  "demo": {"enabled": true},
  "groups": [
    {"name": "demo", "max_concurrent": 2, "max_queued": 8, "max_queue_wait_seconds": 30,
     "rate": {"max_statements": 5, "per_seconds": 21600, "count": "live"}},
    {"name": "default", "max_concurrent": 4}
  ],
  "selectors": [
    {"role": "admin", "group": "default"},
    {"group": "demo"}
  ]
}
```

### Counters

`/v1/node` on the coordinator and its entry on `/v1/cluster` carry
`resource_groups`: per group `limit_bytes`, `max_concurrent`,
`max_queued`, `weight`, `running`, `queued` (now), `admitted_bytes`, and
the cumulative `admitted`, `queued_total`, `rejected`, `withdrawn`, with
`wait_ms_p50` and `wait_ms_p95` over the group's last 1024 admissions
(immediate admissions count as zero). `admission` keeps the node-wide
counters. The same counters are on `GET /v1/admin/resource-groups` under
`counters`.

`ANALYZE … WITH (distinct = true)` runs its per-column counts through the
statement path under the submitting principal's group, at most the
group's `max_concurrent` abreast (and never more than four).

## The audit ledger

An append-only record of who did what on the coordinator, kept on the
coordinator's own disk. Writing a line is an in-memory enqueue on the
request's path; a dedicated thread appends each batch to the current
segment and syncs it once, so a clean shutdown (SIGTERM, Ctrl-C: the
listener closes, in-flight requests finish, the ledger drains) loses
nothing and a crash loses at most the batch in flight. The ledger
references the query record by `query_id` and repeats only what an audit
reader needs; the record store keeps the plan, the stages and the
telemetry.

| Setting | Config key | Default | What it does |
|---|---|---|---|
| `KAVEON_AUDIT_DIR` | `audit.dir` | `<state dir>/audit` | The directory of segments and the catalog cursor. |
| `KAVEON_AUDIT_RETENTION_DAYS` | `audit.retention_days` | 90 | A segment is removed once every record in it is older than this; `0` turns the ledger off (`GET /v1/audit` is then 404 `AUDIT_DISABLED`). |
| `KAVEON_AUDIT_SEGMENT_BYTES` | `audit.segment_bytes` | 67108864 (64 MiB) | A segment is closed and a new one started once it reaches this; at least 1 MiB. |

Segments are `audit-<first record ms>-<first seq>.jsonl`, one JSON object
per line, oldest first; a segment is removed by the 30-second cleanup
loop (and at start) once the segment after it began before the retention
cutoff, so the open segment always stays. `seq` increases across
segments and restarts. Workers keep no ledger.

### Record kinds

Every line carries `seq`, `ts_ms` (Unix milliseconds) and `kind`; the
other fields are present when the kind has them.

| `kind` | When | Fields |
|---|---|---|
| `statement.submitted` | a statement reaches admission (after its settings and context are accepted) | `query_id`, `principal`, `role`, `client`, `source`, `client_tags`, `catalog`, `schema`, `statement_sha256` (SHA-256 of the statement text as submitted, after any `SET SESSION` prefix), `statement` (its first 200 characters), `resource_group` |
| `statement.finished` | the statement's response is sent | the submitted fields, `admission_wait_ms`, `elapsed_ms`, `rows` (the whole result, paged or inline), `bytes_scanned` (compressed bytes the scans read across every task), `mode` (`distributed`, `coordinator`, `cache`, `context`), `quota_charged_at_ms` when the statement was charged against its group's [quota](#the-demo-quota) |
| `statement.failed` | the statement failed after admission — an execution error, a wait that expired, or the demo quota | the finished fields, `error_code` (`MEMORY_ADMISSION_REJECTED`, `RESOURCE_GROUP_REJECTED`, `RATE_LIMITED`, a catalog statement's code, `ANALYZE_FAILED`, else `EXECUTION_FAILED`), `error` (first 200 characters) |
| `statement.canceled` | `DELETE /v1/query/{id}`, or the client disconnected | the finished fields, `error_code` (`QUERY_CANCELED`, `CLIENT_DISCONNECTED`) |
| `statement.rejected` | refused on arrival, before any record exists: over the group's share, the group's or the node's queue full, no wait allowed | the submitted fields, `error_code`, `error` |
| `catalog.create`, `catalog.update`, `catalog.delete` | a durable catalog, schema or table definition changed, through `/v1/catalog/*` or a catalog statement | `principal` (the actor), `object_type`, `object_id`, `revision_before` (absent for a create), `revision_after` (absent for a delete), `details` (a cascade's counts) |
| `settings.resource_groups` | `PUT /v1/admin/resource-groups` applied | `principal`, `role`, `details` (`groups_before`, `groups_after`, `selectors_before`, `selectors_after`) |
| `settings.cache_cleared` | `DELETE /v1/cache` | `principal`, `role`, `details` (`cleared_entries`, `cleared_bytes`) |
| `catalog_access.grant`, `catalog_access.revoke` | a catalog grant was created, changed or revoked ([catalog access](#catalog-access)) | `principal`, `role` (the administrator), `catalog`, `object_type` (`catalog_grant`), `object_id` (`<catalog>/<principal>`), `revision_before`, `revision_after`, `details` (`principal`, `access`, `access_before`, `imported`, `generation`) |
| `auth.unauthorized`, `auth.forbidden` | a request was refused with 401 or 403 by the security layer | `route` (`METHOD /path`), `principal` (the one the request named or resolved to, when any), `error_code` |

Catalog lines come from the catalog store's own `audit_events` table:
every mutation publishes a snapshot, and the publish drains the events
past the ledger's cursor (`catalog.cursor` in the audit directory) into
the ledger, so both mutation paths are covered without a second write in
either. A ledger opened for the first time positions the cursor at the
store's newest event: it starts at its own start.

```json
{"seq":1042,"ts_ms":1789805123456,"kind":"statement.finished","principal":"alice","client":"kaveon-cli/0.3.0","catalog":"lake","schema":"sales","statement_sha256":"9f86d0…","statement":"SELECT country, COUNT(*) FROM orders GROUP BY 1","resource_group":"interactive","admission_wait_ms":0,"elapsed_ms":412,"rows":42,"bytes_scanned":183504211,"mode":"distributed","query_id":"5f0c…"}
```

### Reading it

`GET /v1/audit` (admin) answers `{"records": [...], "next_cursor": seq}`
oldest first. Parameters: `since` and `until` (Unix milliseconds,
`YYYY-MM-DD`, or `YYYY-MM-DDTHH:MM:SS[.fff]Z`), `principal` (exact),
`kind` (comma-separated exact kinds or families: `statement`, `catalog`,
`catalog_access`, `settings`, `auth`), `query_id`, `limit` (1 to 1000, default 200) and
`cursor` (a `next_cursor` from the previous page). `format=jsonl` streams
every matching record as `application/x-ndjson`, one object per line, a
page at a time, for export; the same filters apply. A read flushes the
ledger first, so a line enqueued before the call is in the answer. An
unparsable time, an unknown kind or another format is 400
`INVALID_AUDIT_QUERY`; any other role is 403.

The platform proxies both under `/api/v1/engine/audit` and
`/api/v1/engine/admin/resource-groups` for Studio's Settings → Governance
page, which filters, pages and exports the same ledger.
