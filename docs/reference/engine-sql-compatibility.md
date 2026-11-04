# Engine SQL Compatibility

Status: **Alpha**. This page describes the standalone Rust Engine, not SQL sent by
Studio to registered databases. Registered-source SQL compatibility is determined
by the selected database and Kaveon's API guardrails.

## Current Engine support

| SQL feature | State | Notes |
|---|---|---|
| `SELECT` | Alpha | Local and object-store Parquet, Delta, and Iceberg tables resolved through the Engine catalog |
| `EXPLAIN` | Alpha | Plan-only logical, optimized, and distributed/physical views through the native and Trino statement routes; optimized and physical `Scan` nodes expose pinned source row/byte estimates, source provenance, and source identity when available. Row-preserving operators, joins, and set operations expose conservative bounded output caps derived from child estimates; these are upper bounds, not selectivity estimates. `TYPE IO` returns the physical-plan projection because runtime IO statistics are not available before execution. `EXPLAIN ANALYZE` executes the statement once and returns the optimized plan plus measured output-row, analysis, planning, and execution timings, followed by observed per-stage task, row, byte, and elapsed summaries. The estimate-diagnostics endpoint additionally reports exact `stage_boundaries` for measured fragment roots and final aggregate/TopN shortcut results when a physical-root estimate is available, with actual-row scope explicit (`stage_output_rows`, `query_result`, or bounded `derived_row_preserving` attribution through `Window`/`Project`/`Sort`). Unsupported output formats are rejected explicitly; plan-only forms do not start scans or worker tasks |
| `ANALYZE catalog.schema.table` | Alpha | Admin-only on the coordinator; publishes exact metadata row counts bound to the catalog and immutable storage-source identity through the ADLS catalog head |
| Catalog DDL: `CREATE CATALOG`, `CREATE SCHEMA`, `CREATE TABLE … WITH (location, format [, partitioned_by])`, `CALL system.register_table`, `ALTER TABLE … SET LOCATION`, `DROP CATALOG|SCHEMA|TABLE`, `SHOW CREATE TABLE`, `DESCRIBE`, `SHOW CATALOGS|SCHEMAS|TABLES` | Alpha | Registers existing Parquet, Delta and Iceberg tables in the durable catalog under the principal's role (admin for catalogs, analyst or admin for schemas and tables) and its [catalog access](../engine/governance.md#catalog-access) (`manage` on the catalog for schema and table changes; the metadata statements answer for granted catalogs only); columns are read from the source when omitted; a table is activated only after a metadata-only probe of its location. See the [API reference](api.md#catalog-statements) |
| Column projection and aliases | Alpha | Projection is strict; unknown or duplicate requested columns fail |
| `WHERE` comparisons and boolean expressions | Alpha | Row-level filter operator is implemented |
| `GROUP BY` | Alpha | Local and distributed columnar hash aggregation: partials on several threads that flush on memory pressure, a hybrid final merge that spills sub-partitions; `GROUP BY` without an aggregate is DISTINCT over the keys |
| `SUM`, `COUNT`, `COUNT_BIG`, `AVG`, `MIN`, `MAX`, `COUNT(DISTINCT ...)` | Alpha | `COUNT_BIG` is canonicalized to Kaveon's 64-bit `COUNT`; weighted AVG and exact DISTINCT states are mergeable across workers |
| `LIMIT`, `OFFSET`, `FETCH FIRST/NEXT … ROWS [PERCENT] [WITH TIES]` | Alpha | Physical limit and offset operators; `LIMIT ALL` is an explicit no-limit form; ordinary `FETCH` plans as bounded TopN, `PERCENT` lowers through a full-row-count window and ceiling threshold, and `WITH TIES` lowers through a global `RANK()` boundary and preserves offset semantics; literal percentages are validated in the inclusive 0–100 range and conflicting `LIMIT`/`FETCH` forms are rejected explicitly |
| Qualified table names | Alpha | `catalog.schema.table` resolution is supported by the catalog |
| `ORDER BY`, `NULLS FIRST/LAST`, TopN | Alpha | Local/distributed execution; Sort/TopN support bounded external merge runs |
| Equi joins | Alpha | INNER/LEFT/RIGHT/FULL locally and distributed (hash repartition or broadcast of the small side from exact statistics); typed INT32, INT64, UInt64, Date32, UTF-8, and LargeUtf8 keys use exact single-key hash indexes, while other supported key types use the typed composite path; comma joins with WHERE equalities become hash joins through the binder; ON conditions route like WHERE and an outer join filters its non-preserved side by single-side conjuncts |
| Residual outer joins | Alpha | LEFT/RIGHT/FULL joins evaluate additional `ON` predicates before null extension, preserving SQL TRUE/FALSE/UNKNOWN semantics; equality-key joins use distributed hash candidates, while residual-only joins use an explicit broadcast candidate path and are not claimed as repartitioned joins |
| Cross joins | Alpha | Local Cartesian semantics and distributed broadcast-build path |
| Semi/anti joins | Alpha | `IN`/`NOT IN`/`EXISTS`/`NOT EXISTS` subqueries, locally and distributed; equality-key forms hash-partition both sides and emit only the matching (semi) or non-matching (anti) left rows, with a bounded broadcast fallback when a key cannot be resolved; correlated `EXISTS`/`NOT EXISTS` on one equality are decorrelated by the binder |
| Scalar subqueries | Alpha | Uncorrelated scalar subqueries in WHERE/HAVING (an ungrouped aggregate, as a single-row cross join); correlated scalar aggregates, including `COUNT`, on one or two equalities; `COUNT` uses a left join and zero-fills unmatched keys; a bounded non-aggregate scalar `OFFSET n LIMIT 1` with equality-only correlations is decorrelated with per-key `ROW_NUMBER()` paging. Residual correlations, offset-only scalars, multi-row limits, and bounded derived `CROSS APPLY`/`OUTER APPLY` plus scalar correlated `VALUES` APPLY are qualified; table-valued functions and arbitrary correlated APPLY remain rejected explicitly |
| Prepared statements | Alpha | Principal-owned `POST /v1/prepare`, describe, execute, and close lifecycle; execution reuses typed positional/named parameter binding and the normal query path. The Trino route also supports SQL-level `PREPARE`, `EXECUTE`, `DEALLOCATE`, `DESCRIBE INPUT`, and `DESCRIBE OUTPUT`; input metadata reports discovered positions/names, infers explicit `CAST` and PostgreSQL-style `::type` annotations, and leaves unresolved types as `unknown`, while output metadata reports projection names plus planner-inferred or explicit types |
| CTEs and derived tables | Alpha | Non-recursive CTEs; derived tables in FROM and joined with commas |
| Arithmetic expressions | Alpha | Compatible numeric coercion for supported primitive numeric types |
| `HAVING` | Alpha | Post-aggregation filter |
| Aggregate `FILTER (WHERE ...)` | Alpha | Supported on aggregate functions, including `COUNT`, `SUM`, and `COUNT_IF`; the predicate is evaluated with SQL three-valued semantics before the aggregate state is updated |
| Window functions | Alpha | `ROW_NUMBER`, `RANK`, `DENSE_RANK`, `NTILE`, `PERCENT_RANK`, `CUME_DIST`, `LAG`, `LEAD`, `FIRST_VALUE`, `LAST_VALUE`, and `NTH_VALUE`, plus aggregates with `OVER (PARTITION BY … ORDER BY …)` and `ROWS`/`RANGE`/`GROUPS` frames. Value windows support the qualified `IGNORE NULLS` forms; partitioned and unpartitioned shapes have exact local/three-worker coverage (see the [value-window](../qualification/track2-window-value-functions-distributed-2026-10-18.md), [NTILE](../qualification/track1-window-ntile-ties-nulls-2026-10-18.md), and [percent-rank/CUME_DIST](../qualification/track1-window-percent-rank-cume-dist-2026-10-18.md) gates), while remaining mixed-key and fully parallel window shapes stay bounded/qualified separately |
| `QUALIFY` | Alpha | Filters supported window results after window evaluation; partitioned `ROW_NUMBER` shapes are qualified across local and distributed execution |
| Set operations | Alpha | `UNION [ALL]`, `INTERSECT`, `EXCEPT`, `INTERSECT ALL`, and `EXCEPT ALL`; distributed exact duplicate-count semantics are covered by the differential sweep |
| Date/time | Alpha | `EXTRACT`, `DATE_TRUNC` over DATE calendar units and second/millisecond/microsecond/nanosecond timestamps with unit and timezone preservation, `DATE_PART`/`DATEPART`, `DATE_DIFF`/`DATEDIFF`/`DATEDIFF_BIG` from nanoseconds through weeks plus calendar month/quarter/year boundaries, `DATE_ADD`/`DATEADD` (including SQL Server datepart abbreviations through microseconds and nanoseconds), bounded `DATE_BUCKET` using the SQL Server default origin, typed literal origin, or bounded row-local DATE/TIMESTAMP origin with literal or bounded row-local BIGINT widths (invalid dynamic widths return typed NULL; millisecond and microsecond buckets are qualified for timestamp inputs; nanosecond buckets remain rejected because execution timestamps are microsecond precision; unsupported origin shapes remain rejected), bounded `CONVERT`/`TRY_CONVERT` DATE styles `0`, `1`, `23`, `101`, `102`, `103`, `104`, `105`, `106`, `107`, `110`, `111`, `112`, `20`, `21`, `120`, `121`, and `126` over text columns, plus runtime timestamp style `113` (`dd mon yyyy hh:mi:ss:mmm`), bounded fixed-offset SQL Server `AT TIME ZONE` for `UTC`, `Z`, and `±HH:MM` offsets (naive DATE/timestamp values are interpreted in that offset and normalized to UTC; zoned inputs preserve their instant; NULLs remain NULL; named zones, DST rules, and dynamic/mixed zone expressions are rejected), `DATEFROMPARTS`, `DATETIMEFROMPARTS`, bounded `DATETIME2FROMPARTS` precisions `0`–`6`, bounded UTC-normalized `DATETIMEOFFSETFROMPARTS` precisions `0`–`6`, bounded `TIMEFROMPARTS` precisions `0`–`6`, `SMALLDATETIMEFROMPARTS`, `TO_CHAR`/`FORMAT` for temporal values with invariant date/time patterns, bounded invariant numeric `FORMAT` (`N0`–`N6`, `F0`–`F6`, and fixed decimal patterns), `FROM_UNIXTIME`, `TO_UNIXTIME`, `NOW`, `CURRENT_DATE`, `CURRENT_TIMESTAMP`, `DATE '…'` and `TIMESTAMP '…'` literals, typed timestamp `± INTERVAL 'n'` HOUR/MINUTE/SECOND and typed `DATE`/timestamp `± INTERVAL 'n'` DAY/WEEK/MONTH/QUARTER/YEAR; month/year shifts use calendar arithmetic and clamp month-end days. SQL Server `DATEPART(dw|weekday|w, ...)` uses the explicit default `DATEFIRST 7` numbering (Sunday=1 through Saturday=7). The common `SET DATEFIRST 7;` prefix is accepted as a no-op, and bounded `SET DATEFIRST 1` through `7` is carried immutably through local and distributed planning. General mutable session state remains outside the contract. See [DATEFIRST qualification](../qualification/track2-tsql-datefirst-default-2026-11-03.md) and [custom DATEFIRST qualification](../qualification/track2-tsql-datefirst-custom-2026-11-03.md) |
| Conditional, comparison and strings | Alpha | `CASE`, `IF`/`IIF`, `COALESCE`/`ISNULL`, `GREATEST`, `LEAST`, `NULLIF`, `BETWEEN`, `IN`, `IS DISTINCT FROM`, `IS NOT DISTINCT FROM`, `LIKE`/`ILIKE` (Arrow kernels), `REGEXP_REPLACE`, `REGEXP_COUNT`, `REGEXP_EXTRACT`, `REGEXP_EXTRACT_ALL`, `REGEXP_SPLIT`, `CHR`, `CODEPOINT`, `STRPOS`, SQL Server `CHARINDEX`, bounded SQL Server `PATINDEX`, `CAST`, `TRY_CAST` for bounded numeric, DATE, TIME, DATETIME, SMALLDATETIME, and DATETIME2/TIMESTAMP targets with safe NULL-on-error semantics, bounded invariant SQL Server `PARSE` for strict numeric and temporal casts, bounded invariant SQL Server `TRY_PARSE` for numeric, DATE, TIME, DATETIME2, and DATETIMEOFFSET targets, `TRY(CAST(...))`, `TRY(JSON_PARSE(...))`, `CONCAT_WS`, `TRANSLATE`, concatenation, `UPPER`/`LOWER`/`LENGTH`/`LEN`/`TRIM`/`SUBSTR`/`SPLIT`/`SPLIT_PART`/`REVERSE`/`REPEAT`/SQL Server `REPLICATE`/`REPLACE`/`LPAD`/`RPAD`; SQL Server `TRIM` supports qualified BOTH, LEADING, and TRAILING character sets with NULL-preserving Unicode scalar semantics; SQL Server `LEN` counts Unicode characters and excludes trailing U+0020 spaces; `CHARINDEX` uses SQL Server's needle/value argument order, optional one-based start location, Unicode scalar positions, and returns zero when absent; bounded `PATINDEX` supports `%` and `_` wildcards, removes leading `%` before calculating the one-based Unicode position, propagates NULL, and rejects bracket classes; `LEFT`, `RIGHT`, `LPAD`, `RPAD`, `REVERSE`, `REPEAT`, `STARTS_WITH`, and `ENDS_WITH` preserve nullable `LargeUtf8` columns and count Unicode scalars; `CHR` and `CODEPOINT` operate on Unicode code points and preserve NULLs; `CODEPOINT` rejects values containing anything other than one code point; `STRPOS` and `POSITION` accept Utf8/LargeUtf8 inputs, support Unicode character positions, and `STRPOS` supports positive and negative occurrence indexes; zero occurrences fail explicitly; `CONCAT_WS` skips NULL value arguments and returns NULL for a NULL separator; `TRANSLATE` maps Unicode code points by position, keeps unmapped source characters, and removes source characters without a corresponding target; `SPLIT` returns a typed text array (preserving LargeUtf8 input width), preserves NULL inputs, supports a positive limit whose final element contains the remainder, and splits into characters for an empty delimiter; `REGEXP_COUNT` counts non-overlapping matches and preserves NULL inputs; `REGEXP_EXTRACT_ALL` returns a typed text array of non-overlapping matches, or capture group 1 when present, and preserves NULL inputs; `REGEXP_SPLIT` returns a typed text array and preserves trailing empty fields; `SPLIT_PART` preserves LargeUtf8 input width, uses one-based indexes, returns NULL out of range, and rejects non-positive indexes; `GREATEST` and `LEAST` preserve the common input type and return NULL when any argument is NULL; `REGEXP_EXTRACT(string, pattern[, group])` returns capture group 0 by default or the requested zero-based group, and returns NULL for a non-match or missing capture; SQL Server `LOG`, two-argument `LOG`, `LOG10`, and `EXP` support bounded numeric inputs with NULL propagation and non-finite results rejected |
The bounded invariant SQL Server `TRY_PARSE` and `TRY_CAST` surface also includes
`TIME`, `DATETIME`, and `SMALLDATETIME`, in addition to `DATETIMEOFFSET`. Time
and timestamp text use the Engine's microsecond-precision representation;
`SMALLDATETIME` does not emulate SQL Server minute rounding. Fixed-offset text is normalized to a UTC instant with a
`+00:00` timezone marker; invalid or NULL input returns NULL, and locale-aware
`USING` forms remain rejected. See
[TRY_PARSE qualification](../qualification/track2-tsql-try-parse-2026-10-29.md)
and [TIME qualification](../qualification/track2-tsql-try-parse-time-2026-11-03.md).
Bounded `TRY_CAST(... AS DECIMAL(p,s))` is also qualified with typed NULL on
malformed or overflowing input; see the [decimal `TRY_CAST` qualification](../qualification/track2-tsql-try-cast-decimal-2026-11-04.md).
Strict invariant `PARSE` targets use the same typed cast boundary and are
qualified locally and across three workers; culture-aware `USING` forms remain
rejected. See the [PARSE qualification](../qualification/track2-tsql-parse-invariant-2026-11-04.md).

