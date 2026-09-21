# HTTP API Reference

This is a route map, not a standalone API compatibility promise. FastAPI mounts
application routers in `api/main.py`; when the API is running, `/docs` is the
authoritative generated schema for request and response bodies.

## Studio and FastAPI path — current

Studio normally calls `/api/kaveon/*`, a same-origin Next.js proxy. The proxy reads
the server-side session and forwards identity headers plus `X-Proxy-Secret`.
FastAPI accepts that identity only when the secret matches `KAVEON_PROXY_SECRET`.
Provider bearer tokens and the explicitly configured local-development identity are
the other authentication modes. Health and initial-setup routes have different
requirements.

All paths below are relative to the FastAPI origin.

| Area | Prefix or routes | Purpose |
|---|---|---|
| Service | `/`, `/api/health`, `/docs` | Service metadata, health, generated OpenAPI UI |
| Authentication | `/api/connect`, `/api/disconnect`, `/api/auth/provider` | Connection and provider configuration |
| Setup/admin | `/api/v1/setup/*`, `/api/v1/admin/*` | First-run database setup and administration |
| Datasets | `/api/v1/datasets` | Dataset CRUD, columns, favorites, DLM generation/context/freshness |
| Charts | `/api/v1/charts` | Chart CRUD, summaries, favorites |
| Dashboards | `/api/v1/dashboards` | Dashboard CRUD, summaries, favorites, DLM curation |
| Data sources | `/api/v1/data-sources` | Registration, metadata, favorites; connection test is currently a stub |
| SQL | `/api/v1/sql/*` | SQL generation, execution, detached jobs, result cache, filter values |
| SQL Lab | `/api/v1/lab/*` | Discovery, saved queries, execution, CTAS, history, distinct values; on a KaveonDB source, streamed execution with `stream: true` — see [Streamed SQL Lab statements](#streamed-sql-lab-statements-kaveondb) |
| Engine catalog | `/api/v1/engine/catalog/*` | Register catalogs (Admin), schemas and tables (Editor) on KaveonDB by name; tables are verified by a read before they are kept. Routes and roles in [Connectors](connector-capabilities.md#from-the-platform-api) |
| Engine console | `/api/v1/engine/console/*` | Cluster, query history and statistics diagnostics, read-only |
| DLM | `/api/v1/dlm/*` | Routing, ask, chart serving, filter values, coverage, cache and freshness operations |
| Context router | `/api/v1/context/*` | Build, validity, and adaptive-context ask endpoints |
| Chat | `/api/v1/chat`, `/api/v1/chat/history*` | Assistant and conversation persistence |
| User state | `/api/v1/favorites`, `/api/v1/theme`, `/api/v1/user/recents`, `/api/v1/users/me` | Per-user state |

### Important behavior

- Each platform query targets one selected SQL source. Cross-source federation is
  not implemented.
- `POST /api/v1/data-sources/{id}/test` currently returns a not-implemented message;
  the setup probe endpoints perform real connectivity checks.
- Detached SQL jobs and cached results live in process memory. Restarting the API
  loses that state, and multiple replicas need external coordination that is not
  currently implemented.
- Authorization differs by route. Do not infer write permission merely from an
  authenticated session; inspect the generated OpenAPI schema and router dependency.
- **Product records and the KaveonDB catalog.** PostgreSQL is the read authority
  for datasets, charts, dashboards and the other product families unless
  `KAVEONDB_READ_AUTHORITY_FAMILIES` moves a family. Every product write also
  appends one event to the PostgreSQL outbox, and the API's replay worker
  (`KAVEON_PRODUCT_REPLAY_ENABLED=true`, batch and interval settings alongside
  it) applies those events in source order to the coordinator's product
  catalog (`POST /v1/transaction/sql` under the bridge token); `GET /api/health`
  reports it under `checks.product_replay`. DLM generation binds its definition
  to the dataset's KaveonDB revision, so `POST /api/v1/datasets/{id}/dlm/generate`
  first replays the dataset's own pending events in order and then reads the
  record: a dataset created a moment earlier publishes without waiting for the
  worker's next pass, and a backlog that cannot replay fails the generate
  request with the replay error rather than an incomplete artifact. DLM
  serving (`/api/v1/dlm/*`) reads the legacy PostgreSQL DLM tables until
  `KAVEON_POSTGRESQL_RETIREMENT_MODE` requests the PostgreSQL-free runtime;
  only then does an authenticated caller serve from compiled KaveonDB context.
- **Streaming pages (Engine).** A statement submitted with
  `result_delivery: "paged"` registers its pages the moment its record becomes
  `RUNNING`, and its record (`GET /v1/query/{id}`, the `GET /v1/query` list)
  carries `next_uri: "/v1/query/{id}/results/0"` from then on, kept once it
  finishes; inline statements' records have no `next_uri`. Pages are written
  every 1,000 rows or 4 MiB and are readable as soon as their file is complete,
  while execution continues. `GET /v1/query/{id}/results/{n}` answers `200`
  with `{"id", "data": [...rows], "next_uri", "row_count", "complete"}` when
  page `n` is written — `row_count` is the rows written so far (the total once
  `complete` is true) and `next_uri` is non-null whenever page `n + 1` exists or
  the statement is still running, so a client follows it until it is `null`;
  `202 Accepted` with `Retry-After: 1` and `{"id", "row_count", "complete":
  false}` when `n` is exactly the next page not yet written; `404` for a page
  past the end, an unknown or expired result, or another owner's; `410 Gone`
  when the statement failed or was cancelled after its pages were registered.
  The final `POST /v1/statement` response is unchanged: it carries `next_uri`
  for page 0 once the result is complete. Catalog statements and `ANALYZE`
  answer inline regardless of the requested delivery.
  On a distributed statement the root tasks — the tasks with no exchange
  output, whose batches are the statement's rows — stream those rows to the
  pages as they produce them: the coordinator submits a paged statement's
  root tasks with `stream_result: true` in the task request, the worker
  answers `200` as soon as the result schema is known with an Arrow IPC
  stream that carries each batch when it is produced, and the coordinator
  decodes the body as it arrives and pushes the rows into the pages; rows
  from several root tasks running at once interleave. Which rows appear
  early is a property of the plan: a scan, filter, projection, `LIMIT`
  without `ORDER BY` and a join's probe output emit from their first batch,
  while a root whose operator holds everything until the end — `ORDER BY`,
  `GROUP BY`, `DISTINCT`, a `TopN` — emits only when the task finishes,
  and its pages still land at the end. A streamed task's metrics are not in
  its response headers; the coordinator reads them from the worker's
  `/v1/task/{query_id}/{stage_id}/{partition}/{attempt}/metrics` once the
  body ends (the worker records the outcome before it ends the body). A
  root task that fails after any of its rows reached the pages is not
  retried — a retry would deliver those rows twice — and the statement
  fails with the worker's message followed by
  `(rows already delivered; the statement is not retried)`; a root task
  that fails before any row is retried as any task. Inline delivery keeps
  the collected path: each root task answers once with its whole result.

### DLM answers and their evidence

`POST /api/v1/dlm/ask` takes `{question, limit?, choices?, frame?}` and answers
`ok: true` with the routed dataset, the answer's shape (`columns`, `rows` when
the DLM has them, `chartType`, `xAxis`, `yAxis`, `title`, `note`, `frame`),
its lane (`route: "context" | "cache" | "live"`, `from_context`), or `ok:
false` with a `reason` (`clarify` with a `clarification` to answer,
`out_of_scope`, `no_dataset`, `dataset_not_found`, `query_failed` with a
`message`). Every `ok: true` answer — over a warehouse dataset or an Engine
table — carries an **`evidence`** object:

| Field | Meaning |
|---|---|
| `sql` | The statement that ran, or that would compute a context answer — in the source's dialect |
| `dataset` | `{id, name}` |
| `source` | `{kind: "engine", table_id, catalog, schema, table}` for a dataset bound to an Engine table (or over a native catalog), `{kind: "warehouse", database, schema, table}` otherwise |
| `source_version` | What the answer reflects. Engine: the record's `execution.source_version` for a context answer, the table's `GET /v1/catalog/tables/{id}/version` for a read (`{identity_sha256, kind: delta_version \| iceberg_snapshot \| listing \| file, …}`). Warehouse: `{kind: "postgresql_change_counter", table, row_count, mods_since_analyze, last_analyze, observed_at}` — the snapshot the artifact was compiled against for a context answer, the counter as read now for a live one; `{kind: "unavailable"}` on a source without the counter |
| `lane` | `context` (answered without reading the rows: the Engine's cube or statistics, or the DLM's precomputed cells), `cache` (the Engine's result cache), `live` (the rows were read) |
| `execution` | The Engine's query-record `execution` object verbatim (`mode`, `detail`, `source_version`, `current_source_version`, `approximate[]`); `null` on the warehouse |
| `settings` | The per-statement settings the DLM chose on the Engine: `result_cache` per the dataset's freshness policy, `use_statistics: true`, `approximate: true` only for a metric the context spec marks approximate; `null` on the warehouse |
| `principal` | The identity the Engine statement ran as (the caller when their role can submit statements, else the platform service principal) |
| `query_id`, `elapsed_ms`, `rows` | The Engine's query id and elapsed, and the rows returned. A warehouse `live` answer is run by the client (through `/api/v1/sql/execute` or `/api/v1/sql/engine`), which fills `elapsed_ms` and `rows` itself; every other lane is complete as returned |
| `reproduce` | `{sql, database, schema, engine, settings}` — the same statement as a live read: on the Engine with `{use_statistics: false, result_cache: false}` so neither the cube, the statistics nor a cached result may answer; `settings: null` on the warehouse |

An Engine-backed answer is executed by the DLM itself (`executed: true`,
`engine: true`, the rows in the response); the lane and the estimate label
(`approx`) come from the Engine's `execution.mode` and `execution.approximate`,
never from the DLM's own scoring. `POST /api/v1/dlm/reproduce` (Analyst or
above) takes `{dataset_id, sql}` — an answer's `reproduce` block — and runs it
under the same guards as SQL Lab (one read-only statement, no platform tables,
the caller's rate limit), answering `{ok, columns, rows, evidence, duration_ms}`
with the evidence of that run, so a client can set the live number beside the
context one. `GET /api/v1/datasets/{id}/freshness` reports `signal:
"engine_source_version"` with `source_version`, `current_source_version` and
`observed_at_ms` for an Engine-backed dataset (`postgresql_change_counter`
otherwise).

A dataset is bound to an Engine table with `source: {kind: "engine", table_id}`
on `POST`/`PUT`/`PATCH /api/v1/datasets`: the catalog, schema and table names,
the column list and — when the table declares a shape — the dimensions,
measures and date column are read from `GET /v1/catalog/tables/{id}` and
never typed by hand; columns and metrics the caller (or the stored dataset)
already has are kept. The binding is returned as `source` on the dataset.
The DLM's context spec (`GET`/`PUT /api/v1/datasets/{id}/dlm/context`) gains
`approximate` per metric and a dataset-level `freshness_policy` (`cached` |
`live`) for these datasets. See [Over Engine tables](../guides/nl-to-sql.md#over-engine-tables).

### Streamed SQL Lab statements (KaveonDB)

SQL Lab on a KaveonDB source submits every statement this way, so the grid
shows rows while the statement runs. Every route is bound to the caller
(Analyst or above) and every read goes to the coordinator under that
caller's identity — the same actor the submit was stamped with — so the
coordinator's owner scoping is what binds a record to its reader. The API
holds no state of its own: a second replica serves the same routes.

| Method | Path | Behavior |
|---|---|---|
| `POST` | `/api/v1/lab/query` with `"stream": true` (and `engineSourceId`) | Submits with `result_delivery: "paged"` on a background thread and answers as soon as the coordinator has a record for it: `{"success", "queryId", "tag", "state", "nextUri"}`. A statement the coordinator refuses before it has a record — a parse error, exhausted capacity — is the refusal itself (400 with the Engine's message, 429 with `Retry-After`). Without `stream` the route is unchanged: it waits for the statement and answers with its rows |
| `GET` | `/api/v1/lab/query/{id}` | `{"success", "query"}`: the record view — `id`, `state`, `elapsed_ms` (final once finished), `columns` as names once planned, `stages[]` and `scans[]` counters while it runs, `workers` (distinct nodes its tasks ran on), `execution` placement (`pending` until it finished), `error`, `next_uri`, `timings`, `cached_from`, `submitted_at_ms`, `completed_at_ms`. The record's preview rows, plan and SQL are not served here. 404 for an unknown or another owner's statement |
| `GET` | `/api/v1/lab/query/{id}/results/{n}` | Page `n`, with the coordinator's status and body passed through: `200` `{"id", "data", "next_uri", "row_count", "complete"}`, `202` with `Retry-After: 1` and `{"id", "row_count", "complete": false}` while page `n` is not yet written, `404` past the end or for another owner's, `410 Gone` once the statement failed or was cancelled |
| `DELETE` | `/api/v1/lab/query/{id}` | Cancels; `204`, or `404` when the coordinator no longer knows the statement or it is another owner's. Cancelling releases the statement's pages |

The background thread that holds the statement's `POST /v1/statement`
writes the query-history row when it returns: `status` `success`, `error`
or `cancelled` from the record's final state, `duration_ms` from its
`elapsed_ms`, `row_count` from the writer's total (a page read once the
statement finished), the Engine's error message, and the record as
`engine_details`.

Studio reads the record every 250 ms while the statement runs and follows
the pages from `results/0`, appending each page to the grid as it lands and
honouring `Retry-After` on a 202; once the writer is ahead of the reader it
requests up to four consecutive pages together. It holds at most the
selected row limit and stops reading pages there while the statement runs
to completion, so the summary still carries the true row count. Which rows
appear early is the plan's property described above: a scan, filter or
join from its first batch, an `ORDER BY`, `GROUP BY` or `DISTINCT` root at
the end.

## Engine HTTP path — alpha

The Rust server exposes these routes:

| Method | Path | Current behavior |
|---|---|---|
| `POST` | `/v1/statement` | Parse, bind, plan, execute and retain a query result; inline (up to 16 MiB) or `result_delivery: "paged"` with `next_uri` pages; accepts per-request `settings` and leading `SET SESSION` statements; a parse error is a 400 `SYNTAX_ERROR` whose body carries `position: {line, column}` (one-based) when the parser names the failing token, absent for an error at end of input |
| `POST` | `/v1/statement` | Parse, bind, plan, execute and retain a query result; inline (up to 16 MiB) or `result_delivery: "paged"` with `next_uri` pages; accepts per-request `settings` and leading `SET SESSION` statements. Also runs [catalog statements](#catalog-statements) (`CREATE`/`DROP`/`ALTER` on the durable catalog, `SHOW`, `DESCRIBE`, `CALL system.register_table`) |
| `GET` | `/v1/query` | Return up to 100 newest process-local query records, queued and running ones included |
| `GET` | `/v1/query/{query_id}` | Return retained lifecycle, context, structured logical plan, result, and scan telemetry (`scans[]`: files considered, opened, pruned by partition and `files_skipped` from recorded bounds; row groups considered, read, pruned and `row_groups_pruned_by_bloom` with `bloom_filters_read` and `bloom_filter_bytes_read`; rows and compressed bytes selected and read; row-filter rows examined and admitted; lane spread); while the state is `RUNNING`, `stages` (`completed_tasks`, `tasks`) and `scans` are updated as each distributed task completes, and `scan_metrics_complete` stays `false` until the statement finishes |
| `DELETE` | `/v1/query/{query_id}` | Cancel the query: a queued statement leaves the admission queue at once; a running one propagates cancellation to active worker tasks |
| `GET` | `/v1/cluster` | Coordinator and discovered-worker state, with each node's memory admission counters (`admission`) as last heartbeated |
| `GET` | `/v1/node` | Current node information, with the result cache counters on a coordinator and the node's memory admission counters (`admission`) |
| `DELETE` | `/v1/cache` | Drop every cached result (admin role) |
| `GET` | `/v1/audit` | The audit ledger (admin role): statements submitted, finished, failed, cancelled, rejected; catalog mutations; settings changes; authentication failures — filtered by `since`, `until`, `principal`, `kind`, `query_id`, paged by `limit` and `cursor`, exported whole with `format=jsonl` — see [Audit ledger](#audit-ledger) |
| `GET`, `PUT` | `/v1/admin/resource-groups` | The resource groups and selectors in force with their source and counters; replace them all at once, validated, effective for the next admission and durable across restarts (admin role) — see [Resource groups](#resource-groups) |
| `POST` | `/v1/node/heartbeat` | Register a worker heartbeat on a coordinator |
| `GET` | `/v1/catalog` | List catalogs |
| `GET`, `POST` | `/v1/catalog/definitions` | List or create durable catalog definitions |
| `GET`, `PUT`, `DELETE` | `/v1/catalog/definitions/{catalog_id}` | Read, revision-replace, or delete a durable catalog definition |
| `GET`, `POST` | `/v1/catalog/definitions/{catalog_id}/schemas` | List or create durable schema definitions |
| `GET`, `PUT`, `DELETE` | `/v1/catalog/schemas/{schema_id}` | Read, revision-replace, or delete a durable schema definition |
| `GET`, `POST` | `/v1/catalog/schemas/{schema_id}/tables` | List or create durable table definitions |
| `GET`, `PUT`, `DELETE` | `/v1/catalog/tables/{table_id}` | Read, revision-replace, or delete a durable table definition |
| `GET` | `/v1/catalog/tables/{table_id}/statistics`, `…/version` | The table's statistics on record with the source version observed now, and the current source version alone (see [Statistics endpoints](#statistics-endpoints)) |
| `GET` | `/v1/catalog/{catalog}/schema` | List schemas |
| `GET` | `/v1/catalog/{catalog}/schema/{schema}/table` | List tables |
| `GET` | `/v1/query/{query_id}/results/{page}` | One page of a paged result, served while the statement still runs (owner-scoped, immutable once written, 15 min TTL): `200` with the rows, `202` + `Retry-After: 1` for the next page not yet flushed, `404` past the end or unknown, `410` when the statement failed or was cancelled — see "Streaming pages" under [Important behavior](#important-behavior) |
| `GET` | `/v1/whoami` | The identity the security layer attached to the request: `principal`, `display` (null unless a validated sign-in supplied one), `role` (`reader`, `analyst`, `admin`) and `auth` (`static`, `bridge`, `entra`, `development`, `internal`). The client shows it in its session header; older coordinators answer 404 and the client hides the line |
| `GET` | `/v1/capabilities`, `/v1/statistics`, `/v1/auth/config` | What the coordinator supports (native `ANALYZE`, transactions), the tables with statistics on record and whether each is current, and the Entra sign-in configuration for the UI |
| `POST` | `/v1/transaction`, `/v1/transaction/sql`, `/v1/transaction/{id}/stage`, `…/commit`, `…/rollback`, `…/recovery`; `GET` `/v1/transaction/metrics`, `/v1/products/{kind}`, `/v1/product/{kind}/{id}` | The bounded product-record transaction protocol and typed product reads; see the [SQL compatibility reference](engine-sql-compatibility.md#transaction-api-boundary) |
| `POST`, `GET` | `/v1/task`, `/v1/exchange`, `/v1/internal/exchange/*`, `/v1/internal/query/{query_id}/finish`, `/v1/internal/catalog/snapshot` | Worker task submission, exchange partition upload/download, query finish and cancellation, catalog replica; exchange-token authenticated, not client routes. A task request with `stream_result: true` (a root task of a paged statement) is answered with its Arrow IPC stream as the fragment runs, marked `x-kaveon-task-streamed: 1`, its metrics omitted from the headers; a second submission of a streamed task is refused with `409 TASK_RESULT_NOT_RETAINED` |
| `GET` | `/v1/task/{query_id}/{stage_id}/{partition}/{attempt}/metrics` | A task's outcome on its worker, exchange-token authenticated: `200` `{"elapsed_us", "scan", "execution"}` once it finished (`scan` and `execution` are the task's scan and execution metrics, `null` when it carries none), `202` `{"state": "RUNNING"}` while it runs, `500` `{"error"}` when it failed, `404` for a task the worker does not know or has already forgotten with its finished query |
| `GET` | `/health`, `/ready`, `/ui` | Liveness, catalog readiness, and operational UI |

Catalog mutations through `/v1/catalog/*` require the configured catalog-admin bearer token, an actor header, and optimistic `If-Match` revisions for replacement; the same definitions are also created and dropped by [catalog statements](#catalog-statements) on `/v1/statement` under the submitting principal's role. Internal task/exchange routes use a separate shared bearer token. Statement clients authenticate with a principal token from `KAVEON_SECURITY_JSON` (roles `reader`, `analyst`, `admin`), an Entra bearer token, or the API bridge token with delegated `x-kaveon-principal`/`x-kaveon-role` headers; the server serves native TLS, admits every statement through its resource group (concurrency, memory share, queue) and the memory admission pool, and scopes query records and paged results to their owner (`docs/engineering/engine-security-integration.md`). This is a credential boundary, not production identity federation: rotation without restart, tenant isolation and row/column policies remain gates, so keep the Engine on a private network during alpha.

The statement JSON body requires `query`. Clients may also provide `source`,
`client`, `time_zone`, `client_tags`, `result_delivery` and `settings`. `source`,
`client` and `client_tags` identify the submitting application and session; they
are not trusted user identity. The record's `principal` comes from the
authenticated identity; `client_address` is not recorded.

### Catalog statements

`POST /v1/statement` accepts the Trino-shaped catalog DDL below. A catalog
statement lowers onto the same durable definitions `/v1/catalog/*` manages —
the same identifiers (`catalog:<name>`, `schema:<catalog>:<name>`,
`table:<catalog>:<schema>:<name>`), revisions, lifecycle and audit trail —
and republishes the planning snapshot, so the table answers queries on the
next statement. It leaves a query record like any statement (`FINISHED`
with the result, or `FAILED` with the reason) and returns a one-row result:
`(catalog|schema|table, result)` with `created`, `exists`, `dropped`,
`absent`, `relocated`, `clustered`, `shaped`, `unshaped` or `unchanged`.
One statement per request; the session `catalog` and `schema` of the
request resolve unqualified names.

`SET SHAPE` declares the shape the table's cube is built over (see
[Declared shape and the cube](../engine/storage-and-catalogs.md#declared-shape-and-the-cube)):
`dimensions` are columns each with an optional cardinality cap
(`'region'`, `'status:50'`; default 10,000), `measures` are columns under
`sum`, `count`, `min`, `max` and `count_distinct` (`'total:sum,count'`),
`time` is one date or microsecond-timestamp column at `day` or `month`
grain with an optional cap (`'order_date:day'`, `'at:month:60'`). A
declaration is refused (`CATALOG_INVALID`) for a column the table does
not have, a type the role does not accept (a floating-point dimension, a
sum over text, a date column at month grain), a partition column, or a
planned cube above `KAVEON_CUBE_MAX_CELLS`. `DROP SHAPE` (or `SET SHAPE
()`) clears it; either change drops the cube on record. `CREATE TABLE`
accepts the same three options. The table document
(`GET /v1/catalog/tables/{id}`) carries the shape as `shape: {dimensions:
[{name, cap}], measures: [{column, aggregates: [...]}], time: {column,
grain, cap}}`, absent when none is declared.

```sql
CREATE CATALOG [IF NOT EXISTS] name WITH (storage = 'adls', account = '…', container = '…' [, root = '…'] [, credential = 'workload-identity:<reference>'])
CREATE CATALOG [IF NOT EXISTS] name WITH (storage = 'local', base_path = '<absolute path on the coordinator>')
CREATE CATALOG [IF NOT EXISTS] name WITH (storage = 's3', bucket = '…', region = '…' [, prefix = '…'])
DROP CATALOG [IF EXISTS] name [CASCADE | RESTRICT]
CREATE SCHEMA [IF NOT EXISTS] [catalog.]schema
DROP SCHEMA [IF EXISTS] [catalog.]schema [CASCADE | RESTRICT]
CREATE TABLE [IF NOT EXISTS] [catalog.][schema.]table [(column type [NOT NULL], …)]
    WITH (location = '<path within the catalog root>', format = 'parquet' | 'delta' | 'iceberg' [, access = 'shortcut' | 'optimized'] [, partitioned_by = ARRAY['key', …]] [, clustered_by = ARRAY['column', …]] [, bloom = ARRAY['column', …]] [, dimensions = ARRAY['column[:cap]', …]] [, measures = ARRAY['column:agg[,agg…]', …]] [, time = 'column:day|month[:cap]'])
CALL [catalog.]system.register_table(schema_name => '…', table_name => '…', table_location => '…' [, format => 'delta'])
CALL [catalog.]system.unregister_table(schema_name => '…', table_name => '…')
ALTER TABLE [IF EXISTS] [catalog.][schema.]table SET LOCATION '<path>'
ALTER TABLE [IF EXISTS] [catalog.][schema.]table SET CLUSTERED BY (column, …)
ALTER TABLE [IF EXISTS] [catalog.][schema.]table SET SHAPE (dimensions = ARRAY['column[:cap]', …], measures = ARRAY['column:agg[,agg…]', …], time = 'column:day|month[:cap]')
ALTER TABLE [IF EXISTS] [catalog.][schema.]table DROP SHAPE
OPTIMIZE [TABLE] [catalog.][schema.]table [WITH (row_group_rows = n, row_group_bytes = n, file_bytes = n)] [WHERE predicate]
DROP TABLE [IF EXISTS] [catalog.][schema.]table
SHOW CATALOGS [LIKE 'pattern']
SHOW SCHEMAS [FROM | IN catalog] [LIKE 'pattern']
SHOW TABLES [FROM | IN [catalog.]schema] [LIKE 'pattern']
SHOW CREATE TABLE [catalog.][schema.]table
DESCRIBE [TABLE] [catalog.][schema.]table
SHOW COLUMNS FROM [catalog.][schema.]table
```

- **Roles.** `CREATE CATALOG` and `DROP CATALOG` require `admin`; schema and
  table statements require `analyst` or `admin`; `SHOW` and `DESCRIBE` any
  statement-capable role (`reader` cannot submit statements at all). The
  submitting principal is the audit actor. The `/v1/catalog/*` service
  credential is not involved.
- **Columns.** `CREATE TABLE` without a column list reads the columns from
  the table itself — the Delta log, the Iceberg metadata pointer or the
  Parquet footers (the first file of a directory) — with a metadata-only
  read, and stores them. A declared column list is stored as declared once
  every declared column is found in the source by name. Column types accept
  the SQL spellings (`bigint`, `integer`, `smallint`, `tinyint`, `boolean`,
  `double`, `real`, `varchar`, `varbinary`, `date`, `timestamp`,
  `decimal(p, s)`) and the Arrow names (`Int64`, `Utf8`, …), which is what
  `SHOW CREATE TABLE` and `DESCRIBE` present, so their output registers the
  same table again.
- **Partition columns.** A Parquet directory in the Hive layout
  (`dt=2026-09-01/region=eu/part-0.parquet`) has its `key=value` keys as
  columns after the file columns, typed by inference from the values
  (`bigint`, `date`, else `varchar`; `__HIVE_DEFAULT_PARTITION__` is NULL).
  `partitioned_by = ARRAY['dt', 'region']` declares them — each must be in
  the column list when one is given, which is how a key's type is declared
  (`dt VARCHAR` reads a date key as text) — and must name exactly the path's
  keys in their order. A directory whose paths carry keys is recorded as
  partitioned either way, and `SHOW CREATE TABLE` renders the option. The
  option is refused for Delta and Iceberg. Every file must lie under the
  same keys at the same depth, and a key must not also be a column inside
  the files; both fail the probe naming the file. See
  [Storage and catalogs](../engine/storage-and-catalogs.md#partition-columns)
  for the read and pruning rule.
- **Layout.** `clustered_by = ARRAY['a', 'b']` records the columns rows are
  sorted by within every file the Engine writes for the table, and `bloom =
  ARRAY['c']` the columns that carry a Bloom filter per row group beyond
  the clustering columns (which always do). Both name columns of the table
  as resolved — with a column list, declared columns; without one, the
  source's — and an unknown column is refused by name. `ALTER TABLE … SET
  CLUSTERED BY (…)` publishes the next revision with another clustering
  (an empty list clears it; the `bloom` list stays); its result is
  `clustered`, or `unchanged`. `SHOW CREATE TABLE` renders both options.
  The layout describes what `OPTIMIZE` writes, not what the files hold: a
  freshly registered table is not clustered until it is rewritten. See
  [Layout](../engine/storage-and-catalogs.md#layout).
- **`OPTIMIZE`** (admin role) rewrites a Parquet table's files in its
  layout — sorted by the clustering columns, 128 MiB / 1 M-row row groups
  with a page index and Bloom filters, files of up to 1 GiB — and answers
  one row: `table, files_replaced, files_written, rows, row_groups,
  bytes_before, bytes_after, clustered_by, recovered`. `WITH` overrides the
  sizes (positive integers; `row_group_rows`, `row_group_bytes`,
  `file_bytes`). `WHERE` selects the files to rewrite by their partition
  values and footer statistics — a predicate over columns compared with
  literals, `IN`, `IS [NOT] NULL`, `[NOT] LIKE`, `AND`/`OR`/`NOT` — and a
  selected file is rewritten whole; a predicate the footers cannot
  evaluate is 400 `OPTIMIZE_INVALID`. A table without clustering columns is
  compacted without a sort. The sort runs under the statement's memory
  admission and spills like any operator. Publication is crash-safe (a
  manifest, new files into place, old files deleted, manifest deleted; an
  interrupted rewrite is finished or rolled back by the next `OPTIMIZE`,
  reported as `recovered`); a partitioned directory is rewritten one
  partition directory at a time. Delta and Iceberg tables are refused with
  400 `OPTIMIZE_UNSUPPORTED` (the Engine has no Delta commit writer, and
  their files are named by a log); a second `OPTIMIZE` of a table being
  rewritten is 409 `OPTIMIZE_IN_PROGRESS`; a table clustered by a partition
  column is 400 `OPTIMIZE_INVALID`.
- **Verification.** A table is created as a `Draft` (revision 1), its
  location is probed, and only a readable location becomes `Active`
  (revision 2). A failed probe deletes the draft and the statement fails with
  HTTP 400, code `TABLE_NOT_READABLE`, and the storage error — a missing
  object, a Parquet file registered as Delta, a declared column the source
  does not have. Nothing is left half-registered. `ALTER TABLE … SET
  LOCATION` probes the new location the same way, requires every stored
  column to be present there, and publishes the next revision.
- **`CREATE CATALOG`.** `storage = 'local'` requires an absolute directory
  that exists on the coordinator. ADLS and S3 catalogs store the account or
  bucket and an optional credential *reference* (`kind:reference`, kinds
  `managed-identity`, `workload-identity`, `environment`, `secret-store`);
  never a secret. Their tables are probed when they are created.
- **`DROP … RESTRICT`** (the default) refuses a schema or catalog that still
  holds objects with HTTP 409; `CASCADE` removes the children with it.
- **Errors.** HTTP 400 `SYNTAX_ERROR` for a malformed statement, naming the
  missing option; 400 `CATALOG_NOT_FOUND` / `SCHEMA_NOT_FOUND` /
  `TABLE_NOT_FOUND`; 409 `CATALOG_CONFLICT` for an object that already exists,
  is not empty, or changed revision while the statement ran; 403 `FORBIDDEN`
  for a role that may not make the change; 400 `CATALOG_INVALID` for an
  invalid definition; 500 `CATALOG_UNAVAILABLE` when the store or the snapshot
  publication fails. A failure after the statement was admitted leaves a
  `FAILED` record and its body carries the query `id`; a syntax error is
  refused before a record exists.

A catalog statement does not need an existing session context: `CREATE
CATALOG` on an empty coordinator, or `CREATE SCHEMA` in a catalog with no
schema, runs with whatever `catalog`/`schema` the request names.

### Statistics statements

`ANALYZE` computes a table's statistics and stores them in the durable
catalog beside the table definition; two statements read them (the
engineer's reference for the whole path is
[The learning engine](../engine/learning-engine.md)). All take
the `ANALYZE` name form — `[catalog.][schema.]table`, plain or
double-quoted identifier parts, unqualified names resolved by the session
`catalog` and `schema` — answer inline regardless of `result_delivery`, and
leave a query record like any statement.

```sql
ANALYZE [catalog.][schema.]table
ANALYZE [catalog.][schema.]table WITH (sketches = true)
ANALYZE [catalog.][schema.]table WITH (distinct = true)
ANALYZE [catalog.][schema.]table WITH (columns = ARRAY['a', 'b'])
ANALYZE [catalog.][schema.]table WITH (cube = true)
SHOW STATS FOR [catalog.][schema.]table
DESCRIBE DETAIL [catalog.][schema.]table
```

`ANALYZE` has three forms, by how much of the table it reads, and a
fourth that builds the cube. They combine: `sketches = true` with
`distinct` or `columns` reads every column once and then counts exactly;
`cube = true` with any of them builds the cube first.

| Form | Reads | Produces |
|---|---|---|
| `ANALYZE t` — metadata only, the default | Parquet footers, the Delta log, the Iceberg metadata pointer and manifests; never a data page | The table facts (rows, bytes, files, row groups, last modified), each column's null count and bounds as the writer recorded them (`bounds_exact` says whether a bound may be truncated), the compressed bytes per column, and every file's rows, bytes and bounds for file skipping; the document's `depth` is `metadata` |
| `ANALYZE t WITH (sketches = true)` — the sketches | Every sketchable column once, on the coordinator, files in parallel, batches reserved through the statement's memory admission; a source that changes under the read is refused rather than mixed | Everything the metadata form produces, plus a HyperLogLog distinct-count sketch per column (p = 12, 1.6 % standard error), a KLL quantile sketch per numeric or temporal column (k = 200), exact bounds and exact null counts; the document's `depth` is `full`. These are what the planner estimates selectivity from and what `APPROX_*` aggregates answer from without a scan |
| `ANALYZE t WITH (distinct = true)` / `WITH (columns = ARRAY['a', 'b'])` — exact distinct counts | One `SELECT COUNT(DISTINCT "column") FROM t` per selected column through the cluster (details below) | The exact distinct count of every column, or of the columns named, kept beside whatever the record already holds at this source version; a count answers before a sketch's estimate wherever both exist |
| `ANALYZE t WITH (cube = true)` — the cube | The shape's columns of every file once, on the coordinator, files in parallel, under the statement's memory admission; refused with `SHAPE_UNDECLARED` when the table declares no shape | The cube (see [Declared shape and the cube](../engine/storage-and-catalogs.md#declared-shape-and-the-cube)): cells at the grand total, every axis and every low-cardinality pair, holding the row count, the additive measures exactly and the distinct counts as sketches; stored beside the statistics with one partial per file, versioned by the same source version. The result's `cube_cells` is the cell count. A cube above `KAVEON_CUBE_MAX_CELLS` cells (or the byte bound derived from it) fails the statement; an axis over its cap is excluded, not failed |

- **`ANALYZE`** (admin only) reads the source's metadata — Parquet
  footers, the Delta log, the Iceberg metadata pointer and manifests;
  never a data page — into one statistics object (below): the table
  facts, per-column bounds with their exactness and null counts, and
  per-file bounds for file skipping. The object is versioned by the
  **source version** it was computed from — the Delta version, the
  Iceberg snapshot, the digest of a directory listing, a file's identity
  — and stored under the table's durable id (`table_statistics` in the
  SQLite catalog, deleted with the table). A source that moves between
  the build and the store is 409 `SOURCE_CHANGED`. Its result is one row,
  `table (VARCHAR), row_count (BIGINT), distinct_columns (BIGINT),
  cube_cells (BIGINT), full_read (VARCHAR)` — `distinct_columns` is how
  many columns this statement counted distinct values for, `0` for the
  metadata-only form; `cube_cells` the cells of the cube it built, null
  when it built none; `full_read` where the columns were read
  (`workers`, `coordinator`), null when they were not.
- **`ANALYZE … WITH (sketches = true)`** reads every sketchable column
  once — on the workers, as one `SELECT COUNT(*), COLUMN_STATISTICS(…) …`
  statement whose scan partitions across them by row group, when the
  cluster can run a distributed statement, else on the coordinator
  (files in parallel, batches reserved through the statement's memory
  admission); the result's `full_read` column (`workers` /
  `coordinator`) and the record's `execution` say which — for a
  HyperLogLog distinct-count sketch per column, a KLL quantile sketch
  per numeric or temporal column and exact bounds and null counts;
  `depth` becomes `full`. `WITH (cube = true)` follows the same rule:
  one `GROUP BY` statement per grouping on the workers (see
  [`ANALYZE`](../engine/learning-engine.md#analyze)), one coordinator
  scan otherwise. A source that
  changes under the read is refused rather than mixed. The sketches are
  what the planner estimates selectivity from and what
  `APPROX_COUNT_DISTINCT` and `APPROX_PERCENTILE` answer from without a
  scan (see [Approximate aggregates](engine-sql-compatibility.md#approximate-aggregates));
  a later addition of files folds new sketches in (see automatic
  refresh).
- **`ANALYZE … WITH (distinct = true | columns = ARRAY['a', 'b'])`** adds
  exact distinct counts, which take a scan. `distinct = true` counts every
  column; `columns = ARRAY[…]` (Trino's spelling; single-quoted names,
  case-sensitive as the source spells them, `''` for a quote) counts the
  columns listed; either may be combined with `sketches = true`. The keys
  and the values are case-insensitive; `distinct = false` is the plain
  form. `distinct` and `columns` together, an unknown key, or a malformed
  list is 400 `SYNTAX_ERROR`; a column the table does not have is 400
  `ANALYSIS_ERROR` naming it, before any count runs. Each selected column
  is one statement, `SELECT COUNT(DISTINCT "column") FROM
  catalog.schema.table`, run through the coordinator's own statement
  path — admitted, recorded, planned and executed exactly as a client
  statement is, on the workers when the cluster has them — four at a
  time (fewer when `KAVEON_PRINCIPAL_QUERY_LIMIT` is lower), with the
  result cache off, under the `ANALYZE` statement's cancellation:
  cancelling the `ANALYZE` cancels the counts running, no further count
  starts, and nothing is stored. A count that fails fails the `ANALYZE`
  with that statement's status, code and message, the column named. The
  source version is read again after the counts; a change is 409
  `SOURCE_CHANGED`. The sub-statements are ordinary query records with
  `analyze:<id of the ANALYZE record>` in `client_tags`. `ANALYZE` holds
  no memory, principal or resource-group admission of its own while the
  counts run — each count is admitted in its own right.
- **Keeping what was measured across runs.** A column not counted by a
  statement keeps the exact count of the previous document when the
  source version is unchanged, and a metadata-only `ANALYZE` at the same
  version keeps the previous full read's sketches and exact bounds; a
  document at a new source version carries only what was measured under
  it. A sketch estimate stands in when there is no exact count.
- **`SHOW STATS FOR`** (any statement-capable role) presents the
  statistics on record, never a fresh read: Trino's columns plus
  `row_count` and `analyzed_at`, one row per column and a final summary
  row whose `column_name` is null and which carries the table's
  `row_count` and total `data_size`. A table that was never analyzed is
  400 `STATISTICS_UNAVAILABLE` with `no statistics for c.s.t; run ANALYZE
  c.s.t`.

  | Column | Type | Value |
  |---|---|---|
  | `column_name` | `VARCHAR` | The column; null on the summary row |
  | `data_type` | `VARCHAR` | The SQL spelling `DESCRIBE` uses (`bigint`, `varchar`, `decimal(10, 2)`, …) |
  | `data_size` | `BIGINT` | Compressed bytes of the column's chunks; on the summary row the data files' bytes as stored; null when the source does not record it |
  | `nulls_fraction` | `DOUBLE` | `nulls / row_count`; null when the null count is unknown or the table is empty |
  | `distinct_values_count` | `BIGINT` | The exact count of distinct non-null values when `ANALYZE … WITH (distinct = true \| columns = …)` counted the column under this source version, else the HyperLogLog estimate when `WITH (sketches = true)` read it, else null |
  | `low_value` / `high_value` | `VARCHAR` | The bounds as text: numbers as digits, dates and timestamps as ISO 8601, decimals exact; null when any file or row group lacks the bound |
  | `row_count` | `BIGINT` | Null on column rows; the exact row count on the summary row |
  | `analyzed_at` | `TIMESTAMP` | ISO 8601 UTC text (`2026-09-18T18:17:56.667Z`), the same on every row: when the document was computed or last refreshed |

- **`DESCRIBE DETAIL`** (any statement-capable role) is one row of
  table-level facts: from the statistics on record when the table was
  analyzed, else from a fresh metadata read, so it works before `ANALYZE`
  (400 `DESCRIBE_FAILED` with the storage error when the source cannot be
  read). Columns: `format (VARCHAR: parquet|delta|iceberg)`, `location
  (VARCHAR)`, `created_at (TIMESTAMP, null: no source records it)`,
  `last_modified (TIMESTAMP)`, `num_files (BIGINT)`, `size_in_bytes
  (BIGINT)`, `row_count (BIGINT)`, `delta_version (BIGINT)`,
  `partition_columns (VARCHAR, comma-separated)`, `analyzed_at
  (TIMESTAMP)`, `catalog_snapshot (VARCHAR, the published catalog
  snapshot the statement resolved the table under)`. `row_count` and
  `analyzed_at` are null until the table is analyzed. Timestamps are ISO
  8601 UTC text.

**The statistics object** (document version 3; versions 1 and 2 were the
product catalog's `ANALYZE` documents, which are no longer written or
read) is what `ANALYZE` stores, the two statements and
`GET /v1/catalog/tables/{id}/statistics` read, and the planner plans
from. It is JSON:

```json
{"version": 3, "table_id": "table:…",
 "source_version": {"identity_sha256": "…", "kind": "listing", "files": 3},
 "computed_at_ms": 1789841876667, "depth": "metadata",
 "format": "Parquet", "location": "/lake/events",
 "rows": 300, "bytes": 5232, "files": 3, "row_groups": 3,
 "uncompressed_bytes": 9600, "last_modified_ms": 1789841876000,
 "columns": [{"name": "id", "data_type": "Int64", "null_count": 0,
              "min": 0, "max": 299, "bounds_exact": true, "bytes": 1710}],
 "per_file": [{"path": "a.parquet", "rows": 100, "bytes": 1744,
               "columns": [{"min": 0, "max": 99, "null_count": 0}]}],
 "per_file_complete": true}
```

`source_version.kind` is `delta_version` (`version`), `iceberg_snapshot`
(`snapshot_id`), `listing` (`files`: a directory of Parquet files at one
listing) or `file`; `identity_sha256` is the digest `ANALYZE` and
planning key a source by. `depth` is `metadata` or `full` (the columns
were read for sketches). A column carries `distinct` (the HLL sketch) and
`quantiles` (the KLL sketch), each base64 of its compact encoding, after a
full read, `distinct_exact`
after a count; `bounds_exact` is false when a writer may have truncated a
text bound (the Delta log and Iceberg manifests carry no exactness flag).
`partition_columns` (absent when empty) is the Delta log's or the
`key=value` keys of a partitioned directory. `per_file` carries every
file's bounds while the table has at most 10,000 files
(`per_file_complete`); beyond that file skipping falls back to the
readers' own footer pruning. Every fact is a metadata read: Parquet
column facts are the row-group column-chunk statistics merged over row
groups and files; a Delta table whose add actions all carry `stats` is
profiled from the log alone, else from the active files' footers; an
Iceberg table's rows and bytes come from the manifests and its column
facts from the live files' footers, matched by field id. Bounds are JSON
of the logical type — numbers as numbers, strings as strings, dates and
timestamps as ISO 8601 strings, decimals as exact decimal text.

**What the planner does with them.** The statistics on record are read
for every table on a join side, under a filter, or in a
`COUNT(*)`/`MIN`/`MAX` aggregate, beside the source's current version
and exact row count.

- *Stale statistics cost, never answer.* Whatever their version, the
  statistics give a filtered scan an estimated cardinality — equality and
  `IN` from the distinct count, ranges from the quantiles or interpolated
  between the bounds, null tests from the null count, anything they cannot
  judge at 1.0 so a side is never understated — and the estimated rows and
  bytes decide the build side of an inner join and a broadcast (a build
  side under a million estimated rows and 256 MiB, at least four times
  smaller than the probe). Without statistics the choice is today's, from
  the exact row counts alone.
- *Current statistics answer.* A `SELECT COUNT(*) | MIN(col) | MAX(col)
  [, …] FROM t` with no predicate and no grouping, over a table whose
  statistics describe exactly the source version the statement is pinned
  to (the same identity, the same row count), is answered from them
  without a scan on any node — for a bound only when it is the column's
  true extreme and its null count is known (an all-null or empty column
  answers null). The record carries `execution: {"mode": "context",
  "detail": "statistics at <version label>", "source_version": {…},
  "current_source_version": {…}}` — the two versions equal by
  construction — and no `scans`. Every other case, including a version
  the record does not match exactly, scans; a statistics answer is held
  to the scanned answer by the differential tests.
- *Current statistics skip files.* A coordinator-local scan of a
  directory Parquet table under a predicate reads only the files whose
  recorded bounds admit it; the files left out are `files_skipped` on the
  scan (considered, never opened). Delta and Iceberg scans skip from
  their own metadata — the add actions' `stats`, the manifests' bounds
  and null counts — on every node, statistics or not. A distributed
  directory scan lists the location on each worker and does not carry
  the coordinator's pruned listing.
- *Automatic refresh.* When planning observes a source version newer
  than the one on record, the coordinator refreshes the statistics in the
  background (`KAVEON_STATISTICS_AUTO_REFRESH`, on by default; one
  refresh per table at a time): files added to a full document are read
  and their sketches folded in, a removal or a metadata-only document is
  recomputed at the document's depth. Until it lands, the old record
  costs and does not answer. Exact distinct counts do not survive a
  refresh.
- *The cube answers breakdowns.* Over a table with a declared shape whose
  cube is current for the pinned version, a statement that is exactly a
  `GROUP BY` over a subset of the declared dimensions (none is the grand
  total), the time column at its grain (the date column itself at day
  grain, or `DATE_TRUNC('day' | 'month', ts)` at the declared grain), or
  both — at most two axes counting the predicate's — with `COUNT(*)`,
  the declared `SUM`/`COUNT`/`MIN`/`MAX`, and `APPROX_COUNT_DISTINCT` (or
  a `COUNT(DISTINCT)` under `settings.approximate`, never an exact one)
  of a column declared `count_distinct`, under an optional conjunction
  of `dim = literal` and `dim IN (…)` predicates on declared dimensions,
  projected as they are (renamed at most), is answered from the cells:
  `execution: {"mode": "context", "detail": "cube at <version label>",
  …}`, `execution.approximate` naming each sketch's error, no `scans`.
  A predicate on a dimension not grouped by rolls the cells up along it.
  `ORDER BY`, `LIMIT`, `HAVING`, expressions over aggregates, an
  undeclared measure, a predicate on a measure or on the time column,
  a grouping the cube does not hold (an excluded axis, a pair beyond the
  pair limit) and every other shape take the row path. `use_statistics =
  false` stands the cube aside. A cube behind the source refreshes in
  the background under the same knob (added files folded in; a removal
  re-derived from the per-file partials when every measure is additive,
  rebuilt when a distinct count is declared) and answers nothing until
  it lands. The cube is held to the row path by the differential sweep
  (`the_cube_answers_what_the_row_path_answers`).

### Statistics endpoints

| Method | Path | Purpose |
|---|---|---|
| `GET` | `/v1/catalog/tables/{table_id}/statistics` | The statistics object on record with `source_version` (what it describes), `current_source_version` (the source as observed now, a metadata read: log tail, snapshot pointer or listing digest — no data read), `observed_at_ms` and `stale` (the two differ). 404 with `STATISTICS_UNAVAILABLE` when the table was never analyzed, plain 404 for an unknown id, 409 `TABLE_NOT_PUBLISHED` for a draft or retired table, 502 `SOURCE_UNAVAILABLE` when the source cannot be read |
| `GET` | `/v1/catalog/tables/{table_id}/version` | `{table_id, table, source_version, observed_at_ms}` — the current source version alone, the same cheap read; the platform's freshness signal, to call before an answer |
| `GET` | `/v1/statistics` | Admin only: every table with statistics on record (`table`, `table_id`, `row_count`, `source_version` label, `depth`, `computed_at`, `current`), at most 100 rows with `total` and `truncated` |

Both catalog endpoints follow the catalog API's authorization: the
catalog service credential or any authenticated principal.

### Per-request settings

A statement may carry a `settings` object. Each key is validated against the
coordinator's configuration and can only lower a bound, never raise one; an
unknown key, or a value outside its bound, is refused with HTTP 400 and code
`INVALID_SETTING`, the key named in the message.

| Key | Type | Bound | Effect |
|---|---|---|---|
| `query_memory_limit_bytes` | unsigned integer | 1 to `KAVEON_QUERY_MEMORY_LIMIT_BYTES` on the coordinator | The statement's query memory pool on the coordinator, and the limit every task of the statement is admitted with on the workers (each worker also caps it at its own configured limit). |
| `local_parallelism` | unsigned integer | 1 to the coordinator's configured parallelism (`KAVEON_LOCAL_PARALLELISM`) | Aggregator threads per task for the statement's partial aggregates, DISTINCT and final merges, on every node. Carried in the task request; each worker caps it at its own configured value. |
| `result_cache` | boolean | — | `false` bypasses the coordinator's result cache for this statement: no lookup, no insertion. See the settings reference. |
| `admission_wait_seconds` | unsigned integer | 0 to `KAVEON_MEMORY_ADMISSION_WAIT_SECONDS` on the coordinator | How long the statement waits for memory admission before HTTP 429; `0` refuses at once when its memory pool does not fit on arrival. See [Memory admission](#memory-admission). |
| `approximate` | boolean | — | `true` answers every plain `COUNT(DISTINCT col)` from a HyperLogLog sketch as `APPROX_COUNT_DISTINCT(col)` would, under COUNT's output name; the record's `execution.approximate` states the error. Default `false`. See [Approximate aggregates](engine-sql-compatibility.md#approximate-aggregates). |
| `use_statistics` | boolean | — | `false` bypasses every answer from statistics for this statement: no `context` answer for `COUNT(*)`/`MIN`/`MAX`, no statistics answer for `APPROX_*`; the rows are read. File skipping by the statistics' bounds still applies. `execution.detail` ends with `statistics bypassed` when the statistics would have answered. Default `true`. |

`time_zone` is not a settings key: it is the request's own `time_zone` field.

The same settings may lead the statement text as `SET SESSION <key> =
<value>;` statements in the same request, for clients that only send SQL:

```sql
SET SESSION result_cache = false;
SET SESSION local_parallelism = 2;
SELECT country, SUM(actions) FROM kaveon_events_enriched GROUP BY country
```

`SET SESSION time_zone = 'UTC'` sets the request's `time_zone`. A key given
both in the object and in the prefix must agree. HTTP is stateless and the
Engine keeps no server-side session: a `SET SESSION` statement applies only to
the statement submitted with it, and a request that is only `SET SESSION`
statements is refused with HTTP 400. The query record carries the effective
settings in its `settings` field, present only when the statement set
something; Studio shows them on the query page under Execution.

### Resource groups

Every statement is admitted through one resource group, chosen by the
ordered selectors from the principal, its role and the request's
`client_tags`; the group bounds its running statements, its memory share,
its queue and wait, its aggregator threads, and supplies default settings.
The record carries the group and its limits under `context.resource_group`
(`name`, `max_memory_bytes`, `max_concurrent`, `max_queued`,
`max_queue_wait_seconds`, `max_local_parallelism`, `priority`); Studio
shows it on the query page. The keys, the selector rules, the admission
order across groups and the configuration sources are in
[Governance](../engine/governance.md).

`GET /v1/admin/resource-groups` (admin) answers with `source`,
`store_path`, `admission_limit_bytes`, `groups`, `selectors` and
`counters` (one entry per group: `running`, `queued`, `admitted_bytes`,
`admitted`, `queued_total`, `rejected`, `withdrawn`, `wait_ms_p50`,
`wait_ms_p95` and the limits in force). `PUT /v1/admin/resource-groups`
(admin) takes `{"groups": [...], "selectors": [...]}` and replaces
everything at once: HTTP 200 with the same document as `GET` once it is in
force and written to `store_path`, HTTP 400 `INVALID_RESOURCE_GROUPS`
naming the group or selector otherwise, with nothing changed. Both are
HTTP 403 for any other role and 400 `NOT_COORDINATOR` on a worker.

A refusal by the group's own limit is HTTP 429 `RESOURCE_GROUP_REJECTED`
with `resource_group`, `limit` (`{"max_memory_bytes"}`, `{"max_queued"}`
or `{"max_queue_wait_seconds"}`) and `admission_wait_ms`; a refusal by the
node's pool or queue keeps the code below and names the group too.

### Audit ledger

`GET /v1/audit` (admin) reads the coordinator's append-only ledger,
oldest first: `{"records": [...], "next_cursor": <seq>}`, `next_cursor`
present when more follow and passed back as `cursor`. Filters: `since`,
`until` (Unix milliseconds, `YYYY-MM-DD`, or an RFC 3339 UTC timestamp),
`principal`, `kind` (comma-separated kinds or families `statement`,
`catalog`, `settings`, `auth`), `query_id`; `limit` 1 to 1000, default
200. `format=jsonl` streams every matching record as
`application/x-ndjson` for export. An invalid parameter is 400
`INVALID_AUDIT_QUERY`; a node without a ledger (a worker, or
`KAVEON_AUDIT_RETENTION_DAYS=0`) answers 404 `AUDIT_DISABLED`. Each record
carries `seq`, `ts_ms`, `kind` and the fields of its kind; the record
schema, the storage and the retention are in
[Governance](../engine/governance.md#the-audit-ledger). Query records
gained `row_count` (the whole result's rows, when the statement produced
one) and `error_code` (the stable code of a refusal or classified failure)
for the ledger's use; both are omitted when absent.

### Memory admission

Every statement is admitted against the coordinator's memory admission
limit (`KAVEON_MEMORY_ADMISSION_LIMIT_BYTES`) with its query memory pool
(`KAVEON_QUERY_MEMORY_LIMIT_BYTES`, or the request's
`query_memory_limit_bytes`). A statement whose pool fits on arrival, whose
group has a running slot and room in its share, runs at once. One that
does not waits in its group's queue (`KAVEON_MEMORY_ADMISSION_QUEUE`,
default 64, bounds every group together) until running statements release
enough, for at most the shortest of `KAVEON_MEMORY_ADMISSION_WAIT_SECONDS`
(default 60), the group's `max_queue_wait_seconds` and the request's
`admission_wait_seconds`. Within a group the head is admitted first and
only when its whole pool fits; across groups the group furthest below its
weighted share of the pool is next, and the pool is held for it until its
head fits ([the order](../engine/governance.md#the-admission-order)).

While it waits the statement is in the history with `state: "QUEUED"`, so
`GET /v1/query` shows it and `DELETE /v1/query/{query_id}` cancels it: the
statement leaves the queue at once and its submitter receives HTTP 409
`QUERY_CANCELED`. A submitter that closes its connection leaves the queue
the same way.

The refusal is HTTP 429 with code `MEMORY_ADMISSION_REJECTED` in three
cases: the node's queue is full on arrival, the request asked not to wait
(`admission_wait_seconds: 0`) and it is not next, or the node's wait
expired. The body carries `resource_group` and `admission_wait_ms`, how
long the statement waited before the refusal (zero for the first two). A
statement refused after waiting stays in the history as `FAILED` with the
same `admission_wait_ms` and the reason; one refused on arrival leaves no
record. Clients that retry should honour the wait they were given rather
than resubmitting at once: the coordinator has already held the request
for the configured time.

```json
{
  "error": "memory admission wait of 60 s expired: 2147483648 of 2147483648 bytes admitted, 3 statements waiting",
  "code": "MEMORY_ADMISSION_REJECTED",
  "admission_wait_ms": 60003
}
```

Every query record carries `admission_wait_ms`: zero when the statement
was admitted on arrival, otherwise the time between arrival and admission.
`elapsed_ms` starts at admission and does not include it, so
`completed_at_ms - submitted_at_ms` is approximately the sum of the two.
Studio shows the wait on the query page under Execution. The same counters
appear on `/v1/node` and, per node, on `/v1/cluster` under `admission`:
`limit_bytes`, `admitted_bytes`, `peak_admitted_bytes`, `queue_limit`,
`queue_depth` (waiting now), and the cumulative `admitted`, `queued`
(arrivals that waited), `rejected` and `withdrawn` (left the queue by
cancellation or disconnection); the coordinator adds `resource_groups`,
the same counters per group with the wait percentiles. Workers admit each
task of a distributed statement through the same queue (one group, no
concurrency bound) and report the same counters; a task's wait is
`admission_wait_us` in the stage telemetry.

### Result cache

The coordinator keeps complete results of finished statements
(`KAVEON_RESULT_CACHE_BYTES`, default 256 MiB, `0` disables;
`KAVEON_RESULT_CACHE_TTL_SECONDS`, default 600). A statement whose key
matches a kept result is answered from it without any worker or coordinator
execution. The key is the normalised statement text (trimmed, whitespace
collapsed, letters lowercased outside string literals and quoted
identifiers), the catalog, the schema, the catalog snapshot identity, the
Delta versions the planner pinned, and the request's `time_zone`. A catalog
publish, a committed product transaction and `DELETE /v1/cache` clear every
entry. Paged results of statements the coordinator ran itself are streamed
to disk and never held whole, so they are not kept.

A hit's query record carries `execution: {"mode": "cache", "detail": "hit"}`,
`cached_from` (the query whose result was served) and `cached_elapsed_ms`
(what that query took); its own `elapsed_ms` is the time to serve. The
statement response is otherwise the same as a live one. Studio shows "Cache"
in the query page's "Ran on" row and labels a SQL Lab result "From cache" or
"Live query". `settings.result_cache = false` bypasses the cache for one
statement: no lookup and no insertion. Every benchmark and qualification
script in this repository sends that bypass.
