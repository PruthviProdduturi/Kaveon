# Engine SQL Compatibility

Status: **Alpha**. This page describes the standalone Rust Engine, not SQL sent by
Studio to registered databases. Registered-source SQL compatibility is determined
by the selected database and Kaveon's API guardrails.

## Current Engine support

| SQL feature | State | Notes |
|---|---|---|
| `SELECT` | Alpha | Local and object-store Parquet, Delta, and Iceberg tables resolved through the Engine catalog |
| `EXPLAIN` | Alpha | Plan-only logical, optimized, and distributed/physical views through the native and Trino statement routes; `TYPE IO` returns the physical-plan projection because runtime IO statistics are not available before execution. `EXPLAIN ANALYZE` executes the statement once and returns the optimized plan plus measured output-row, analysis, planning, and execution timings. Unsupported output formats are rejected explicitly; plan-only forms do not start scans or worker tasks |
| `ANALYZE catalog.schema.table` | Alpha | Admin-only on the coordinator; publishes exact metadata row counts bound to the catalog and immutable storage-source identity through the ADLS catalog head |
| Catalog DDL: `CREATE CATALOG`, `CREATE SCHEMA`, `CREATE TABLE … WITH (location, format [, partitioned_by])`, `CALL system.register_table`, `ALTER TABLE … SET LOCATION`, `DROP CATALOG|SCHEMA|TABLE`, `SHOW CREATE TABLE`, `DESCRIBE`, `SHOW CATALOGS|SCHEMAS|TABLES` | Alpha | Registers existing Parquet, Delta and Iceberg tables in the durable catalog under the principal's role (admin for catalogs, analyst or admin for schemas and tables) and its [catalog access](../engine/governance.md#catalog-access) (`manage` on the catalog for schema and table changes; the metadata statements answer for granted catalogs only); columns are read from the source when omitted; a table is activated only after a metadata-only probe of its location. See the [API reference](api.md#catalog-statements) |
| Column projection and aliases | Alpha | Projection is strict; unknown or duplicate requested columns fail |
| `WHERE` comparisons and boolean expressions | Alpha | Row-level filter operator is implemented |
| `GROUP BY` | Alpha | Local and distributed columnar hash aggregation: partials on several threads that flush on memory pressure, a hybrid final merge that spills sub-partitions; `GROUP BY` without an aggregate is DISTINCT over the keys |
| `SUM`, `COUNT`, `AVG`, `MIN`, `MAX`, `COUNT(DISTINCT ...)` | Alpha | Weighted AVG and exact DISTINCT states are mergeable across workers |
| `LIMIT`, `OFFSET`, `FETCH FIRST/NEXT … ROWS [PERCENT] [WITH TIES]` | Alpha | Physical limit and offset operators; `LIMIT ALL` is an explicit no-limit form; ordinary `FETCH` plans as bounded TopN, `PERCENT` lowers through a full-row-count window and ceiling threshold, and `WITH TIES` lowers through a global `RANK()` boundary and preserves offset semantics; literal percentages are validated in the inclusive 0–100 range and conflicting `LIMIT`/`FETCH` forms are rejected explicitly |
| Qualified table names | Alpha | `catalog.schema.table` resolution is supported by the catalog |
| `ORDER BY`, `NULLS FIRST/LAST`, TopN | Alpha | Local/distributed execution; Sort/TopN support bounded external merge runs |
| Equi joins | Alpha | INNER/LEFT/RIGHT/FULL locally and distributed (hash repartition or broadcast of the small side from exact statistics); typed INT32, INT64, UInt64, Date32, UTF-8, and LargeUtf8 keys use exact single-key hash indexes, while other supported key types use the typed composite path; comma joins with WHERE equalities become hash joins through the binder; ON conditions route like WHERE and an outer join filters its non-preserved side by single-side conjuncts |
| Residual outer joins | Alpha | LEFT/RIGHT/FULL joins evaluate additional `ON` predicates before null extension, preserving SQL TRUE/FALSE/UNKNOWN semantics; equality-key joins use distributed hash candidates, while residual-only joins use an explicit broadcast candidate path and are not claimed as repartitioned joins |
| Cross joins | Alpha | Local Cartesian semantics and distributed broadcast-build path |
| Semi/anti joins | Alpha | `IN`/`NOT IN`/`EXISTS`/`NOT EXISTS` subqueries, locally and distributed; equality-key forms hash-partition both sides and emit only the matching (semi) or non-matching (anti) left rows, with a bounded broadcast fallback when a key cannot be resolved; correlated `EXISTS`/`NOT EXISTS` on one equality are decorrelated by the binder |
| Scalar subqueries | Alpha | Uncorrelated scalar subqueries in WHERE/HAVING (an ungrouped aggregate, as a single-row cross join); correlated scalar aggregates, including `COUNT`, on one or two equalities; `COUNT` uses a left join and zero-fills unmatched keys |
| Prepared statements | Alpha | Principal-owned `POST /v1/prepare`, describe, execute, and close lifecycle; execution reuses typed positional/named parameter binding and the normal query path. The Trino route also supports SQL-level `PREPARE`, `EXECUTE`, `DEALLOCATE`, `DESCRIBE INPUT`, and `DESCRIBE OUTPUT`; input metadata reports discovered positions/names, infers explicit `CAST` and PostgreSQL-style `::type` annotations, and leaves unresolved types as `unknown`, while output metadata reports projection names plus planner-inferred or explicit types |
| CTEs and derived tables | Alpha | Non-recursive CTEs; derived tables in FROM and joined with commas |
| Arithmetic expressions | Alpha | Compatible numeric coercion for supported primitive numeric types |
| `HAVING` | Alpha | Post-aggregation filter |
| Aggregate `FILTER (WHERE ...)` | Alpha | Supported on aggregate functions, including `COUNT`, `SUM`, and `COUNT_IF`; the predicate is evaluated with SQL three-valued semantics before the aggregate state is updated |
| Window functions | Alpha | `ROW_NUMBER`, `RANK`, `DENSE_RANK`, `LAG`, `LEAD`, aggregates with `OVER (PARTITION BY … ORDER BY …)` and `ROWS`/`RANGE`/`GROUPS` frames |
| `QUALIFY` | Alpha | Filters supported window results after window evaluation; partitioned `ROW_NUMBER` shapes are qualified across local and distributed execution |
| Set operations | Alpha | `UNION [ALL]`, `INTERSECT`, `EXCEPT`, `INTERSECT ALL`, and `EXCEPT ALL`; distributed exact duplicate-count semantics are covered by the differential sweep |
| Date/time | Alpha | `EXTRACT`, `DATE_TRUNC` over DATE calendar units and second/millisecond/microsecond/nanosecond timestamps with unit and timezone preservation, `DATE_PART`, `DATE_DIFF` from nanoseconds through weeks plus calendar month/quarter/year boundaries, `TO_CHAR`, `FROM_UNIXTIME`, `TO_UNIXTIME`, `NOW`, `CURRENT_DATE`, `CURRENT_TIMESTAMP`, `DATE '…'` and `TIMESTAMP '…'` literals, typed timestamp `± INTERVAL 'n'` HOUR/MINUTE/SECOND and typed `DATE`/timestamp `± INTERVAL 'n'` DAY/WEEK/MONTH/QUARTER/YEAR; month/year shifts use calendar arithmetic and clamp month-end days |
| Conditional, comparison and strings | Alpha | `CASE`, `IF`/`IIF`, `COALESCE`, `GREATEST`, `LEAST`, `NULLIF`, `BETWEEN`, `IN`, `IS DISTINCT FROM`, `IS NOT DISTINCT FROM`, `LIKE`/`ILIKE` (Arrow kernels), `REGEXP_REPLACE`, `REGEXP_COUNT`, `REGEXP_EXTRACT`, `REGEXP_EXTRACT_ALL`, `REGEXP_SPLIT`, `CHR`, `CODEPOINT`, `STRPOS`, `CAST`, `TRY_CAST`, `TRY(CAST(...))`, `TRY(JSON_PARSE(...))`, `CONCAT_WS`, `TRANSLATE`, concatenation, `UPPER`/`LOWER`/`LENGTH`/`TRIM`/`SUBSTR`/`SPLIT`/`SPLIT_PART`/`REVERSE`/`REPEAT`/`REPLACE`/`LPAD`/`RPAD`; `LEFT`, `RIGHT`, `LPAD`, `RPAD`, `REVERSE`, `REPEAT`, `STARTS_WITH`, and `ENDS_WITH` preserve nullable `LargeUtf8` columns and count Unicode scalars; `CHR` and `CODEPOINT` operate on Unicode code points and preserve NULLs; `CODEPOINT` rejects values containing anything other than one code point; `STRPOS` and `POSITION` accept Utf8/LargeUtf8 inputs, support Unicode character positions, and `STRPOS` supports positive and negative occurrence indexes; zero occurrences fail explicitly; `CONCAT_WS` skips NULL value arguments and returns NULL for a NULL separator; `TRANSLATE` maps Unicode code points by position, keeps unmapped source characters, and removes source characters without a corresponding target; `SPLIT` returns a typed text array (preserving LargeUtf8 input width), preserves NULL inputs, supports a positive limit whose final element contains the remainder, and splits into characters for an empty delimiter; `REGEXP_COUNT` counts non-overlapping matches and preserves NULL inputs; `REGEXP_EXTRACT_ALL` returns a typed text array of non-overlapping matches, or capture group 1 when present, and preserves NULL inputs; `REGEXP_SPLIT` returns a typed text array and preserves trailing empty fields; `SPLIT_PART` preserves LargeUtf8 input width, uses one-based indexes, returns NULL out of range, and rejects non-positive indexes; `GREATEST` and `LEAST` preserve the common input type and return NULL when any argument is NULL; `REGEXP_EXTRACT(string, pattern[, group])` returns capture group 0 by default or the requested zero-based group, and returns NULL for a non-match or missing capture |
| `ROW(...)`, `MAP(...)`, and `MAP_FROM_ENTRIES(...)` constructors | Alpha | Produces typed Arrow `StructArray`/`MapArray` values from list inputs with null-preserving child values; `MAP_FROM_ENTRIES` accepts `ARRAY[ROW(key, value)]`, preserves NULL outer rows, and rejects NULL or duplicate keys; `SPLIT_TO_MAP` parses delimited text into typed text maps, rejects empty delimiters and duplicate keys, and preserves NULL rows; `SPLIT_TO_MULTIMAP` groups repeated keys into ordered text arrays; 32-bit and 64-bit-offset list arrays, including mixed-offset comparisons, share the same semantics; `ROW(...).field`, row equality and lexicographic ordering, null-preserving element-wise array equality/inequality and lexicographic ordering, order-independent map equality, map subscript lookup, `MAP_KEYS`, `MAP_VALUES`, and Trino-compatible `MAP_CONTAINS_KEY(map, key)` (plus the legacy `CONTAINS_KEY` alias) expose typed projections; ordered map comparison remains open |
| Array and map functions/subscripts | Alpha | Numeric, text, and boolean `ARRAY[...]` literals preserve typed elements, including signed and unsigned 8/16/32/64-bit integers, `Float32`, and microsecond/nanosecond `TIME`; `SEQUENCE` builds bounded BIGINT ranges with inferred or explicit direction; `ZIP` builds typed row arrays and pads shorter inputs with NULL; `ARRAY_FIRST` and `ARRAY_LAST` return typed elements or NULL for empty/NULL arrays; `FLATTEN` concatenates nested typed arrays and preserves outer NULLs; `TRIM_ARRAY` removes a validated non-negative suffix count; `ARRAY_REPEAT` repeats typed numeric, text, boolean, date, and time scalars with a bounded non-negative count and preserves NULL elements; `ARRAY_UNION`, `ARRAY_INTERSECT`, and `ARRAY_EXCEPT` deduplicate typed values with Trino-compatible NULL equality and preserve list offsets; `ARRAY_CONCAT` accepts two or more matching typed lists; `CARDINALITY` over arrays and maps, `CONTAINS`, `ARRAYS_OVERLAP`, `ARRAY_POSITION`, `ARRAY_JOIN`, `ARRAY_APPEND`, `ARRAY_PREPEND`, `ARRAY_REMOVE`, `ARRAY_DISTINCT`, `ARRAY_SORT`, `ARRAY_REVERSE`, positive and negative-stride array slices, and one-based/negative `ELEMENT_AT` over numeric, text, and boolean lists/maps; `ARRAY_HISTOGRAM` preserves integer, floating-point, decimal, date, timestamp, boolean, and UTF-8 key types with deterministic ordering and null-element omission; 64-bit-offset lists are normalized for the supported scalar functions when their offsets fit the 32-bit execution representation; date, decimal, and second/millisecond/microsecond/nanosecond timestamp arrays preserve their element types with qualified `CARDINALITY`/`ARRAY_POSITION` coverage; broader element types and function combinations remain open |
| Literals against columns | Alpha | Integer literals push down to narrow integer and Date32 columns; integer and decimal literals meet double columns; text literals against Date32 columns are coerced on every reader |
| `SUM(DISTINCT)`, `AVG(DISTINCT)` | Alpha | Exact mergeable distinct state |
| `APPROX_COUNT_DISTINCT` (`APPROX_DISTINCT`), `APPROX_PERCENTILE` | Alpha | Sketch-answered estimates with the error stated on the query record; from the table's statistics without a scan when they carry sketches at the pinned version, else computed over the rows. See [Approximate aggregates](#approximate-aggregates) |