SQL Server `DATENAME` millisecond, microsecond, and nanosecond parts are also
qualified for typed timestamp expressions. The nanosecond spelling follows the
Engine's microsecond execution precision and is not a claim of 100-nanosecond
`datetime2` storage; see the [subsecond DATENAME qualification](../qualification/track2-tsql-datename-subsecond-2026-11-04.md).

| `ROW(...)`, `MAP(...)`, and `MAP_FROM_ENTRIES(...)` constructors | Alpha | Produces typed Arrow `StructArray`/`MapArray` values from list inputs with null-preserving child values; `MAP_FROM_ENTRIES` accepts `ARRAY[ROW(key, value)]`, preserves NULL outer rows, and rejects NULL or duplicate keys; `SPLIT_TO_MAP` parses delimited text into typed text maps, rejects empty delimiters and duplicate keys, and preserves NULL rows; `SPLIT_TO_MULTIMAP` groups repeated keys into ordered text arrays; 32-bit and 64-bit-offset list arrays, including mixed-offset comparisons, share the same semantics; `ROW(...).field`, row equality and lexicographic ordering, null-preserving element-wise array equality/inequality and lexicographic ordering, order-independent map equality, map subscript lookup, `MAP_KEYS`, `MAP_VALUES`, and Trino-compatible `MAP_CONTAINS_KEY(map, key)` (plus the legacy `CONTAINS_KEY` alias) expose typed projections; ordered map comparison remains open |
| Array and map functions/subscripts | Alpha | Numeric, text, and boolean `ARRAY[...]` literals preserve typed elements, including signed and unsigned 8/16/32/64-bit integers, `Float32`, and microsecond/nanosecond `TIME`; `SEQUENCE` builds bounded BIGINT ranges with inferred or explicit direction; `ZIP` builds typed row arrays and pads shorter inputs with NULL; `ARRAY_FIRST` and `ARRAY_LAST` return typed elements or NULL for empty/NULL arrays; `FLATTEN` concatenates nested typed arrays and preserves outer NULLs; `TRIM_ARRAY` removes a validated non-negative suffix count; `ARRAY_REPEAT` repeats typed numeric, text, boolean, date, and time scalars with a bounded non-negative count and preserves NULL elements; `ARRAY_UNION`, `ARRAY_INTERSECT`, and `ARRAY_EXCEPT` deduplicate typed values with Trino-compatible NULL equality and preserve list offsets; `ARRAY_CONCAT` accepts two or more matching typed lists; `CARDINALITY` over arrays and maps, `CONTAINS`, `ARRAYS_OVERLAP`, `ARRAY_POSITION`, `ARRAY_JOIN`, `ARRAY_APPEND`, `ARRAY_PREPEND`, `ARRAY_REMOVE`, `ARRAY_DISTINCT`, `ARRAY_SORT`, `ARRAY_REVERSE`, positive and negative-stride array slices, and one-based/negative `ELEMENT_AT` over numeric, text, and boolean lists/maps; `ARRAY_HISTOGRAM` preserves integer, floating-point, decimal, date, timezone-less timestamp, boolean, and UTF-8 key types with deterministic ordering and null-element omission; 64-bit-offset lists are normalized for the supported scalar functions when their offsets fit the 32-bit execution representation; date, decimal, and second/millisecond/microsecond/nanosecond timestamp arrays preserve their element types with qualified `CARDINALITY`/`ARRAY_POSITION` coverage; broader element types and function combinations remain open |
| Literals against columns | Alpha | Integer literals push down to narrow integer and Date32 columns; integer and decimal literals meet double columns; text literals against Date32 columns are coerced on every reader |
| `SUM(DISTINCT)`, `AVG(DISTINCT)` | Alpha | Exact mergeable distinct state |
| `APPROX_COUNT_DISTINCT` (`APPROX_DISTINCT`), `APPROX_PERCENTILE` | Alpha | Sketch-answered estimates with the error stated on the query record; from the table's statistics without a scan when they carry sketches at the pinned version, else computed over the rows. See [Approximate aggregates](#approximate-aggregates) |

