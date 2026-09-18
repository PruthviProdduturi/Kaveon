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
| SQL Lab | `/api/v1/lab/*` | Discovery, saved queries, execution, CTAS, history, distinct values |
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
- <a id="streaming-pages"></a>**Streaming pages (Engine).** A statement submitted with
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

## Engine HTTP path — alpha

The Rust server exposes these routes:

| Method | Path | Current behavior |
|---|---|---|
| `POST` | `/v1/statement` | Parse, bind, plan, execute and retain a query result; inline (up to 16 MiB) or `result_delivery: "paged"` with `next_uri` pages; accepts per-request `settings` and leading `SET SESSION` statements; a parse error is a 400 `SYNTAX_ERROR` whose body carries `position: {line, column}` (one-based) when the parser names the failing token, absent for an error at end of input |
| `POST` | `/v1/statement` | Parse, bind, plan, execute and retain a query result; inline (up to 16 MiB) or `result_delivery: "paged"` with `next_uri` pages; accepts per-request `settings` and leading `SET SESSION` statements. Also runs [catalog statements](#catalog-statements) (`CREATE`/`DROP`/`ALTER` on the durable catalog, `SHOW`, `DESCRIBE`, `CALL system.register_table`) |
| `GET` | `/v1/query` | Return up to 100 newest process-local query records, queued and running ones included |
| `GET` | `/v1/query/{query_id}` | Return retained lifecycle, context, structured logical plan, result, and scan telemetry; while the state is `RUNNING`, `stages` (`completed_tasks`, `tasks`) and `scans` are updated as each distributed task completes, and `scan_metrics_complete` stays `false` until the statement finishes |
| `DELETE` | `/v1/query/{query_id}` | Cancel the query: a queued statement leaves the admission queue at once; a running one propagates cancellation to active worker tasks |
| `GET` | `/v1/cluster` | Coordinator and discovered-worker state, with each node's memory admission counters (`admission`) as last heartbeated |
| `GET` | `/v1/node` | Current node information, with the result cache counters on a coordinator and the node's memory admission counters (`admission`) |
| `DELETE` | `/v1/cache` | Drop every cached result (admin role) |
| `POST` | `/v1/node/heartbeat` | Register a worker heartbeat on a coordinator |
| `GET` | `/v1/catalog` | List catalogs |
| `GET`, `POST` | `/v1/catalog/definitions` | List or create durable catalog definitions |
| `GET`, `PUT`, `DELETE` | `/v1/catalog/definitions/{catalog_id}` | Read, revision-replace, or delete a durable catalog definition |
| `GET`, `POST` | `/v1/catalog/definitions/{catalog_id}/schemas` | List or create durable schema definitions |
| `GET`, `PUT`, `DELETE` | `/v1/catalog/schemas/{schema_id}` | Read, revision-replace, or delete a durable schema definition |
| `GET`, `POST` | `/v1/catalog/schemas/{schema_id}/tables` | List or create durable table definitions |
| `GET`, `PUT`, `DELETE` | `/v1/catalog/tables/{table_id}` | Read, revision-replace, or delete a durable table definition |
| `GET` | `/v1/catalog/{catalog}/schema` | List schemas |
| `GET` | `/v1/catalog/{catalog}/schema/{schema}/table` | List tables |
| `GET` | `/v1/query/{query_id}/results/{page}` | One page of a paged result, served while the statement still runs (owner-scoped, immutable once written, 15 min TTL): `200` with the rows, `202` + `Retry-After: 1` for the next page not yet flushed, `404` past the end or unknown, `410` when the statement failed or was cancelled — see [Streaming pages](#streaming-pages) |
| `GET` | `/v1/whoami` | The identity the security layer attached to the request: `principal`, `display` (null unless a validated sign-in supplied one), `role` (`reader`, `analyst`, `admin`) and `auth` (`static`, `bridge`, `entra`, `development`, `internal`). The client shows it in its session header; older coordinators answer 404 and the client hides the line |
| `GET` | `/v1/capabilities`, `/v1/statistics`, `/v1/auth/config` | What the coordinator supports (native `ANALYZE`, transactions), published exact statistics, and the Entra sign-in configuration for the UI |
| `POST` | `/v1/transaction`, `/v1/transaction/sql`, `/v1/transaction/{id}/stage`, `…/commit`, `…/rollback`, `…/recovery`; `GET` `/v1/transaction/metrics`, `/v1/products/{kind}`, `/v1/product/{kind}/{id}` | The bounded product-record transaction protocol and typed product reads; see the [SQL compatibility reference](engine-sql-compatibility.md#transaction-api-boundary) |
| `POST`, `GET` | `/v1/task`, `/v1/exchange`, `/v1/internal/exchange/*`, `/v1/internal/query/{query_id}/finish`, `/v1/internal/catalog/snapshot` | Worker task submission, exchange partition upload/download, query finish and cancellation, catalog replica; exchange-token authenticated, not client routes |
| `GET` | `/health`, `/ready`, `/ui` | Liveness, catalog readiness, and operational UI |

Catalog mutations through `/v1/catalog/*` require the configured catalog-admin bearer token, an actor header, and optimistic `If-Match` revisions for replacement; the same definitions are also created and dropped by [catalog statements](#catalog-statements) on `/v1/statement` under the submitting principal's role. Internal task/exchange routes use a separate shared bearer token. Statement clients authenticate with a principal token from `KAVEON_SECURITY_JSON` (roles `reader`, `analyst`, `admin`), an Entra bearer token, or the API bridge token with delegated `x-kaveon-principal`/`x-kaveon-role` headers; the server serves native TLS, applies a per-principal concurrent-statement limit, resource groups and memory admission, and scopes query records and paged results to their owner (`docs/engineering/engine-security-integration.md`). This is a credential boundary, not production identity federation: rotation without restart, tenant isolation and row/column policies remain gates, so keep the Engine on a private network during alpha.

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
`absent`, `relocated` or `unchanged`. One statement per request; the
session `catalog` and `schema` of the request resolve unqualified names.

```sql
CREATE CATALOG [IF NOT EXISTS] name WITH (storage = 'adls', account = '…', container = '…' [, root = '…'] [, credential = 'workload-identity:<reference>'])
CREATE CATALOG [IF NOT EXISTS] name WITH (storage = 'local', base_path = '<absolute path on the coordinator>')
CREATE CATALOG [IF NOT EXISTS] name WITH (storage = 's3', bucket = '…', region = '…' [, prefix = '…'])
DROP CATALOG [IF EXISTS] name [CASCADE | RESTRICT]
CREATE SCHEMA [IF NOT EXISTS] [catalog.]schema
DROP SCHEMA [IF EXISTS] [catalog.]schema [CASCADE | RESTRICT]
CREATE TABLE [IF NOT EXISTS] [catalog.][schema.]table [(column type [NOT NULL], …)]
    WITH (location = '<path within the catalog root>', format = 'parquet' | 'delta' | 'iceberg' [, access = 'shortcut' | 'optimized'])
CALL [catalog.]system.register_table(schema_name => '…', table_name => '…', table_location => '…' [, format => 'delta'])
CALL [catalog.]system.unregister_table(schema_name => '…', table_name => '…')
ALTER TABLE [IF EXISTS] [catalog.][schema.]table SET LOCATION '<path>'
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

### Memory admission

Every statement is admitted against the coordinator's memory admission
limit (`KAVEON_MEMORY_ADMISSION_LIMIT_BYTES`) with its query memory pool
(`KAVEON_QUERY_MEMORY_LIMIT_BYTES`, or the request's
`query_memory_limit_bytes`). A statement whose pool fits on arrival runs
at once. One that does not fit waits in a FIFO queue
(`KAVEON_MEMORY_ADMISSION_QUEUE`, default 64) until running statements
release enough budget, for at most `KAVEON_MEMORY_ADMISSION_WAIT_SECONDS`
(default 60) or the request's `admission_wait_seconds`. The head of the
queue is admitted first and only when its whole pool fits; nothing behind
it is admitted ahead of it. A resource group's queue, when the principal
has one, is passed before memory admission.

While it waits the statement is in the history with `state: "QUEUED"`, so
`GET /v1/query` shows it and `DELETE /v1/query/{query_id}` cancels it: the
statement leaves the queue at once and its submitter receives HTTP 409
`QUERY_CANCELED`. A submitter that closes its connection leaves the queue
the same way.

The refusal is HTTP 429 with code `MEMORY_ADMISSION_REJECTED` in three
cases: the queue is full on arrival, the request asked not to wait
(`admission_wait_seconds: 0`) and its pool does not fit, or the wait
expired. The body carries `admission_wait_ms`, how long the statement
waited before the refusal (zero for the first two). A statement refused
after waiting stays in the history as `FAILED` with the same
`admission_wait_ms` and the reason; one refused on arrival leaves no
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
cancellation or disconnection). Workers admit each task of a distributed
statement through the same queue and report the same counters; a task's
wait is `admission_wait_us` in the stage telemetry.

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
