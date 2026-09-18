# Engine SQL Compatibility

Status: **Alpha**. This page describes the standalone Rust Engine, not SQL sent by
Studio to registered databases. Registered-source SQL compatibility is determined
by the selected database and Kaveon's API guardrails.

## Current Engine support

| SQL feature | State | Notes |
|---|---|---|
| `SELECT` | Alpha | Local and object-store Parquet, Delta, and Iceberg tables resolved through the Engine catalog |
| `ANALYZE catalog.schema.table` | Alpha | Admin-only on the coordinator; publishes exact metadata row counts bound to the catalog and immutable storage-source identity through the ADLS catalog head |
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

## Not currently executable

- Correlated subqueries beyond one or two equalities: a correlated non-equality
  (TPC-H Q21's `l2.l_suppkey <> l1.l_suppkey`), `COUNT` in a correlated scalar,
  `LIMIT`/set operations inside a correlated subquery, `EXISTS` correlated on
  more than one column, correlated `IN`; each is refused by name.
- Non-equality join conditions (residual join filters) and `GROUPING SETS`/
  `CUBE`/`ROLLUP`, recursive CTEs, approximate aggregates, array/map/JSON types.
- General-purpose table DDL and row DML. The separate product transaction API
  accepts a bounded, revisioned metadata-record DML subset; it is not arbitrary
  OLTP SQL.
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
The CLI currently has no PostgreSQL wire-protocol, JDBC, or ODBC compatibility
claim.

Unsupported syntax should be treated as unsupported even if the upstream SQL
parser accepts it. The executable contract is the intersection of parsing,
logical planning, and physical operator construction.

## Current execution semantics

`POST /v1/statement` returns synchronously and materializes the root result before JSON serialization unless the client requests paged delivery. A statement is admitted through the memory admission queue, bound against the catalog, and, where the stage planner can express the shape, run as dependency-gated stages with versioned worker fragments and streamed Arrow IPC exchanges; otherwise the coordinator runs it and records why (`execution.detail`). A statement whose normalized text, catalog snapshot, pinned Delta versions and time zone match a cached result is served from the coordinator's result cache unless the request sets `settings.result_cache = false`. Query IDs identify retained records; cancellation removes a queued statement or propagates to active worker tasks; history remains process-local. Native `ANALYZE` returns the qualified table and exact row count; capability discovery reports it only when the coordinator has a durable product-catalog authority configured. Coverage records: `../qualification/tpch/coverage.md` (21 of 22) and `../qualification/clickbench-2026-09-16.md`.

See [Architecture](../../ARCHITECTURE.md), [HTTP API](api.md), and
[Operations and troubleshooting](../operations-troubleshooting.md).