General lake-table index and constraint DDL is intentionally fail-closed:
`CREATE INDEX`, `CREATE UNIQUE INDEX`, `DROP INDEX`, and
`ALTER TABLE ... ADD CONSTRAINT` are not accepted. `clustered_by` and Bloom
filters are physical layout hints, not transactional indexes or uniqueness
constraints. The supported integrity surface is the qualified KaveonDB
typed-row contract; broader constraint/index DDL requires durable catalog
metadata, writer enforcement, snapshot-replacement semantics, and restart /
distributed qualification before it can be advertised.

`REGEXP_LIKE(value, pattern)` is qualified for literal-pattern text-column
predicates with SQL NULL propagation and exact local/three-worker results;
see [`track2-regexp-like-column-2026-11-03.md`](../qualification/track2-regexp-like-column-2026-11-03.md).

Result values preserve logical text at the client boundary: Arrow `Utf8View`
values serialize as ordinary strings in direct rows and nested values, and
`Utf8View` map keys retain their string keys. This is qualified separately from
SQL planning and execution in
[`track5-utf8view-result-serialization-2026-11-02.md`](../qualification/track5-utf8view-result-serialization-2026-11-02.md).

The bounded `DATE_BUCKET` surface also accepts `week` as a seven-day unit and
normalizes wider calendar widths before arithmetic (for example, two weeks is
fourteen days and two years is twenty-four months). Typed custom origins are
advanced by the complete bucket offset. Calendar buckets normalize typed
TIMESTAMP origins to the DATE arithmetic domain; time-of-day buckets retain
timestamp precision. Exact local/three-worker qualification is recorded in
`docs/qualification/track2-tsql-date-bucket-week-2026-10-30.md` and
`docs/qualification/track2-tsql-date-bucket-width-origin-2026-11-03.md`.

