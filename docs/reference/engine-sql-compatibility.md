# Engine SQL Compatibility

Status: **Alpha**. This page describes the standalone Rust Engine, not SQL sent by
Studio to registered databases. Registered-source SQL compatibility is determined
by the selected database and Kaveon's API guardrails.

## Current Engine support

| SQL feature | State | Notes |
|---|---|---|
| `SELECT` | Alpha | Local and object-store Parquet, Delta, and Iceberg tables resolved through the Engine catalog |
| `ANALYZE catalog.schema.table` | Alpha | Admin-only on the coordinator; publishes exact metadata row counts bound to the catalog and immutable storage-source identity through the ADLS catalog head |
| Catalog DDL: `CREATE CATALOG`, `CREATE SCHEMA`, `CREATE TABLE … WITH (location, format [, partitioned_by])`, `CALL system.register_table`, `ALTER TABLE … SET LOCATION`, `DROP CATALOG|SCHEMA|TABLE`, `SHOW CREATE TABLE`, `DESCRIBE`, `SHOW CATALOGS|SCHEMAS|TABLES` | Alpha | Registers existing Parquet, Delta and Iceberg tables in the durable catalog under the principal's role (admin for catalogs, analyst or admin for schemas and tables); columns are read from the source when omitted; a table is activated only after a metadata-only probe of its location. See the [API reference](api.md#catalog-statements) |
| Column projection and aliases | Alpha | Projection is strict; unknown or duplicate requested columns fail |
| `WHERE` comparisons and boolean expressions | Alpha | Row-level filter operator is implemented |
| `GROUP BY` | Alpha | Local and distributed columnar hash aggregation: partials on several threads that flush on memory pressure, a hybrid final merge that spills sub-partitions; `GROUP BY` without an aggregate is DISTINCT over the keys |
| `SUM`, `COUNT`, `AVG`, `MIN`, `MAX`, `COUNT(DISTINCT ...)` | Alpha | Weighted AVG and exact DISTINCT states are mergeable across workers |
| `LIMIT`, `OFFSET` | Alpha | Physical limit and offset operators; `ORDER BY … OFFSET n LIMIT m` plans as a top-N |
| Qualified table names | Alpha | `catalog.schema.table` resolution is supported by the catalog |
| `ORDER BY`, `NULLS FIRST/LAST`, TopN | Alpha | Local/distributed execution; Sort/TopN support bounded external merge runs |
| Equi joins | Alpha | INNER/LEFT/RIGHT/FULL locally and distributed (hash repartition or broadcast of the small side from exact statistics); comma joins with WHERE equalities become hash joins through the binder; ON conditions route like WHERE and an outer join filters its non-preserved side by single-side conjuncts |
| Cross joins | Alpha | Local Cartesian semantics and distributed broadcast-build path |
| Semi/anti joins | Alpha | `IN`/`NOT IN`/`EXISTS`/`NOT EXISTS` subqueries, locally and distributed (broadcast of the subquery side); correlated `EXISTS`/`NOT EXISTS` on one equality decorrelated by the binder |
| Scalar subqueries | Alpha | Uncorrelated scalar subqueries in WHERE/HAVING (an ungrouped aggregate, as a single-row cross join); correlated scalar aggregates on one or two equalities decorrelated into a join on the grouped aggregate |
| CTEs and derived tables | Alpha | Non-recursive CTEs; derived tables in FROM and joined with commas |
| Arithmetic expressions | Alpha | Compatible numeric coercion for supported primitive numeric types |
| `HAVING` | Alpha | Post-aggregation filter |
| Window functions | Alpha | `ROW_NUMBER`, `RANK`, `DENSE_RANK`, `LAG`, `LEAD`, aggregates with `OVER (PARTITION BY … ORDER BY …)` and `ROWS`/`RANGE`/`GROUPS` frames |
| Set operations | Alpha | `UNION [ALL]`, `INTERSECT`, `EXCEPT`, distributed (each side deduplicated on the workers) |
| Date/time | Alpha | `EXTRACT`, `DATE_TRUNC`, `DATE_PART`, `TO_CHAR`, `NOW`, `CURRENT_DATE`, `CURRENT_TIMESTAMP`, `DATE '…'` literals, `date ± INTERVAL 'n' DAY`; `MONTH`/`YEAR` intervals only against a DATE literal |
| Conditional, comparison and strings | Alpha | `CASE`, `COALESCE`, `BETWEEN`, `IN`, `LIKE`/`ILIKE` (Arrow kernels), `REGEXP_REPLACE`, `CAST`, concatenation, `UPPER`/`LOWER`/`LENGTH`/`TRIM`/`SUBSTR`/`REPEAT`/`REPLACE`/`LPAD`/`RPAD`; functions over dictionary columns run once per dictionary value the batch uses and keep a text result dictionary-encoded; `REGEXP_REPLACE` over plain text runs once per distinct value in the batch |
| Literals against columns | Alpha | Integer literals push down to narrow integer and Date32 columns; integer and decimal literals meet double columns; text literals against Date32 columns are coerced on every reader |
| `SUM(DISTINCT)`, `AVG(DISTINCT)` | Alpha | Exact mergeable distinct state |
| `APPROX_COUNT_DISTINCT` (`APPROX_DISTINCT`), `APPROX_PERCENTILE` | Alpha | Sketch-answered estimates with the error stated on the query record; from the table's statistics without a scan when they carry sketches at the pinned version, else computed over the rows. See [Approximate aggregates](#approximate-aggregates) |