`JSON_ARRAY_CONTAINS(json_array, value)` supports boolean, integer,
floating-point, and text needles. It preserves NULL inputs, returns NULL for
malformed JSON, and returns `false` for valid non-array JSON values.

`JSON_ARRAY(...)` constructs a JSON array from scalar arguments and preserves
SQL NULL arguments as JSON `null`; its bounded `ABSENT ON NULL` form omits
NULL elements. `JSON_OBJECT(key : value, ...)` (or the
equivalent `key VALUE value` form) constructs an object from scalar pairs; the
bounded surface also accepts comma-separated key/value pairs, preserves SQL
NULL values as JSON `null`, supports `ABSENT ON NULL`, and rejects null or
duplicate keys. `JSON_PARSE` validates a text document,
`JSON_FORMAT` emits canonical JSON,
and `JSON_QUERY`/`JSON_EXTRACT` preserve object and array results. Their paths
support dotted keys, quoted bracket keys, numeric array indexes, and array
wildcards (`[*]`); a wildcard result is collected as a JSON array. `JSON_VALUE`
and `JSON_EXTRACT_SCALAR` return only scalar values and therefore return NULL
for object, array, or wildcard-array results. `JSON_EXISTS` reports whether a
supported path resolves, including a path that resolves to JSON null. Full
SQL/JSON coercion, error/empty clauses, recursive descent, and the broader
Trino path grammar remain outside the qualified surface.

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
  (TPC-H Q21's `l2.l_suppkey <> l1.l_suppkey`),
  `LIMIT`/set operations inside a correlated subquery, `EXISTS` correlated on
  more than one column, or multi-key correlated `IN`; each is refused by name.
- Residual outer joins are supported for the qualified local and distributed
  shapes described above; unqualified outer join forms outside that binder
  surface, recursive CTEs, ordered map and JSON comparison remain unsupported.
  Column-key `GROUPING SETS`, `CUBE`, and
  `ROLLUP` (including the grand-total `GROUPING SETS (())` form) lower to typed
  `UNION ALL` aggregate branches; grouping metadata
  `GROUPING(column)` and `GROUPING_ID(...)` are materialized for those
  expanded branches. Expansion is bounded at 4,096 aggregate branches so a
  query cannot construct an unbounded UNION plan; CUBE expansions with 16 or
  more keys remain unsupported and are rejected explicitly rather than
  silently treated as a plain `GROUP BY`;
  typed numeric, text, and boolean `ARRAY[...]` literals support `CARDINALITY`,
  `CONTAINS`, `ARRAY_POSITION`, `ARRAY_DISTINCT`, `ARRAY_SORT`,
  `ARRAY_REVERSE`, `ELEMENT_AT`, and positive or negative-stride slices;
  date, timestamp, and decimal arrays are currently qualified for selected
  scalar operations, and `ARRAY_CONCAT`, `ARRAY_APPEND`, `ARRAY_PREPEND`, and
  `ARRAY_REMOVE` preserve nested list, row, and map elements; broader array
  element types and function combinations remain open;
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
- Dynamic filtering beyond the qualified scan paths. Build-side keys are
  propagated for supported single-key and composite-key joins into Parquet,
  Delta, Iceberg, and object-store readers before decode; exact-result local
  and distributed regressions cover those paths. A representative cloud
  dataset with measurable file or row-group pruning, broader type/exchange
  coverage, and cross-coordinator snapshot qualification remain open.
- `DATE_DIFF` is supported for fixed units from nanoseconds through weeks and
  for calendar month, quarter, and year boundaries over DATE and all supported
  timestamp units. Month-family differences count complete calendar boundaries
  and preserve the sign for reverse ranges.

## Transaction API boundary

The Engine exposes an authenticated transaction API at `/v1/transaction`.
This is a deliberately bounded product-metadata contract, not a PostgreSQL
server or a general row-store protocol.

| Existing operation | Behavior |
|---|---|
| `BEGIN` | Creates an owner-isolated transaction session against the current product snapshot |
| `INSERT` | One or more explicit `(id, document_json)` records into a supported `kaveon.product` family; all rows stage under one transaction commit |
| `UPDATE` | Revision-checked replacement of one product document; `id` and `revision` are required in `WHERE` |
| `DELETE` | Revision-checked deletion of one product document; `id` and `revision` are required in `WHERE` |
| `COMMIT` | Publishes a prepared immutable catalog snapshot through the configured product store |
| `ROLLBACK` | Discards the session without publishing its staged changes |

Each request carries one statement and a transaction ID after `BEGIN`. Product
operations are authenticated, owner-isolated, revision-checked, and limited to
the supported product families. Multi-row `VALUES` are expanded into bounded
typed commands and committed atomically. `INSERT ... SELECT`,
`RETURNING`, `ON CONFLICT`, savepoints, transaction modifiers, arbitrary user
tables, and lake-table DML are rejected explicitly. Statement execution and the
authenticated product transaction endpoint accept typed positional (`$1`, `?`)
and named (`:name`) scalar or JSON-array parameters through the HTTP API; values
are bound before parsing/planning and are never interpolated into SQL. Missing
or extra positional and named values fail before execution. Nested arrays and
null array elements lower to typed
`ARRAY[...]` expressions; JSON objects with string keys lower to typed `MAP`
values (duplicate keys and malformed values are rejected before planning).
The Trino adapter also accepts scalar and `ARRAY[...]` `EXECUTE name USING`
values carried by
`X-Trino-Prepared-Statement`, including validated `DATE 'YYYY-MM-DD'` literals
that remain text until the target column's Date32 coercion. Compound values
are still limited to the supported array forms. The raw Trino statement route
also accepts owner-scoped SQL `PREPARE name FROM ...`, `EXECUTE name`, and
`DEALLOCATE PREPARE name`; prepared SQL still passes through the normal parser
and typed binder. `SHOW SESSION` on that route returns the Engine's supported
session settings (`query_memory_limit_bytes`, `admission_wait_seconds`,
`result_cache`, `approximate`, and `use_statistics`) in Trino's tabular
metadata shape. `SHOW SESSION LIKE 'pattern'` applies the same wildcard
filtering to session settings. `SHOW FUNCTIONS` and `SHOW FUNCTIONS LIKE 'pattern'` return
the Engine's supported scalar and aggregate function metadata in Trino's
standard six-column shape. Compound values beyond arrays,
timestamps, intervals, and the complete prepare/describe/bind wire lifecycle
remain explicitly unsupported. `EXECUTE IMMEDIATE 'sql' USING ...` is supported
for a quoted SQL literal with the same typed binder. `USE catalog.schema` and
`USE schema` emit Trino's session-update response headers.
The current product transaction contract
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

Every metadata statement is answered for the session's identity against
the catalogs it was granted ([catalog access](../engine/governance.md#catalog-access)):
`SHOW CATALOGS` lists granted catalogs; `SHOW SCHEMAS`, `SHOW TABLES`,
`DESCRIBE`, `SHOW CREATE TABLE`, `SHOW STATS FOR` and `DESCRIBE DETAIL`
on an ungranted catalog answer `catalog 'x' not found`, the same text
and code as for a catalog that does not exist; a table reference in a
query fails at bind the same way. There is no `information_schema` in the
Engine today; when one is added it is subject to the same projection —
it lists what the caller may see, never an administrator's view of hidden
objects.

Unsupported syntax should be treated as unsupported even if the upstream SQL
parser accepts it. The executable contract is the intersection of parsing,
logical planning, and physical operator construction.

## Current execution semantics

`POST /v1/statement` returns synchronously and materializes the root result before JSON serialization unless the client requests paged delivery. A statement is admitted through the memory admission queue, bound against the catalog, and, where the stage planner can express the shape, run as dependency-gated stages with versioned worker fragments and streamed Arrow IPC exchanges; otherwise the coordinator runs it and records why (`execution.detail`). A statement whose normalized text, catalog snapshot, pinned Delta versions and time zone match a cached result is served from the coordinator's result cache unless the request sets `settings.result_cache = false`. Query IDs identify retained records; cancellation removes a queued statement or propagates to active worker tasks; history remains process-local. Native `ANALYZE` returns the qualified table and exact row count; capability discovery reports it only when the coordinator has a durable product-catalog authority configured. Coverage records: `../qualification/tpch/coverage.md` (21 of 22) and `../qualification/clickbench-2026-09-16.md`.

The result cache is an analytical execution cache, not the source of truth for
product state. It stores complete finished coordinator results with bounded
byte, TTL, and LRU eviction; paged responses are not cached, workers do not
retain a second result copy, and administrators can clear it. KaveonDB
transaction commits and replays invalidate affected analytical entries, while
revision/CAS state remains authoritative. Repeated analytical reads are faster
without allowing a stale result to change transactional visibility or commit
semantics.

See [Architecture](../../ARCHITECTURE.md), [HTTP API](api.md), and
[Operations and troubleshooting](../operations-troubleshooting.md).