`JSON_ARRAY_CONTAINS(json_array, value)` supports boolean, integer,
floating-point, and text needles. It preserves NULL inputs, returns NULL for
malformed JSON, and returns `false` for valid non-array JSON values.

The SQL Server/Fabric trigonometric scalar spellings `SIN`, `COS`, `TAN`,
`ASIN`, `ACOS`, `ATAN`, `DEGREES`, `RADIANS`, and `PI` are qualified for the
bounded finite numeric contract with NULL propagation; see the
[`track2-tsql-trigonometric-2026-11-03.md`](../qualification/track2-tsql-trigonometric-2026-11-03.md)
qualification record.

SQL Server's `ATN2(y, x)` spelling is also qualified through the same typed
two-argument arctangent path; `ATAN2` remains the generic Trino-compatible
name. See [`track2-tsql-atn2-2026-11-03.md`](../qualification/track2-tsql-atn2-2026-11-03.md).

`DATEPART(week|wk|ww, value)` uses SQL Server's default `DATEFIRST 7`
Sunday-first week-one numbering unless a bounded `SET DATEFIRST 1..7`
prefix is supplied. The selected value is encoded in the immutable plan so
local and distributed workers share the same weekday/week semantics; invalid
values remain explicitly rejected. Evidence is in
[`track2-tsql-datepart-week-2026-11-03.md`](../qualification/track2-tsql-datepart-week-2026-11-03.md).

`DATEPART(tzoffset, value)` is qualified for timestamp values carrying a
fixed `UTC`, `Z`, or `±HH:MM` offset and returns signed minutes. Naive values,
named zones, and malformed or out-of-range offsets fail closed; see
[`track2-tsql-datepart-tzoffset-2026-11-03.md`](../qualification/track2-tsql-datepart-tzoffset-2026-11-03.md).

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
SQL Server `ISJSON(text[, type_constraint])` is qualified for the `VALUE`,
`ARRAY`, `OBJECT`, and `SCALAR` constraints: it returns integer `1` when the
document matches, `0` for invalid/non-matching input, and NULL for NULL text.
Other constraint values remain explicitly rejected.

## Approximate aggregates

Two aggregates answer with an estimate from a mergeable sketch, and say
so: every query record whose result includes one carries
`execution.approximate`, a list of `{function, argument, sketch, error,
error_kind}`. Exact `COUNT(DISTINCT)` and every other function are
unchanged; nothing is approximated unless the statement writes an
`APPROX_*` function or sets `approximate = true`.

| Function | Result | Sketch | Error stated |
|---|---|---|---|
| `APPROX_COUNT_DISTINCT(col)` — `APPROX_DISTINCT(col)` is Trino's name for the same function | `BIGINT` (`UInt64`): the estimated number of distinct non-null values | HyperLogLog, p = 12 (4 096 six-bit registers; the DLM's register layout and Kaveon's self-contained PostgreSQL-compatible `hash_bytes_extended` implementation over canonical text, so a sketch built here merges with a stored one without a PostgreSQL runtime dependency) | `error_kind: relative_standard_error`, 1.04 / √4096 = 1.6 % of the true count (one standard error) |
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

### Transactional source `INSERT … SELECT`

KaveonDB supports a bounded product-record source copy: one same-kind
`kaveon.product` table, direct `id` and `document_json` projection, pinned
snapshot, owner isolation, and a 1,000-row cap. A literal `id IN (...)`
predicate may narrow the source. It reuses product conflict and revision-CAS
semantics. Joins, other filters, CTEs, arbitrary expressions,
cross-kind copies, lake/external sources, and arbitrary unbounded source scans
remain rejected. `RETURNING id, revision, document_json` is qualified from the
staged snapshot with or without the literal `id IN (...)` filter; the unfiltered
form still expands at most 1,000 source records. This is not general lake-table
DML.

## Bounded compatibility aliases

### SQL Server/Fabric compatibility aliases