## Approximate aggregates

Two aggregates answer with an estimate from a mergeable sketch, and say
so: every query record whose result includes one carries
`execution.approximate`, a list of `{function, argument, sketch, error,
error_kind}`. Exact `COUNT(DISTINCT)` and every other function are
unchanged; nothing is approximated unless the statement writes an
`APPROX_*` function or sets `approximate = true`.

| Function | Result | Sketch | Error stated |
|---|---|---|---|
| `APPROX_COUNT_DISTINCT(col)` — `APPROX_DISTINCT(col)` is Trino's name for the same function | `BIGINT` (`UInt64`): the estimated number of distinct non-null values | HyperLogLog, p = 12 (4 096 six-bit registers; the DLM's register layout and PostgreSQL's `hash_bytes_extended` over the value's canonical text, so a sketch built here merges with a stored one) | `error_kind: relative_standard_error`, 1.04 / √4096 = 1.6 % of the true count (one standard error) |
| `APPROX_PERCENTILE(col, p)` | `DOUBLE`: the value at fraction `p` of the column's distribution | KLL, k = 200 | `error_kind: rank_error`, 2.446 / k^0.9433 = 1.65 %: the value returned has a true rank within this of `p`, with about 99 % confidence |
| `APPROX_PERCENTILE(col, ARRAY[p, …])` | `List(Float64)`: one value per fraction, in the order written (a JSON array inline; an Arrow list on a page) | KLL, k = 200 | As above, per value |
| `APPROX_COUNT_DISTINCT_STATE(col)` | `VARCHAR`: the HyperLogLog sketch itself — its compact bytes, base64, the encoding the statistics document stores — so a client merges it with others (register-wise maximum; merges are exact, whatever the partitioning) | HyperLogLog, p = 12 | None: the state is exact; its estimate carries the error above |
| `COLUMN_STATISTICS(col)` | `VARCHAR`: the column's read profile over the rows aggregated as JSON — `nulls`, `min`, `max` (exact), `distinct` (HyperLogLog, base64), `quantiles` (KLL, base64; null for booleans and text) — what `ANALYZE … WITH (sketches = true)` records per column, over any group | HyperLogLog p = 12, KLL k = 200 | None: the profile is the state |

- `col` is a column reference (`*` is refused). `APPROX_COUNT_DISTINCT`,
  `APPROX_COUNT_DISTINCT_STATE` and `COLUMN_STATISTICS` take any sketchable
  type — booleans, integers, floats, decimals, text, dates, timestamps,
  and dictionaries over them; `APPROX_PERCENTILE` takes integers, floats
  and decimals (dates and timestamps are refused). The two state-returning
  functions are what `ANALYZE` runs on the workers; their results are not
  estimates, so the record's `execution.approximate` does not list them. The
  fractions are numeric constants in `[0, 1]`; `DISTINCT` inside an
  `APPROX_*` call is refused. Over no non-null values the count is `0` and
  a percentile is null.
- **From statistics.** An ungrouped statement over one table with no
  predicate whose statistics are current for the statement's pinned
  source version and carry the column's sketch (`ANALYZE … WITH (sketches
  = true)`) answers with no scan: `execution.mode = "context"`,
  `execution.detail = "statistics at <version> (hyperloglog p=12, kll
  k=200)"`. When `ANALYZE … WITH (distinct = true | columns = …)` counted
  the column exactly, the exact count answers and the note reads `sketch:
  "exact count", error: 0`. Statistics for another version, a predicate, a
  grouping, a join, or a missing sketch take the computed path; the
  statistics object holds one sketch per column for the whole table, so a
  grouped or filtered statement is never answered from it.
- **Computed.** Otherwise the aggregate builds its sketch over the scanned
  rows — a partial sketch per worker thread, merged on the final stage
  through the same encoded partial state every aggregate uses, memory
  accounted like any other state — and returns the estimate. HyperLogLog
  registers merge exactly, so a distinct count is the same on every
  execution path; a KLL sketch merged from partials compacts differently
  from one built in sequence, so a percentile agrees across paths within
  its rank error (the differential sweep compares approximate results
  within a tolerance rather than as text).
- **`approximate = true`** (`settings.approximate` or `SET SESSION
  approximate = true`) lets the planner answer a plain `COUNT(DISTINCT
  col)` from a HyperLogLog sketch exactly as `APPROX_COUNT_DISTINCT`
  would, under COUNT's output name; the record's note names the function
  as written (`COUNT`). Off by default.