The Engine accepts a deliberately bounded set of SQL Server/Fabric spellings:
`[bracketed_identifiers]`, `TOP n`, bounded `TOP n PERCENT`, `TOP n PERCENT WITH TIES`, and `TOP n WITH TIES`
with `ORDER BY`, `ISNULL`, `DATEADD`, `DATEDIFF`, `DATEPART`, `GETDATE`,
`GETUTCDATE`, `SYSDATETIME`, `SYSUTCDATETIME`, `ISDATE`, `ISNUMERIC`, `TRANSLATE`, `LEFT`, `RIGHT`, `REVERSE`, `STUFF`, `SPACE`, `TRIM` (including qualified BOTH, LEADING, and TRAILING character sets), `LTRIM`, `RTRIM`, unstyled `CONVERT`, and
unstyled `TRY_CONVERT` (including qualified numeric and UTF-8 text targets).
Literal DATE conversion styles `0`, `1`, `23`, `101`,
`102`, `103`, `104`, `105`, `106`, `107`, `110`, `111`, `112`, `20`, `21`,
`120`, `121`, and `126` are normalized to ISO dates;
runtime column style evaluation remains bounded. Runtime timestamp conversion
also accepts the implemented date-only styles `0`, `1`, `23`, `102`, `103`,
`104`, `105`, `106`, `107`, `110`, `111`, and `112`, returning midnight for
valid values and NULL under `TRY_CONVERT` for malformed values. Datepart
abbreviations such
as `dd`, `mm`, `yyyy`, `wk`, `mi`, and `ss` are normalized to the canonical
vectorized date operators. `DATEADD` also accepts `weekday`/`dw`/`w` and
`dayofyear`/`dy`/`y` as day-equivalent offsets, while `DATEPART` accepts those
aliases under Kaveon's deterministic calendar convention. `DATEPART` and
`DATENAME` also accept `iso_week`/`isowk`/`isoww` with ISO-8601 numbering;
the existing non-ISO `WEEK` contract remains separate. This is a
read-compatibility layer, not a claim of general T-SQL parity. The common
default read-session prefixes `SET ANSI_NULLS ON`, `SET QUOTED_IDENTIFIER ON`,
`SET ANSI_WARNINGS ON`, `SET NOCOUNT ON`, and `SET DATEFIRST 1..7;` are
accepted as read prefixes. The selected DATEFIRST value is encoded in the
plan so every worker uses the same immutable weekday/week semantics; invalid
values remain unsupported. `ISNULL`, `DATEADD` (including microsecond offsets on
microsecond-or-finer timestamps and nanosecond offsets on nanosecond
timestamps), `DATEDIFF`, `DATEDIFF_BIG`, and `DATEPART` are
qualified across the local and three-worker Parquet differential paths. The
`TOP PERCENT`, `TOP PERCENT WITH TIES`, and `TOP WITH TIES` are distributed-qualified when their
`ORDER BY` keys define a deterministic order. Without a complete tie-breaker,
SQL permits different tied rows and the qualification is withheld. Unsupported
`CONVERT` style codes, procedural T-SQL,
and unsupported SQL Server functions remain rejected explicitly.

The bounded identifier/string aliases also include `QUOTENAME` (square-bracket
or double-quote delimiters, with the 128-character limit) and
`STRING_ESCAPE(value, 'json')`; unsupported delimiter and escape-mode forms
fail closed.
SQL Server `SOUNDEX` and `DIFFERENCE` are also qualified for bounded ASCII
alphabetic text. `SOUNDEX` returns the invariant four-character code and
`DIFFERENCE` returns the number of matching code positions, with NULL
propagation. Non-ASCII input returns NULL rather than being transliterated or
silently interpreted under a worker-local code page; arbitrary locale-aware
phonetic behavior is outside this contract.
`FORMATMESSAGE` is qualified for literal templates with positional `%s`, `%d`,
`%i`, `%u`, and escaped `%%` tokens, with NULL propagation. `%i`/`%d` use
signed decimal text and `%u` accepts unsigned integer inputs. Width, precision,
numbered arguments, locale-aware forms, and message-ID lookup remain explicit
rejections.

`ISNULL` requires exactly two arguments and preserves the first argument's
type; `LEN`, UTF-8 byte-count `DATALENGTH`, `IIF`, `CHARINDEX`, `STUFF`, and `SPACE` are also qualified on the
distributed path. `STUFF` uses one-based Unicode scalar positions and returns
NULL for invalid start/length values; `SPACE` accepts bounded row-local counts,
preserving NULL and returning NULL for negative counts.
`ISJSON(text[, VALUE|ARRAY|OBJECT|SCALAR])` is qualified on the same bounded
distributed path, with SQL Server's integer validity result and NULL input
semantics; other type constraints fail closed.
SQL Server `ASCII`, `UNICODE`, bounded `CHAR`, and bounded `NCHAR` are qualified as an
invariant UTF-8/byte contract: `ASCII` returns the leading ASCII scalar and
NULL for a non-ASCII leading scalar, `UNICODE` returns the leading Unicode
scalar, and `CHAR` accepts integer values 0 through 255, returning NULL outside
that range. `NCHAR` accepts valid Unicode scalar values and returns NULL for
NULL, surrogate, negative, or out-of-range values. Locale/code-page conversion
is not claimed. `NULLIF` is qualified for row-local scalar expressions and
preserves SQL NULL-on-equality semantics through the vectorized conditional
path.
SQL Server `ROUND` is qualified for one- and two-argument forms plus the
bounded third `operation` argument (`0` round, `1` truncate toward zero), with
NULL propagation and explicit rejection of other operation values.
The bounded SQL Server table-valued `STRING_SPLIT` form is also qualified for
scalar value/separator inputs (including row-local expressions through
`CROSS JOIN LATERAL` or bounded `CROSS APPLY`) and literal ordinal mode `0`/`1`, lowering to `SPLIT`
plus `UNNEST`; arbitrary table-valued functions and dynamic ordinal modes
remain rejected.
`EOMONTH` is translated to the typed calendar operators and returns a `DATE`
for date or timestamp inputs, including a row-local month offset. Its
qualification is limited to those input types; it does not imply support for
the broader SQL Server date-construction family. `DATEFROMPARTS` is qualified
for valid year/month/day parts, `DATETIMEFROMPARTS` is qualified for valid
calendar and time parts, `DATETIME2FROMPARTS` is qualified for valid calendar
and time parts at precisions 0 through 6, bounded UTC-normalized
`DATETIMEOFFSETFROMPARTS` is qualified at precisions 0 through 6,
`TIMEFROMPARTS` is qualified for valid time parts at precisions 0 through 6,
and `SMALLDATETIMEFROMPARTS` is
qualified for valid calendar parts with minute precision, with SQL Server-style
NULL propagation and explicit
rejection of invalid calendar dates or unsupported arities.
`DATETRUNC(datepart, value)` is a qualified SQL Server alias for the same
typed `DATE_TRUNC` path, with the identical supported datepart and precision
boundary. See
[`track2-tsql-datetrunc-2026-10-30.md`](../qualification/track2-tsql-datetrunc-2026-10-30.md).
SQL `CAST`/`TRY_CAST` and SQL Server `CONVERT`/`TRY_CONVERT` to `TIME` are
qualified for UTF-8 text and existing Arrow time values at microsecond
precision. Strict casts reject malformed values; safe forms return typed NULL
for malformed or NULL input. Other
numeric and timestamp-to-time coercions remain outside this bounded contract.
The SQL Server time-text styles `8`/`108` (`HH:MM:SS`) and `114`
(`HH:MM:SS:mmm`) are also qualified for strict and safe conversion, with
malformed values rejected or nulled respectively; other time styles remain
unsupported. See
[`TIME conversion styles`](../qualification/track2-tsql-convert-time-styles-2026-11-03.md).
`DATEADD` over microsecond `TIME` values is qualified for fixed-duration
hour, minute, second, millisecond, and microsecond units with modulo-day
wrapping; calendar and nanosecond units remain unsupported. See
[`TIME DATEADD`](../qualification/track2-tsql-time-dateadd-2026-11-03.md).
`DATENAME` is qualified for calendar parts and returns deterministic English
month/weekday names or textual numeric parts; its bounded `tzoffset` form is
qualified for timestamps carrying a fixed timezone offset and returns the
SQL Server-shaped signed `+HH:MM`/`-HH:MM` text. NULL timestamps propagate
to NULL, while naive timestamps and named zones are rejected explicitly;
locale/session-language formatting is intentionally not claimed. See
[`DATENAME(tzoffset)` qualification](../qualification/track2-tsql-datename-tzoffset-2026-11-03.md).
`ISDATE` returns a non-null boolean validity result for text and temporal
inputs; numeric values are not interpreted as epoch dates.
`ISNUMERIC` returns a non-null boolean result for native numeric values and
text accepted by the safe numeric cast; locale-specific currency parsing is
not claimed.
Runtime `TRY_CONVERT(DATE, text, style)` for styles `0`, `1`, `23`, `101`, `102`, `103`, `104`, `105`, `106`, `107`, `110`, `111`, `112`, `20`, `21`, `120`, `121`, and `126`
is qualified across local and three-worker Parquet execution; malformed and
NULL inputs remain NULL. See
[`track2-tsql-convert-runtime-styles-2026-10-29.md`](../qualification/track2-tsql-convert-runtime-styles-2026-10-29.md).
`CONVERT(VARCHAR, expression)` and `TRY_CONVERT(VARCHAR, expression)` are
also qualified, as is `TRY_CAST(expression AS VARCHAR)`; unbounded
`NVARCHAR` uses the same Engine UTF-8 text representation at the planner
boundary. Sized
`VARCHAR(n)`, `NVARCHAR(n)`, `CHAR(n)`, and equivalent text casts are qualified
for a bounded UTF-8 contract: the width counts Unicode scalars, variable-width
forms truncate without padding, and `CHAR(n)` truncates then pads with U+0020.
The contract is deliberately not a SQL Server code-page or UTF-16 storage
emulation; binary-style, locale-sensitive, and unbounded expansion semantics
remain outside the boundary. Lengths above 1,000,000 scalars and sized text
`CONVERT` style codes fail closed. See
[`track2-tsql-convert-varchar-2026-11-03.md`](../qualification/track2-tsql-convert-varchar-2026-11-03.md).
Unstyled `TRY_CONVERT(DATETIMEOFFSET, expression)` is qualified for invariant
fixed-offset text inputs, with malformed and NULL values returning NULL through
the safe-cast path. Strict `CONVERT(DATETIMEOFFSET, expression)` uses the same
parser and fails the statement for malformed non-NULL input. Styled literal `CONVERT`/`TRY_CONVERT` to the T-SQL timestamp family is also
planner-qualified: ISO date-time styles preserve the supplied time, while
date-only styles produce midnight. Runtime text-column timestamp styles now
have local vectorized evaluator coverage for ISO styles `20`, `21`, `120`,
`121`, and `126`, date-only styles through `112`, and SQL Server style `113`
(`dd mon yyyy hh:mi:ss:mmm`) with invariant English month names, with
`TRY_CONVERT` NULL/error behavior. Styles `20`, `21`, `120`, `121`, `126`,
`101`, and `113` are qualified across the local and three-worker differential. See
[`track2-tsql-convert-timestamp-literals-2026-10-29.md`](../qualification/track2-tsql-convert-timestamp-literals-2026-10-29.md).
See the dedicated strict conversion qualification for the unstyled form:
[`track2-tsql-convert-datetimeoffset-2026-11-03.md`](../qualification/track2-tsql-convert-datetimeoffset-2026-11-03.md).
`LEFT` and `RIGHT` are qualified for bounded Unicode scalar text lengths and
preserve NULLs across local and three-worker execution.
`REVERSE` is qualified for the same Unicode scalar and NULL-preserving path.
Arrow `Utf8View` inputs are accepted and return ordinary `VARCHAR` output;
the focused executor boundary is recorded in
[`track2-utf8view-reverse-2026-11-03.md`](../qualification/track2-utf8view-reverse-2026-11-03.md).
`TRIM`, `LTRIM`, and `RTRIM` are qualified for ASCII-space boundary trimming
with NULL preservation.
`TRANSLATE` is qualified for Unicode scalar position mapping, SQL Server's
first-match behavior, unmapped-character preservation, source-only removal,
and NULL propagation.
`STRING_ESCAPE(value, 'json')` is qualified for UTF-8 text with SQL Server
NULL propagation; escape modes other than `json` are rejected explicitly.
`QUOTENAME` supports default square-bracket and double-quote delimiters,
doubles closing delimiters, preserves NULLs, and returns NULL for identifiers
longer than 128 Unicode characters; other delimiters are rejected explicitly.
SQL Server `REPLICATE(text, count)` is accepted through the bounded Unicode
`REPEAT` path for non-negative integer-literal or bounded row-local `BIGINT`
expressions (including arithmetic and casts); NULLs propagate and a negative
count returns NULL as in SQL Server.
Binary inputs, legacy 8,000-byte truncation, and unbounded expansion remain
outside the qualified contract.

`PIVOT` and `UNPIVOT` have bounded lowerings for derived sources with simple
columns and static `IN` lists of at most 128 values. `PIVOT` lowers to
conditional aggregation and `UNPIVOT` to `UNION ALL`; both have exact local
and three-worker differential evidence in
`docs/qualification/track2-tsql-pivot-unpivot-distributed-2026-10-28.md`.
An outer `ORDER BY` is preserved by both lowerings. Ordered PIVOT is qualified
across layouts and workers. Ordered UNPIVOT is qualified when the ordering keys
define a total order; ties remain subject to normal SQL ordering semantics.
The bounded `PIVOT(COUNT(*))` form is also accepted; direct-table, join-backed,
dynamic-column, and unbounded forms remain explicitly rejected.
Bounded `CROSS APPLY` over a parenthesized `SELECT` or `VALUES` derived
relation lowers to the existing lateral-join path, including scalar correlated
`VALUES` expressions, with exact local and
three-worker evidence in
`docs/qualification/track2-tsql-apply-distributed-2026-10-28.md`.
Bounded `OUTER APPLY` over the same derived forms lowers to a left-lateral join
and preserves NULL extension for an empty derived result through local and
three-worker execution; scalar correlated `VALUES` expressions are qualified
on the same path. Unbounded forms, arbitrary correlated derived `SELECT`
forms, and table-valued APPLY are recognized and rejected with rewrite
guidance rather than being treated as generic syntax failures.
The unordered `STRING_AGG(value, literal_separator)` form is qualified: NULL
values are omitted, empty groups return NULL, and partial workers carry a
mergeable string state bounded to 16 MiB per group. A predicate
`FILTER (WHERE ...)` is also qualified by lowering the predicate to a
NULL-preserving aggregate input before the same mergeable state path. Merge
order is unspecified, matching SQL Server's unordered form. The bounded
`WITHIN GROUP (ORDER BY value [ASC|DESC])` form is also qualified: workers
retain value strings, merge them under a 1,000,000-value/16 MiB per-group
bound, and sort once during finalization. Independent or multiple order keys,
explicit NULL placement, and dynamic separators remain explicitly rejected.
SQL-standard/Trino `LISTAGG(value[, literal_separator]) WITHIN GROUP (ORDER BY
value [ASC|DESC])` is also qualified when the separator is a literal (or
omitted, which defaults to the empty string) and there is one value-order key.
It reuses the same bounded mergeable state. `ON OVERFLOW`, dynamic separators,
multiple order keys, explicit NULL-placement clauses, and other unbounded
forms fail closed until separately qualified. `STRING_AGG(DISTINCT value, literal_separator)` is qualified through an exact
distinct projection over grouping keys and the value before the existing
mergeable state; mixed aggregate lists and expression-valued forms remain
rejected. Focused planner, state round-trip, and three-worker differential
evidence is recorded in
`docs/qualification/track2-tsql-string-agg-within-group-2026-11-02.md` and
`docs/qualification/track2-tsql-string-agg-distinct-2026-11-03.md`.
The embedded/local CLI planner uses the same bounded aggregate state and
separator validation, so local and server execution share this declared
surface; the same ordered boundary is enforced in both paths.