- **`use_statistics = false`** stands every answer from statistics aside:
  `APPROX_*` computes over the rows and `COUNT(*)`/`MIN`/`MAX` scan;
  `execution.detail` ends with `statistics bypassed` when the statistics
  would have answered. File skipping by the statistics' bounds still
  applies.

## Not currently executable

- Correlated subqueries beyond one or two equalities: a correlated non-equality
  (TPC-H Q21's `l2.l_suppkey <> l1.l_suppkey`), `COUNT` in a correlated scalar,
  `LIMIT`/set operations inside a correlated subquery, `EXISTS` correlated on
  more than one column, correlated `IN`; each is refused by name.
- Non-equality join conditions (residual join filters) and `GROUPING SETS`/
  `CUBE`/`ROLLUP`, recursive CTEs, array/map/JSON types (the one list-typed
  result, `APPROX_PERCENTILE(col, ARRAY[…])`, is rendered, not operated on);
  approximate aggregates beyond `APPROX_COUNT_DISTINCT` and
  `APPROX_PERCENTILE` — `APPROX_MOST_FREQUENT` has no heavy-hitter sketch in
  the statistics object yet.
- Table-creating DDL and row DML: `CREATE TABLE AS`, `INSERT`, `UPDATE`,
  `DELETE`, `ALTER TABLE ADD/DROP COLUMN`. Catalog DDL registers tables that
  already exist in storage. The separate product transaction API accepts a
  bounded, revisioned metadata-record DML subset; it is not arbitrary OLTP SQL.
- Scalar optimizer statistics beyond row count. Native `ANALYZE` persists the
  exact row count and an immutable statistics document, but histogram/selectivity
  planning is not yet implemented.
- Dynamic filtering. Exchange output streams while a task runs; the consumer
  still downloads a whole payload before decoding it.

## Transaction API boundary

The Engine exposes an authenticated transaction API at `/v1/transaction`.
This is a deliberately bounded product-metadata contract, not a PostgreSQL
server or a general row-store protocol.

| Existing operation | Behavior |
|---|---|
| `BEGIN` | Creates an owner-isolated transaction session against the current product snapshot |
| `INSERT` | One explicit `(id, document_json)` record into a supported `kaveon.product` family |
| `UPDATE` | Revision-checked replacement of one product document; `id` and `revision` are required in `WHERE` |
| `DELETE` | Revision-checked deletion of one product document; `id` and `revision` are required in `WHERE` |
| `COMMIT` | Publishes a prepared immutable catalog snapshot through the configured product store |
| `ROLLBACK` | Discards the session without publishing its staged changes |

Each request carries one statement and a transaction ID after `BEGIN`. Product
operations are authenticated, owner-isolated, revision-checked, and limited to
the supported product families. Multi-row values, `INSERT ... SELECT`,
`RETURNING`, `ON CONFLICT`, savepoints, transaction modifiers, arbitrary user
tables, and parameter binding are rejected explicitly. The current contract
does not provide PostgreSQL MVCC isolation, row-level indexes, foreign-key
enforcement, or crash-recovery parity.

## CLI metadata surface

The native CLI provides metadata commands over the Engine HTTP API:
`SHOW CATALOGS`, `SHOW SCHEMAS [IN catalog]`, `SHOW TABLES [IN schema]`,
`SHOW COLUMNS FROM table`, `DESCRIBE table`, `USE [catalog.]schema`, and
single-quoted `LIKE` filters. These commands resolve catalog definitions and
are not emulations of PostgreSQL's `pg_catalog` or `information_schema`.
The coordinator answers the same `SHOW` and `DESCRIBE` statements on
`POST /v1/statement`, and `kaveon catalog|schema|table …` submits the catalog
DDL from the command line ([CLI guide](../guides/engine-cli.md#catalog-administration)).
The CLI currently has no PostgreSQL wire-protocol, JDBC, or ODBC compatibility
claim.

Unsupported syntax should be treated as unsupported even if the upstream SQL
parser accepts it. The executable contract is the intersection of parsing,
logical planning, and physical operator construction.

## Current execution semantics

`POST /v1/statement` returns synchronously and materializes the root result before JSON serialization unless the client requests paged delivery. A statement is admitted through the memory admission queue, bound against the catalog, and, where the stage planner can express the shape, run as dependency-gated stages with versioned worker fragments and streamed Arrow IPC exchanges; otherwise the coordinator runs it and records why (`execution.detail`). A statement whose normalized text, catalog snapshot, pinned Delta versions and time zone match a cached result is served from the coordinator's result cache unless the request sets `settings.result_cache = false`. Query IDs identify retained records; cancellation removes a queued statement or propagates to active worker tasks; history remains process-local. Native `ANALYZE` returns the qualified table and exact row count; capability discovery reports it only when the coordinator has a durable product-catalog authority configured. Coverage records: `../qualification/tpch/coverage.md` (21 of 22) and `../qualification/clickbench-2026-09-16.md`.

See [Architecture](../../ARCHITECTURE.md), [HTTP API](api.md), and
[Operations and troubleshooting](../operations-troubleshooting.md).