- Correlated subqueries beyond the qualified bounded forms: residual
  correlations inside scalar `OFFSET`/`LIMIT` forms, `LIMIT`/set operations
  inside other correlated subqueries, `EXISTS` shapes whose correlation cannot
  be reduced to the supported semi/anti-join keys, and multi-key correlated
  `IN` outside the keyed membership contract are refused by name. Residual
  non-equality predicates are qualified for bounded `EXISTS`/`NOT EXISTS`
  semi/anti joins, including the TPC-H Q21 shape
  (`l2.l_suppkey <> l1.l_suppkey`); see
  [`docs/qualification/tpch/coverage.md`](../qualification/tpch/coverage.md).
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
  silently treated as a plain `GROUP BY`. SQL Server/ClickHouse modifier
  spellings `GROUP BY a, b WITH ROLLUP` and `GROUP BY a, b WITH CUBE` lower to
  the same explicit branches; `WITH TOTALS` and mixed modifier forms remain
  explicit rejections. See
  `docs/qualification/track2-tsql-group-by-modifiers-2026-11-02.md`;
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
- Table-creating DDL and general lake-row DML: `CREATE TABLE AS`, `INSERT`,
  `UPDATE`, `DELETE`, `ALTER TABLE ADD/DROP COLUMN`. Catalog DDL registers
  tables that already exist in storage. The separate product transaction API
  accepts a bounded, revisioned metadata-record DML subset, including
  `RETURNING id, revision, document_json` on product creates, updates, upserts,
  and revision-checked deletes; bounded source-copy `INSERT ... SELECT`
  document projections are qualified with or without a literal `id IN (...)`
  filter, with the unfiltered form capped at 1,000 source IDs. Its typed-row facade validates declared primary/unique
  keys and product references, but it is not arbitrary OLTP SQL or general
  constraint DDL.
- Universal cost-based optimization. Native `ANALYZE` persists an immutable
  source-version-bound statistics document with exact metadata, sketches,
  quantiles, and bounded numeric histograms; the planner still falls back
  conservatively when a shape or source lacks a usable summary.
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
| `BEGIN` / `BEGIN TRAN` / `BEGIN TRANSACTION` | Creates an owner-isolated transaction session against the current product snapshot |
| `INSERT` | One or more explicit `(id, document_json)` records into a supported `kaveon.product` family; all rows stage under one transaction commit; bounded `OUTPUT inserted.column` or `OUTPUT inserted.*` is accepted as the `id, revision, document_json` result projection |
| `UPDATE` | Revision-checked replacement of one product document; an optional simple target alias is accepted, `id` and `revision` are required in `WHERE`, and bounded `OUTPUT inserted.column` or `OUTPUT inserted.*` is accepted. A bounded same-kind source form also accepts one aliased product table with `SET target.document_json = source.document_json`, a literal revision, one literal source id plus one or more literal target ids (including a 1,000-id `IN` list), or a target/source identity join paired with a literal target-id list. The same `RETURNING` contract applies; arbitrary joins and source predicates remain rejected. See [UPDATE FROM qualification](../qualification/track2-native-dml-update-from-2026-11-03.md) |
| `DELETE` | Revision-checked deletion of one product document; an optional simple target alias is accepted, `id` and `revision` are required in `WHERE`, and bounded `OUTPUT deleted.column` or `OUTPUT deleted.*` is accepted. A bounded same-kind `DELETE FROM target USING source` form also accepts a literal revision, a target-id list capped at 1,000 ids, and one literal source id; the source is owner-checked before the target CAS deletes are staged. Arbitrary `USING` joins remain rejected. |
| `MERGE` | One bounded `VALUES` batch (up to 1,000 source rows), with an optional simple target alias, can update-or-insert, delete-or-insert, matched-update-only, matched-delete-only, unmatched-insert-only, or matched update plus `WHEN NOT MATCHED BY SOURCE THEN DELETE`; matched updates may copy `source.document_json` or assign one bounded string literal, while the by-source update form accepts a literal replacement. Matched-only update/delete clauses may additionally use `source.id = 'literal'` or `source.id IN (...)` to select a bounded subset; the update-or-insert form also accepts that bounded matched filter while still inserting absent source rows; insert-only `WHEN NOT MATCHED` clauses accept the same bounded source filter; with a declared target alias, the equivalent `target.id = 'literal'` or `target.id IN (...)` filter is accepted; excluded source rows are no-ops. All forms lower to owner-checked revision/CAS paths; by-source forms cap expanded affected rows at 1,000; SQL Server's explicit `WHEN NOT MATCHED BY TARGET` spelling is accepted as the same bounded clause. The update-or-insert, matched-update-only, and unmatched-insert-only forms accept `OUTPUT inserted.id|revision|document_json`, `OUTPUT inserted.*`, and bounded `OUTPUT $action`; matched-delete-only, delete-or-insert, and bounded not-matched-by-source delete forms accept `OUTPUT deleted.id|revision|document_json`; mixed by-source action batches also accept `OUTPUT inserted.id|revision|document_json`. Deleted documents are captured from the pinned pre-delete snapshot, including staged replacements. Delete-or-insert `inserted` output returns only inserted/update actions and `deleted` output returns only deleted/update actions; no-op source rows produce no output row. Mixed pseudo-table projections reject `MERGE OUTPUT`. General search predicates, other target-column predicates, and arbitrary assignments remain rejected. See [MERGE qualification](../qualification/track2-native-dml-merge-2026-10-28.md), [MERGE OUTPUT qualification](../qualification/track2-tsql-merge-output-2026-11-03.md), [predicate qualification](../qualification/track2-native-dml-merge-predicates-2026-11-03.md), [filtered update-or-insert qualification](../qualification/track2-tsql-merge-filtered-update-insert-2026-11-04.md), and [by-source update qualification](../qualification/track2-native-dml-merge-not-matched-by-source-update-2026-11-03.md) |

The bounded `MERGE` surface also qualifies a literal source-id filter on a
matched `DELETE` paired with `WHEN NOT MATCHED THEN INSERT`; selected existing
rows delete, excluded rows no-op, and absent source rows insert through the
same pinned revision/CAS path. Its `OUTPUT deleted.*` and bounded `OUTPUT
$action` projections are also qualified. See [filtered delete-or-insert
qualification](../qualification/track2-tsql-merge-filtered-delete-insert-2026-11-04.md).
| `COMMIT` / `COMMIT TRAN` / `COMMIT TRANSACTION` | Publishes a prepared immutable catalog snapshot through the configured product store |
| `ROLLBACK` / `ROLLBACK TRAN` / `ROLLBACK TRANSACTION` | Discards the session without publishing its staged changes |
| `SAVEPOINT name` | Captures the staged product write set inside the transaction |
| `ROLLBACK TO SAVEPOINT name` | Restores the staged write set while retaining the savepoint |
| `RELEASE SAVEPOINT name` | Removes a savepoint without changing staged writes |

Each request carries one statement and a transaction ID after `BEGIN`. Product
operations are authenticated, owner-isolated, revision-checked, and limited to
the supported product families. The `kaveon.product.typed_rows` facade also
supports revisioned typed-row create/update/delete with owner isolation,
replay-safe `ON CONFLICT DO NOTHING`, and bounded
`ON CONFLICT (id) DO UPDATE SET document_json = excluded.document_json` upserts
that preserve the declared table identity. A bounded literal `document_json`
assignment (including `CAST(literal AS JSON)`) is accepted on the conflict path;
new rows still use the `VALUES` document. Arbitrary typed schemas and
assignments remain outside this contract. Multi-row `VALUES` are expanded into bounded
typed commands and committed atomically. Unbounded or cross-kind source-backed
`INSERT ... SELECT`, general `ON CONFLICT` actions, transaction
modifiers, arbitrary user tables, and lake-table DML are rejected explicitly.
The bounded same-kind `UPDATE ... FROM` form also accepts
`SET target.document_json = CAST(source.document_json AS JSON)` when its
existing literal revision/source-id or identity-join boundary is satisfied;
the cast is syntax-only and arbitrary source expressions remain rejected. See
[UPDATE FROM qualification](../qualification/track2-native-dml-update-from-2026-11-03.md).
The bounded source-backed `INSERT ... SELECT` form likewise accepts
`CAST(document_json AS JSON)` for its direct document projection and retains
the same source snapshot and conflict/CAS behavior; arbitrary projections
remain rejected. See [source-copy qualification](../qualification/track2-native-dml-insert-select-source-2026-10-28.md).
The qualified `MERGE` form is intentionally narrow: one target product table
with an optional simple alias, one source `VALUES` batch of at most 1,000 rows aliased as
`(id, document_json)`, an equality
join on the target qualifier's `id` and `source.id`, and one of the qualified
matched-update (from the source document or one string literal)/not-matched-insert, matched-delete/not-matched-insert,
matched-update-only, matched-delete-only, or unmatched-insert-only forms.
For an unaliased target, the base table name is the only accepted qualifier.
Matched updates and not-matched inserts may also use the direct typed source assignment
`CAST(source.document_json AS JSON)`; this is syntax-only and does not permit
arbitrary JSON expressions or non-direct casts. Evidence is recorded in
`../qualification/track2-native-dml-merge-json-typed-source-2026-11-03.md`.
Source scans, non-id predicates, arbitrary assignments, and other `MERGE` clause
orderings remain rejected. `OUTPUT inserted.id`, `OUTPUT inserted.revision`,
`OUTPUT inserted.document_json`, and `OUTPUT inserted.*` are accepted on the
update-or-insert, delete-or-insert, matched-update-only, and unmatched-insert-only
forms. Bounded `OUTPUT $action` is also accepted for a resolved action label.
Matched-delete-only and delete-or-insert forms accept `OUTPUT deleted.id`,
`OUTPUT deleted.revision`, and `OUTPUT deleted.document_json`; bounded
not-matched-by-source deletes accept the same deleted projection, while mixed
by-source action batches accept the corresponding inserted projection. Deleted
documents are captured from the pinned pre-delete snapshot, including staged
replacements, and delete-or-insert output is filtered by action from the ordered
staged write set. Mixed pseudo-table projections reject `OUTPUT` before staging.
Product `RETURNING id, revision`, `RETURNING *` (the stable
`id`/`revision`/`document_json` contract), and staged `document_json` projections on
creates, updates, upserts, and deletes are evaluated from the revision/CAS
write set. Product inserts
also support the bounded `ON CONFLICT DO NOTHING`
form, with an optional conflict target of `id`; a duplicate is a transaction-
local no-op that can still be committed. `RETURNING id, revision, document_json`
on this form returns the inserted row and returns an empty result for a
duplicate. They also support the bounded
`ON CONFLICT (id) DO UPDATE SET document_json = excluded.document_json` form,
or the bounded literal/`CAST(literal AS JSON)` conflict assignment; an existing
owned record is updated with revision CAS, while a missing record is created.
The conventional `<product_table>_pkey` named target is accepted as the same
identity target; other named constraints, conflict predicates, arbitrary
assignments, and arbitrary conflict targets remain rejected. Literal conflict
assignments are limited to `VALUES` inserts, not source-backed `INSERT ... SELECT`.
Statement execution and the
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

`POST /v1/statement` returns synchronously and materializes the root result before JSON serialization unless the client requests paged delivery. A statement is admitted through the memory admission queue, bound against the catalog, and, where the stage planner can express the shape, run as dependency-gated stages with versioned worker fragments and streamed Arrow IPC exchanges; otherwise the coordinator runs it and records why (`execution.detail`). A statement whose normalized text, catalog snapshot, pinned Delta versions and time zone match a cached result is served from the coordinator's result cache unless the request sets `settings.result_cache = false`. Query IDs identify retained records; cancellation removes a queued statement or propagates to active worker tasks; history remains process-local. Native `ANALYZE` returns the qualified table and exact row count; capability discovery reports it only when the coordinator has a durable product-catalog authority configured. Coverage records: `../qualification/tpch/coverage.md` (22 of 22) and `../qualification/clickbench-2026-09-16.md`.

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
