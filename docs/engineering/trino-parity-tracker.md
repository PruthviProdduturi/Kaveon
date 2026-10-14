# Kaveon–Trino parity tracker

This is the working tracker for taking Kaveon from a strong integrated analytics
product to a broadly capable distributed SQL engine. It is a capability tracker,
not a marketing score. A row moves to **Qualified** only when the stated test
passes against an exact-result reference and the evidence is archived.

The target is **9/10 broad federated analytics capability**: production-grade
distributed SQL, connector pushdown, optimizer behavior, recovery, and
operations across a declared workload. It does not mean that every Trino
connector or SQL extension must be copied.

## Status vocabulary

- **Implemented** — code exists and focused tests pass.
- **Partial** — a slice exists, but a required boundary is unqualified.
- **Missing** — no usable implementation exists.
- **Qualified** — deployed evidence passes the acceptance gate.

## Parallel workstreams

| ID | Workstream | Current status | Next acceptance gate |
|---|---|---|---|
| 1 | Distributed SQL execution | Partial — initial implementation slices complete | Three-worker exact-result corpus covering joins, aggregates, windows, sort, TopN, set operations, worker loss, cancellation, and snapshot pinning |
| 2 | SQL compatibility and types | Partial — initial implementation slices complete | Differential corpus against Trino/DuckDB covering NULLs, casts, arrays/structs/maps, CTEs, windows, subqueries, set operations, and explicit unsupported errors |
| 3 | Federation connectors and pushdown | Partial | Connector matrix with authenticated reads, predicate/projection/limit pushdown, partition pruning, retries, cancellation, and secret isolation |
| 4 | Statistics and cost-based optimization | Partial | Exact Parquet/Delta/Iceberg statistics, join-order and broadcast decisions, runtime filters, and plan-regression evidence |
| 5 | Memory, spill, and admission | Partial | Concurrent join/aggregate/sort pressure, bounded reservations, spill cleanup, quotas, cancellation, and no silent coordinator fallback |
| 6 | Recovery and operations | Partial | Coordinator/worker restart, exchange loss, retries, upgrade/rollback, backup/restore, and sustained mixed-workload soak |
| 7 | Benchmark and publication gate | Partial | Matched ClickBench, TPC-H/TPC-DS, federation, and Kaveon workloads with hashes, p50/p95/p99, errors, resource use, and cost |

Workstreams **1** and **2** begin in parallel. Workstreams **3**, **4**, and
**7** are being audited in parallel so implementation work is measured against
the same acceptance contract rather than separate informal benchmarks.

## Audit snapshot — October 13, 2026

The local parity baseline was rerun on October 13 with focused qualification
changes through commit `3e741ce9`. The checkout also contained separate, uncommitted
engine work from the parallel optimization lane, so these numbers are a local
regression snapshot and are not a release-image claim:
`cargo test -p kaveon-server --bin kaveon-server` passed **364/364** tests,
`cargo test -p kaveon-sql --lib` passed **134/134** tests, and
`cargo test -p kaveon-optim --lib` passed **48/48** tests,
`cargo test -p kaveon-exec --lib` passed **294/294** non-ignored tests with four
benchmark tests intentionally ignored. Documentation validation also passed for 146 Markdown
files, 39 routes, and 9 SVGs. This is an in-process regression baseline; it
does not replace the deployed three-worker qualification gates listed below.

The complete `cargo test --workspace` run also passed on this tree, including
catalog, CLI, core, executor, optimizer, server, SQL, and storage crates plus
all doctests. The executor result is **294 passed, 4 ignored**; the ignored
cases are explicitly marked release benchmarks. This workspace result is
repository-wide regression evidence, not a substitute for the deployed
three-worker and connector qualification gates.

The Python qualification unit suite also passes **37/37** tests. The local
pressure runner now authenticates its disposable bootstrap catalogs as an
administrator, matching the documented grant model. Its latest rebuilt profile
passed all ten cases, including exact grouped completion, skew handling,
cancellation cleanup, and deliberate window, oversized-string, and disk-quota
rejections. Forced distributed aggregate spill and cleanup remain covered by the
server differential test
`a_distributed_grouped_aggregate_under_spill_matches_the_local_result`; the
AKS release SLO still requires separate image-bound fault-pressure and
comparison artifacts.

The server differential corpus currently passes **12/12** tests, including
forced distributed aggregate spill with exact-result comparison and zero
retained spill bytes, join spill, residual and outer-join modes, filtered
aggregates, and correlated `EXISTS` shapes. This is package-level evidence for
those declared cases; it does not replace the deployed image-bound corpus.

Temporal `ARRAY[...]` literals now preserve DATE32, DATE64, TIME32 second and
millisecond, and TIMESTAMP microsecond element types, including null elements;
the execution suite includes a timestamp-list regression. `ARRAY_CONCAT` also
preserves those temporal element types, as do `ARRAY_APPEND`,
`ARRAY_PREPEND`, `ARRAY_REMOVE`, `ARRAY_MIN`, `ARRAY_MAX`, and
`ARRAY_REVERSE`, `ARRAY_SORT` (with nulls last), and `ARRAY_SLICE` with
positive or negative stride. `ARRAY_POSITION` and `ARRAY_DISTINCT` also
preserve temporal values and element types.
`CONTAINS` and `ARRAYS_OVERLAP` now do as well. Other timestamp units and
temporal array operations remain outside this slice.

`ARRAY_REPEAT(element, count)` now returns a typed list for numeric, text,
boolean, date, and time scalars used by the Trino compatibility corpus. Counts are validated per row,
NULL counts produce a NULL array, NULL elements remain NULL, and counts are
bounded at one million elements to prevent an accidental query from consuming
unbounded execution memory. This is a bounded function-compatibility slice;
complex element types and the full Trino array-function surface remain open.

`ARRAY_UNION`, `ARRAY_INTERSECT`, and `ARRAY_EXCEPT` now operate on matching
typed lists, preserve first-seen order, remove duplicates, and treat NULL as a
single comparable element. Both 32-bit and 64-bit list offsets are accepted;
mixed offset widths and complex nested values remain explicit boundaries.

`ARRAY_CONCAT` now accepts two or more matching typed lists, including the
64-bit-offset representation, while retaining explicit null-array semantics.

`FLATTEN` now concatenates nested typed arrays for both list offset widths. An
outer NULL remains NULL, and a NULL nested array fails closed instead of being
silently treated as an empty array.

`TRIM_ARRAY` now removes a per-row non-negative suffix count from typed lists,
preserving list offsets and NULL arrays while rejecting negative counts.

`ARRAY_FIRST` and `ARRAY_LAST` now return the typed first or last element,
with NULL for empty or NULL arrays, for both list offset widths.

`ZIP` now builds typed row arrays from two or more lists and pads shorter input
arrays with NULL fields, preserving the outer list's offset width.

`SEQUENCE` now builds bounded BIGINT arrays with inferred ascending or
descending steps or an explicit step, rejecting zero steps, overflow, and
unbounded output.

The statistics planner now preserves exact row and byte estimates through
row-preserving projected and sorted subqueries, plus bounded `LIMIT` and
`OFFSET` cardinality changes. Simple column forwarding, aliases, ordering,
and bounded pagination can therefore still qualify a small side for a
broadcast join; computed projections and other cardinality-changing operators
remain conservative and use the existing partitioned fallback.

The initial repository audit confirms that the Engine has real distributed
foundations, but several operators are still centralized or narrow:

The current immutable AKS image has now passed the bounded worker-loss,
retry, cancellation, and concurrent pressure gate. The evidence is
`docs/qualification/engine-aks-fault-pressure-2026-10-14.md`; the report
exercised three compatible workers, exact result hashes, catalog-snapshot
pinning, replacement-worker recovery, and cleanup checks. This closes the
bounded qualification slice for those scenarios. Coordinator failover,
node-drain recovery, and sustained soak remain separate acceptance gates.

- Hash repartition, broadcast, joins, partial/final aggregates, spill, retry,
  cancellation, and snapshot fields exist. Cluster qualification remains open
  for skew, spill pressure, rolling worker loss, and cancellation stress.
- Exchange producers stream output, and HTTP consumers now pump each response
  into a bounded replayable append-only spool while Arrow decodes batches
  incrementally. The old second contiguous payload copy is gone; network
  backpressure is covered by a bounded-lane regression test (`8fe8fafb`), but
  pressure qualification and sustained federated-exchange evidence remain
  open.
- Windows gather to one task. Final sort/TopN still have centralized merge
  stages. `UNION ALL` now round-robins its inputs; set operations with matching projected keys use hash exchange;
  identity-ambiguous inputs retain a centralized merge. Compatible windows with a shared non-empty
  `PARTITION BY` are now hash-partitioned across workers; unpartitioned and
  mixed-key windows remain centralized for correctness. These are not yet
  Trino-class parallel plans across the full window corpus.
- Static predicates are pushed into eligible build scans, including broadcast
  joins; the build-side key set is not yet propagated to the probe scan at
  runtime. A bounded in-memory inner/semi-join filter now removes probe batches
  whose keys are absent from the build before hash probing; it is disabled for
  outer/anti/residual joins and spilled builds. Scan-level dynamic filtering,
  join reordering, skew mitigation, and qualified coordinator failover remain
  open.
- Ordinary distributed joins use equality keys; inner residual predicates now
  lower to a correctness-first cross join plus filter, while equality-key
  residual predicates on outer joins are evaluated inside the hash join before
  null extension. Residual-only outer joins use an explicit broadcast path.
  Semi/anti joins now hash-partition both inputs on
  ordinary column keys, including positional keys projected by correlated
  EXISTS plans, and retain a deliberate broadcast fallback only when a key
  cannot be resolved. The distributed fragment executor now emits only the
  matching (semi) or non-matching (anti) left rows from the partitioned hash
  join, preserving the left schema; runtime build-key propagation and adaptive
  broadcast selection remain open.
- Non-recursive CTEs, derived tables, joins, aggregates, `HAVING`, `CASE`, and
  common set operations work. Uncorrelated aggregate scalar subqueries in
  `WHERE`, the qualified correlated scalar aggregate filter shape, and bounded
  projection scalar subqueries are supported when they return one ungrouped,
  single-column aggregate. Correlated non-aggregate, multi-row, and other
  scalar forms, recursive CTEs, the full prepared-statement protocol, and
  general lake-table DML remain missing. `INTERSECT ALL` and `EXCEPT ALL`
  now preserve multiplicity through local and distributed execution.
  Typed positional `$1`, sequential `?`, and named `:name` parameters are now
  validated and bound through the HTTP statement API; the full prepare/describe/
  bind/execute protocol remains open.
  A `/v1/trino/statement` adapter now accepts raw SQL with standard
  `X-Trino-*` session headers (including URL-decoded `X-Trino-Session`
  properties), recognizes one or more encoded entries in the
  `X-Trino-Prepared-Statement` SQL header, selects the named entry for
  `EXECUTE name USING`, binds scalar values (including typed `DATE`,
  `TIMESTAMP`, `TIME`, and `DECIMAL` literals) through the typed binder, and emits
  `nextUri`/`stats`-shaped
  responses, including standard `infoUri` and `partialCancelUri` links for
  query inspection and cancellation. Malformed prepared headers now fail closed with a structured
  `INVALID_PREPARED_STATEMENT` response rather than silently falling back to
  the request body. JSON array parameters through the HTTP statement API now
  lower to typed `ARRAY[...]` expressions, including nested arrays and null
  elements. String-keyed JSON objects and Trino `MAP(ARRAY[…], ARRAY[…])`
  literals now lower to typed `MAP` expressions through the same recursive
  binder; malformed lengths, keys, and duplicate keys fail closed. The full
  prepare/describe/bind wire protocol remains open. Trino `EXECUTE ... USING
  ARRAY[...]` literals now use the same recursive binding path, including nested
  arrays and null elements.
  SQL `DEALLOCATE PREPARE ALL` now clears every prepared statement owned by the
  requesting principal while preserving other principals' entries.
  The raw Trino route also answers `SHOW SESSION` with the supported Engine
  settings in Trino's metadata result shape, and answers `SHOW FUNCTIONS` with
  the supported scalar and aggregate registry in Trino's six-column metadata
  shape, including `LIKE` wildcard filtering; these metadata calls do not
  create a persistent server-side session.
  `EXPLAIN` now unwraps the plan-only envelope, runs the normal binder and
  optimizer without starting scans or worker tasks, and returns a Trino-shaped
  `Query Plan` column for logical, optimized, and distributed/physical views.
  `TYPE IO` uses the physical-plan projection because runtime IO statistics
  are not available before execution. `EXPLAIN ANALYZE` now executes once and
  returns the optimized plan with measured output rows and analysis/planning/
  execution timings; unsupported output formats fail explicitly.
  `LIMIT ALL` is also accepted as Trino's explicit no-limit pagination form.
  Prepared-parameter discovery now also ignores PostgreSQL `::type` casts,
  so cast names cannot be mistaken for named bindings (`617b69f6`).
  Uncorrelated aggregate scalar subqueries in `WHERE` are now supported with
  exact-result differential coverage; unsupported correlated and multi-row
  scalar forms remain explicitly rejected while bounded projection forms are
  supported. Equality-correlated `COUNT` scalar aggregates now use a left
  join and zero-fill unmatched keys, with local and distributed differential
  coverage.
  Equality-correlated `IN`/`NOT IN` subqueries whose projected value is the
  correlation key now lower to keyed semi/anti joins with owner-side NULL
  handling. A single equality key plus residual predicate now uses the same
  residual-aware semi/anti path as `EXISTS`; multi-key and deeper correlations
  remain explicitly rejected.
  `CAST(... AS DATE)` is now supported with nullable `Date32` execution and
  focused parser/executor coverage. `CAST(... AS TIMESTAMP)` and
  `TIMESTAMP WITH TIME ZONE` now map to Arrow microsecond timestamps with
  nullable execution coverage. `MIN` and `MAX` over timestamp columns now
  preserve the input unit/timezone and exact integer values, including grouped
  and null-only groups (`178c3bdb`). `DATE_TRUNC` now preserves nulls and
  executes over DATE calendar units plus second, millisecond, microsecond, and
  nanosecond timestamp columns while preserving the source unit and timezone;
  sub-day units on DATE values fail with an explicit diagnostic. Day and week intervals
  now use a unit-aware runtime path for typed DATE and timestamp columns, and
  hour/minute/second intervals use a unit-aware runtime path for timestamps, while
  DATE literals retain exact day-number lowering. Month and year intervals now
  use a runtime calendar path for typed DATE/timestamp columns, clamping source
  month-end days to the target month while preserving timestamp units. Typed
  `TIMESTAMP` literals lower through the existing Arrow timestamp cast path.
- `DATE_DIFF` now returns exact BIGINT differences for DATE and all supported
  timestamp units from nanoseconds through weeks and complete calendar
  month/quarter/year boundaries, preserving NULL inputs and reverse-range
  signs. Leap-year month ends are covered by executor and distributed tests.
- Background statistics and cube refresh deduplication now scopes its
  process-local registry by table and source location. Independent embedded
  catalog stores can no longer suppress one another's refresh, which removes a
  parallel-test and multi-store stale-statistics race.
- The cube row-path differential now runs every grouping through the declared
  three-worker execution fixture, comparing exact row/additive measures and
  distinct sketches with the coordinator-built cube. This qualifies the
  statistics answer path at the current deployed worker width; cloud fault and
  larger-capacity evidence remain separate gates.
- Cube grouping children now divide the parent memory budget across their
  admission width before dispatch, preventing a cube build from exhausting
  the pool it is holding and waiting on itself. The boundary is covered by
  `cube_child_memory_budget_is_shared_across_concurrent_groupings`; one-scan
  cube construction remains open.
- The analytical coordinator result cache is now explicitly qualified: it
  normalizes statement text, keys entries by catalog/schema snapshot, pinned
  Delta versions, and time zone, applies byte/TTL/LRU bounds, excludes paged
  results, and supports an admin clear endpoint. `result_cache = false` is a
  per-statement bypass. KaveonDB transactional state is not served from this
  cache; revision/CAS visibility remains authoritative after writes.
- Numeric, text, and boolean array literals are supported as null-preserving
  typed Arrow lists. `CARDINALITY`, `CONTAINS`, `ARRAY_POSITION`,
  positive/negative-stride slices, and `ELEMENT_AT` cover those list types;
  `ROW(...)` produces typed Arrow structs with field projection, equality, and
  lexicographic ordering; `MAP(...)` builds Arrow maps with key lookup,
  order-independent equality, and typed key/value projections. Ordered map
  comparison remains open.
  Homogeneous integer array literals preserve `INT64` element types across
  `CONTAINS`, `ARRAY_POSITION`, `ARRAY_DISTINCT`, `ARRAY_SORT`,
  `ARRAY_REVERSE`, `ARRAY_SLICE`, and `ELEMENT_AT`; mixed integer/float arrays
  retain the common `Float64` representation. Homogeneous decimal arrays now
  preserve their precision and scale through `ELEMENT_AT`, `CONTAINS`,
  `ARRAY_POSITION`, `ARRAY_DISTINCT`, `ARRAY_SORT`, `ARRAY_REVERSE`, and
  `ARRAY_SLICE`; `ARRAY_CONCAT` now joins typed lists while preserving null
  elements; `ARRAY_APPEND` and `ARRAY_PREPEND` add typed values while
  preserving list nullability, including nested list, row, and map elements;
  `ARRAY_REMOVE` removes matching values without
  dropping non-matching nulls, including nested list, row, and map elements;
  and `ARRAY_MIN` and `ARRAY_MAX` return typed extrema while
  ignoring null elements, and `ARRAYS_OVERLAP` follows Trino's true/false/NULL
  intersection semantics. `ARRAY_JOIN` now formats supported typed elements,
  omits NULL elements by default, and accepts a NULL replacement policy.
  Nested arrays and arrays of typed `ROW`/map values now materialize as typed
  Arrow containers with element nulls preserved; mixed or unsupported element
  types remain explicit boundaries. `ARRAY_DISTINCT` now deduplicates nested
  list, row, and map elements by typed equality while preserving order and one
  NULL element. `ARRAY_CONCAT` also preserves nested list, row, and map
  elements through a generic Arrow child-array path; `CONTAINS_KEY` now checks
  typed map keys with Trino-compatible NULL propagation; other combinations remain
  explicit boundaries.
  Scalar, DATE, timestamp, and DECIMAL casts are supported; complex cast
  targets fail with typed planner diagnostics. `value IN UNNEST(array)` and
  its `NOT IN` form lower to the existing nullable `CONTAINS` path; row-
  producing `UNNEST` in `FROM` is rejected with an actionable diagnostic
  (rather than being misread as a table named `UNNEST`); dereference and other
  array operations remain open. Equality quantified predicates `= ANY(array)` and
  `<> ALL(array)` use the exact membership path; ordered and not-equal quantified
  predicates now lower to nullable `ARRAY_ANY_*`/`ARRAY_ALL_*` reductions with
  Trino's empty-array semantics. Unsupported element types still fail explicitly.
  `JSON_EXTRACT` now preserves objects, arrays,
  and JSON nulls as serialized JSON text; `JSON_EXTRACT_SCALAR` supports dotted
  object paths and numeric array subscripts. Invalid JSON and missing paths
  return NULL; `JSON_ARRAY_LENGTH` returns nullable lengths for JSON arrays and
  `JSON_SIZE` returns container sizes (or zero for scalar values).
  `REGEXP_LIKE` now reuses the bounded compiled-regex cache and
  preserves nullable boolean results. `MAP_CONCAT` now merges supported typed
  maps with later values winning duplicate keys, and `MAP_ENTRIES` exposes the
  typed key/value rows as an array.
  HTTP and embedded-CLI result materialization now preserves supported nested
  Arrow lists, structs, and UTF-8-keyed maps as JSON arrays/objects rather than
  Arrow debug text; focused server coverage protects the wire shape. Nested
  temporal/binary values and non-UTF-8 map keys remain outside this slice.
  Result column metadata uses the corresponding client-facing `List`, `Row`,
  and `Map(key, value)` spellings instead of Arrow's internal field dump.
- `QUALIFY` now has an exact-result distributed differential case using
  partitioned `ROW_NUMBER()` output. This qualifies the implemented
  row-number filter shape only; window `NULLS FIRST`/`NULLS LAST` ordering is
  preserved through planning and execution, including a nullable partitioned
  ordering case, while other window modifiers remain subject to the broader
  corpus.
- Window aggregate `FILTER` clauses now lower to the same null-preserving
  `CASE` representation used by grouped aggregates. Window peer and partition
  comparisons decode dictionary-backed text values once per evaluated column,
  so Parquet files with different local dictionary codes produce identical
  local and distributed results. The differential sweep includes this case
  across plain, dictionary, clustered, and Hive-partitioned layouts. Window
  `COUNT/SUM/AVG/MIN/MAX(DISTINCT ...)` uses the canonical hash key path and
  caches repeated frame bounds per partition; its exact result is now covered
  by the same multi-encoding local/distributed differential sweep. Arbitrary
  ordered frames can still have one distinct-set build per unique frame, so
  this optimization does not change their semantics.
- Hash and semi/anti joins now accept `DATE` (`Date32`) keys with the same
  exact day-number semantics as their integer representation. A correlated
  scalar aggregate whose correlation includes a date column now passes the
  Parquet-encoding and local/distributed differential sweep; the focused
  join suites pass as well. This closes the date-key slice without claiming
  support for other unqualified complex join-key types.
- Timestamp join keys now use their exact Arrow integer representation for all
  four time units in hash and semi/anti joins. Duplicate and NULL behavior is
  covered by an execution test; timestamp joins still require matching Arrow
  units/timezones through the existing schema check.
- `Date64`, `TIME32`, and `TIME64` keys now use their exact Arrow integer
  representation in hash and semi/anti joins. Date and millisecond-time
  duplicate/NULL behavior is covered by an execution test; joins still require
  identical Arrow logical types through the existing schema check.
- Regular hash joins now support `DECIMAL(precision, scale)` keys using the
  exact unscaled integer plus scale, matching the existing semi/anti-join
  representation. Duplicate and NULL behavior is covered by execution tests.
  `DECIMAL256` keys now retain Arrow's native `i256` value through regular and
  semi/anti joins, with exact-value and NULL coverage; decimal scale and
  precision still must match through the existing schema check.
- Top-level `ORDER BY` now carries explicit `NULLS FIRST`/`NULLS LAST` through
  logical planning, binding, fragment sort specifications, and local/remote
  execution. Omitted placement keeps Trino's defaults (NULLS LAST ascending,
  NULLS FIRST descending), with parser-level exact coverage for explicit and
  default forms.
- There is no Trino-style cross-source federation yet. Registered SQL sources
  are API passthrough paths, while the strongest native Engine path is
  Parquet/Delta/Iceberg. The connector matrix now records S3 as implemented
  but unqualified and Iceberg as locally qualified but not yet qualified on
  ADLS/AKS; federation remains an open workstream.
- The October 7 AKS comparison is valid for its declared 12-query fixture
  (3.8282 versus 1.8012 successful exact-result QPS, 2.1254x), but it is not
  broad Trino parity. TPC-H SF100 and repeated ClickBench qualification remain
  open.

The detailed source audit is recorded in the Engine status and connector
qualification documents; this tracker remains the single ordered work list.

## Workstream 1 — distributed SQL execution

| Capability | Status | Evidence required |
|---|---|---|
| Distributed scans and exchange | Partial | Multi-worker exact results and exchange byte/copy counters |
| Repartitioned hash joins | Partial | Correctness under skew, spill, retry, and worker loss |
| Broadcast joins from exact source statistics | Partial | Small-side decision, stale-statistics fallback, and AKS result/latency comparison |
| Global and grouped aggregates | Partial | Associative merge correctness, bounded memory, spill and cancellation |
| Analytical result cache | **Qualified on AKS for the declared row-result workload**; shared invalidation remains open | `docs/qualification/cache-slo-aks-2026-10-13.json`: exact hit parity, 20/20 warm hits, p50/p95/p99/max latency, hit metrics, bypass, clear, authorization scope, and coordinator restart-to-miss |
| Windows, sort, TopN, set operations | Partial | Distributed result hashes and null/order semantics |
| Snapshot pinning | Partial | Catalog mutation during an in-flight query never mixes generations |
| Worker loss and exchange recovery | Partial | Mid-stage loss with no missing or double-counted partitions |
| Distributed execution SLO | Partial | A merged qualification artifact proving exact-result, reliability, latency, recovery, cancellation, and bounded-resource gates |

## Workstream 2 — SQL compatibility and types

| Capability | Status | Evidence required |
|---|---|---|
| Joins, filters, grouping, ordering | Partial | Differential corpus with typed result hashes |
| CTEs and subqueries | Partial | Nested and correlated cases, including empty and NULL inputs |
| Windows and set operators | Partial | `OVER`, frames, `UNION`, `INTERSECT`, and `EXCEPT` corpus |
| Arrays, maps, structs, JSON | Partial — JSON/JSONB casts now use the Engine's UTF-8 JSON representation | Parse, plan, execute, and explicit unsupported diagnostics; broader typed JSON semantics remain to be qualified |
| Decimal, timestamp, interval, and casts | Partial | Overflow, timezone, precision, and implicit-cast cases |
| `CASE`, `HAVING`, `FILTER`, `QUALIFY` | Partial | Exact-result and error semantics |
| Native DML and prepared parameters | Partial | `BEGIN`/`COMMIT`/`ROLLBACK`, parameter binding, and transactional visibility |

### Cache boundary and latency target

Kaveon has a bounded analytical result cache. It normalizes statement text and
keys a completed result by the catalog/schema snapshot, pinned Delta versions,
and time zone. Entries are coordinator-local, byte-bounded, TTL-bounded, and
LRU-evicted; paged responses are excluded. `result_cache = false` bypasses it,
and administrators can clear it. This is the safe equivalent of a repeated
analytical-result fast path, not a replacement for storage or transaction
semantics.

KaveonDB transactional records are deliberately never served from that cache.
Revision/CAS visibility, commits, replay, and read-your-write behavior remain
authoritative in the transactional store; writes invalidate affected analytical
entries before the new state becomes visible. A shared cross-coordinator cache
is not claimed yet. Track 1 therefore treats the cache as implemented locally
and keeps distributed invalidation, restart warm-up, and deployed hit/miss
latency evidence as the remaining qualification work.

#### Track 1 cache SLO

The cache is qualified only when one deployed, same-image evidence run proves
all of the following:

- a warm analytical hit returns the exact same columns, rows, ordering, and
  result hash as an uncached execution;
- hit latency is recorded separately from planning and execution, with p50,
  p95, p99, and max, and the evidence reports hit ratio plus byte and entry
  evictions. The Engine now also exports coordinator-side hit sample count,
  total nanoseconds, and maximum lookup nanoseconds; percentile publication
  remains a qualification-run responsibility;
- the cache is bounded by its byte budget and TTL, and eviction or expiry
  never changes the result or causes an unbounded memory or disk allocation;
- catalog publish, Delta-version change, and committed KaveonDB product
  transactions invalidate affected analytical entries before a subsequent
  read can observe the new snapshot;
- `result_cache = false` produces no hit and inserts no entry, paged results
  remain uncached, and an administrator can clear the cache;
- cache keys remain isolated by catalog, schema, snapshot/version, timezone,
  and authorization view, with no cross-principal result exposure; the
  implementation now includes the effective role and catalog-grant scope,
  so grant changes cannot reuse the previous authorization view; and
- a coordinator restart fails closed to a miss and rebuilds safely. A shared
  cross-coordinator cache, restart warm-up, and cache-backed transactional
  reads are outside the current claim until separately implemented and
  qualified.

The deployed AKS qualification passed for the declared row-result workload on
image `sha256:9c0821abf24cf5646de44c0009a8f83b85f752933928e574ce7e8d4741b60f3d`.
It does not claim shared cross-coordinator invalidation, TTL-expiry evidence,
or cache-backed transactional reads; those remain separate gates.

This is a Track 1 execution SLO, not a claim that Trino has a built-in result
cache. Trino deployments may add connector caches or materialized views, but
the Trino engine itself does not provide this general coordinator result-cache
contract. The cache SLO is an independent Track 1 gate: the distributed
execution SLO cannot be marked complete merely because cache unit tests pass,
and the cache SLO cannot be marked complete from a latency sample without the
invalidation, isolation, bounds, and restart evidence listed above.

### Completed implementation slices in the current cycle

- Exchange decoding no longer assembles a second contiguous in-memory payload
  before Arrow decoding (`8bfb76fa`). The HTTP exchange path now feeds a
  bounded replayable append-only spool before Arrow consumption.
- Windows with compatible shared partition keys are hash-partitioned across
  workers (`06e1fb80`). Unpartitioned and mixed-key windows stay centralized.
- Differential coverage now includes `INTERSECT`, `EXCEPT`, and non-recursive
  CTEs (`59aaf0ec`). `CAST(... AS DATE)` now maps to Arrow `Date32` and keeps
  nulls (`bf49f5cb`). Timestamp and timestamp-with-time-zone casts are covered
  in `52fa76c6`.
- Uncorrelated aggregate scalar subqueries in `WHERE`, plus actionable
  multi-row and multi-column diagnostics, are covered in `3a60cf23`.
- Unsupported placeholder forms fail with an explicit binding diagnostic rather
  than being misinterpreted as literals (`89499c7b`).
  Positional `$1`, `$2` binding safely converts typed AST literals without
  string interpolation (`74f66a10`), sequential `?` placeholders are bound in
  appearance order (`0a3fa2a4`), and the HTTP statement API routes validated
  scalar parameters before planning (`0ea777ba`). Named `:name` values now use
  the same typed binding path and fail closed when missing.
- A bounded replayable append-only exchange spool foundation is implemented
  with partial-read, fork/retry, cancellation, and writer-failure tests
  (`79ddf443`).
- Shared exchange spool byte quotas now have concurrent-pressure and retry
  cleanup coverage: an over-quota retry is refused, failed-attempt cleanup
  releases the reservation, and the retry can proceed (`70348885`). This
  qualifies the local quota lifecycle; sustained network-pressure and
  deployed-cluster evidence remain open.
- Exchange chunk writes now refund their quota reservation when the disk write
  itself fails, preventing a transient spool I/O error from poisoning query
  capacity; the failure-injection test verifies zero residual bytes and query
  reservations (`a12e7e76`).
- The spool now exposes an incremental Arrow IPC reader with partial-write and
  later-batch coverage (`13f27d74`); HTTP exchange fetches use it through
  `5dd9d275`.
- Task telemetry now carries exchange input/output bytes, decoded bytes,
  partition hash/copy time, copy allocations, and copied bytes through the
  response header; API coverage verifies those counters survive serialization.
- Decimal casts now validate precision/scale before planning and execute with
  Decimal128 null/scale coverage (`81b610e2`).
- Numeric `ARRAY[...]` literals now execute as null-preserving Arrow
  `List<Float64>` values with parser and executor coverage (`b1fe824f`).
- Numeric, text, and boolean `ARRAY[...]` literals now execute as
  null-preserving typed Arrow lists; `CONTAINS`, `ARRAY_POSITION`, and slices
  support all three element types with SQL three-valued null semantics
  (`b2054d10` plus the follow-up execution coverage).
- `CARDINALITY(ARRAY[...])` now parses and evaluates to a nullable `Int64`
  list length, with explicit rejection for non-list values (`1c0e29e5`).
- `CONTAINS(array, value)` now preserves nullable SQL boolean semantics for
  null arrays, search values, and elements across numeric, nested list, row,
  and map values, including `LargeList` containers, with planner and executor
  coverage (`b2054d10`). Unsupported element types retain explicit diagnostics.
- `ARRAYS_OVERLAP(left, right)` now applies the same nullable intersection
  semantics to nested list, row, and map elements using typed equality,
  including `LargeList` containers; a matching non-null element returns
  `true`, while an unresolved null match returns `NULL`. Unsupported element
  types retain explicit diagnostics.
- `ARRAY_POSITION(array, value)` now returns the first 1-based position, `0`
  when a value is absent, and matches null elements for numeric, text, nested
  list, row, and map values, including `LargeList` containers; null list
  inputs remain null. Unsupported list element types retain an explicit
  execution diagnostic, with parser and executor coverage in this slice.
- `ARRAY_DISTINCT` now preserves first occurrence order and one null for
  nested list, row, and map values in both `List` and `LargeList` containers.
- `ARRAY_REVERSE` now preserves nested element values, nulls, and container
  width for both `List` and `LargeList` inputs.
- `ARRAY_SORT` now supports the existing orderable scalar element set in
  `LargeList` containers with nulls last and stable container width.
- `ARRAY_SLICE` now preserves nested values, negative/positive strides, null
  elements, and `List`/`LargeList` container width through the same bound
  normalization path.
- `ARRAY_MIN` and `ARRAY_MAX` now support the existing scalar element set in
  `LargeList` containers, preserving null results for null or empty arrays.
- The differential corpus now exercises scalar `CARDINALITY`,
  `ARRAY_POSITION`, `ARRAY_JOIN`, and `ARRAYS_OVERLAP` results across alternate
  Parquet encodings and local/three-worker execution paths. This extends the
  complex-type parity gate without claiming support for row-producing `UNNEST`
  or array/map-valued result comparison.
- The same corpus now checks `MAP_ELEMENT_AT` and order-independent map
  equality on scalar results across those encodings and execution paths;
  nullable map values are now covered as well, preserving equality when both
  sides contain the same null entry. Ordered map comparisons and differential
  comparison of complex-valued result sets remain open; API materialization of
  supported map values is qualified separately below.
- Date-array construction, cardinality, positional lookup, reverse, and
  distinct operations are covered in the same differential paths. Timestamp
  and decimal arrays now have the same cardinality/positional coverage. The
  differential row canonicalizer now
  uses Arrow's type-aware formatter, so literal array/map-valued projections
  are compared across local and distributed paths instead of being silently
  excluded by an unsupported UTF-8 cast. Representative `ROW(...)`, arrays of
  rows/maps, and `MAP_KEYS(...)` nested projections now pass the same gate;
  mixed element types and complex-valued ordering remain open.
- Array equality and inequality now use null-preserving, element-wise
  semantics, including nested list values; ordered array comparisons now use
  lexicographic semantics with prefix handling and NULL propagation. The
  executor unit test and differential corpus cover equal, different, ordered,
  and NULL-containing arrays, including the SQL UNKNOWN result when equal
  arrays contain a null element.
- The same comparison path now normalizes Arrow 64-bit-offset `LargeList`
  arrays before evaluation, including mixed `List`/`LargeList` operands, so
  either storage representation uses the same lexicographic and null
  semantics; a focused executor regression covers both paths.
- Scalar list functions receive the same normalization, so `LargeList` columns
  can use the supported `CARDINALITY` and array-function paths when their
  offsets fit the 32-bit execution representation.
- `ARRAY[...]` now preserves Arrow `TIMESTAMP` values at second, millisecond,
  microsecond, and nanosecond resolution instead of rejecting the non-
  microsecond representations. The timestamp unit remains part of the list
  element type; timezone-bearing arrays still require the existing compatible
  timestamp path.
- `ARRAY[...]` now preserves unsigned `UInt32` and `UInt64` element columns,
  including nullable rows, instead of rejecting those Arrow integer types.
- `ARRAY[...]` now preserves nullable `Float32` element columns and keeps the
  list element type at `Float32` rather than rejecting or widening the source.
- `ARRAY[...]` now preserves nullable signed and unsigned `Int8`, `Int16`,
  `UInt8`, and `UInt16` element columns with their native list types.
- `ARRAY[...]` now preserves `Time64` microsecond and nanosecond element
  columns, retaining the source time unit in the list type.
- Equality-key `LEFT`, `RIGHT`, and `FULL` joins now carry residual `ON`
  predicates through local and distributed hash joins. Candidate pairs are
  evaluated in bounded batches with SQL TRUE-only matching; preserved rows and
  the right-side match bitmap are updated only after that evaluation. Residual-
  only outer joins use an explicit broadcast candidate path because they have no
  safe repartition key; this is correctness-qualified, not a scalability claim.
- Three-worker global `ORDER BY`/TopN centralized merge correctness is now
  qualified for NULL ordering and ties against a single-partition reference
  (`bb788709`). The input remains distributed, while the exact global merge
  is explicitly finalized by one task; a range exchange for parallel global
  ordering remains open (`7f32d854`).
- Inner equi-joins now propagate a safe literal predicate across the equality
  key in both join-condition orders. This lets exact source statistics see the
  reduced build side before distribution selection; the optimizer qualification
  covers the propagated filters and the existing no-statistics partitioned
  fallback (`0ba56846`, `04c5d391`).
- The same optimizer path now propagates bounded positive `IN (...)` literal
  filters across inner equality keys (up to 64 values), preserving a safe
  partition/build reduction without claiming runtime dynamic filtering.
- A distributed self-join with grouped output now has exact worker-loss
  recovery coverage, including retries and moved exchange spools
  (`43a2f8f9`). The deployed AKS worker-loss gate is now also green for the
  declared three-worker workload; broader join-shape and cloud-fault coverage
  remains open.
- A grouped aggregate followed by a global descending sort now has an exact
  worker-loss recovery case: the final order, row multiplicity, and re-executed
  stage are asserted after a producer-side worker disappears. This qualifies
  the combined in-process path; the deployed AKS qualification covers the
  declared scan/aggregate workload, while this exact grouped-sort shape still
  needs a deployed differential run.
- Trino `COUNT_IF(predicate)` now lowers to the existing null-preserving
  filtered-count state, including aggregate `FILTER` composition, window use,
  distributed partial/final merges, and explicit arity/wildcard diagnostics.
  The differential corpus covers grouped `COUNT_IF` results across alternate
  encodings and execution paths.
- A three-worker high-cardinality grouped aggregate now has exact local versus
  distributed equivalence under 4 MiB spill pressure, with spill cleanup and
  zero reservation checks (`69d327e9`). Larger-scale and AKS spill gates remain.
- `JSON_EXTRACT_SCALAR` now supports nested dot paths with null/malformed
  input semantics and executor coverage (`054e898a`).
- Typed positional parameters are now exposed through the HTTP statement API,
  validated as JSON scalars, and bound before planning (`0ea777ba`).
- The conventional `/v1/statement` route now accepts both the native JSON
  request and Trino's raw UTF-8 SQL request selected by `Content-Type`; the
  raw path uses the same session headers and Trino-shaped paging response as
  the namespaced adapter. This removes a client-side route special case while
  preserving the native contract (`573c1163` follow-up).
- The HTTP parameter path now has an explicit regression test proving validated
  scalar values reach SQL planning without string interpolation (`dcd67679`).
- Correlated scalar subqueries now have explicit rejection and binder-boundary
  coverage; the preserved outer reference documents the remaining Apply/lateral
  runtime requirement (`72c7e302`).
- Running exchange cancellation now marks all dependent stages terminal,
  drains queued work, and emits one idempotent cleanup intent (`c2ea4da2`).
- Cancelled exchange spools now reject late producer appends with `Interrupted`,
  preventing post-cancellation writes and quota charging (`b9b9c450`).
- `ROW(...)` and `MAP(...)` constructors now lower and execute as typed Arrow
  struct/map values; dereference, lookup, and row/map comparison remain typed
  complex-type boundaries (`12d44965`, `8ae24b96`).
- `CAST(... AS JSON)` and `CAST(... AS JSONB)` now validate and lower to the
  Engine's UTF-8 JSON representation, which is the type consumed by the
  existing `JSON_*()` functions; `TRY_CAST` returns NULL for invalid documents.
  Parser coverage includes both spellings. Binary JSON storage and full Trino
  JSON coercion semantics remain outside this slice.
- Ordinary aggregate `FILTER (WHERE ...)` clauses now lower to
  null-preserving `CASE` expressions, including `COUNT(*) FILTER`, so the
  existing aggregate and spill paths retain their exact semantics. Approximate
  distinct and percentile sketches use the same null-elision contract, so their
  filters are supported as well. Window filters use that same lowering, and
  `COUNT|SUM|AVG|MIN|MAX(DISTINCT value) OVER (...)` now deduplicates each
  frame by logical value (including dictionary-backed columns) before the
  aggregate is evaluated. Column-statistics profiles and non-aggregate
  filters remain explicit unsupported diagnostics.
- The filtered-aggregate corpus case now compares dictionary, plain, split,
  clustered, partitioned, and two-worker distributed execution paths with
  exact canonical rows (`56a07582`).
- `QUALIFY` now lowers to a filter after the window stage, including
  projection aliases that reference window expressions; the planner no longer
  silently drops the clause. Broader QUALIFY shapes still require additional
  differential coverage.
- The core planner now lowers `FETCH FIRST/NEXT … ROWS ONLY` directly to the bounded limit/offset path (`efaaffaf`), while rejecting `PERCENT`, `WITH TIES`, and conflicting `LIMIT`/`FETCH` forms explicitly.
- Trino's `FETCH FIRST … ROWS ONLY` and `OFFSET … ROWS FETCH NEXT … ROWS
  ONLY` forms now share the existing bounded TopN/offset path and are covered
  in the multi-encoding distributed differential corpus.
- Bounded projection scalar subqueries now support an ungrouped, single-column
  aggregate and preserve its alias; correlated and multi-row projection forms
  remain explicitly rejected (`d7be5f9a`).
- `INTERSECT ALL` and `EXCEPT ALL` previously had exact unsupported
  diagnostics (`1c053837`). The multiset forms now carry their quantifier
  through logical planning, optimizer rewrites, local execution, and
  distributed fragments; duplicate counts follow the SQL multiset rules
  (`88a96207`).
- The differential sweep now includes `INTERSECT ALL` and `EXCEPT ALL` over
  repeated keys across dictionary, plain, split, clustered, partitioned, and
  two-worker paths (`f7e164ef`).
- Broadcast planning now has an explicit dynamic-filter boundary test: static
  predicates reach the build scan, while runtime build-key propagation to the
  probe scan remains refused until its correctness and exchange behavior are
  qualified (`57db621d`). The bounded in-memory join-side filter is covered
  separately and does not claim scan pruning.
- Semi/anti join planning now hash-partitions both inputs across the worker
  set when both keys are resolvable columns, including the binder's positional
  `*` marker for correlated EXISTS projections. Truly unresolvable keys retain
  the explicit broadcast fallback; runtime build-key propagation and adaptive
  broadcast selection remain open.
- Streamed-root task metrics now have a stabilized eventual-read test: the
  metrics endpoint may report `202 Accepted` briefly and is polled until the
  completed metrics record is available (`6cd66cb4`).
- Named `:name` parameters now bind typed scalar values through the HTTP
  statement API, alongside sequential `?` and typed positional `$n`; missing
  names fail before planning (`080c9ace`). QUALIFY predicates now execute after
  window materialization; broader window-aware differential coverage remains.
- `IS DISTINCT FROM` and `IS NOT DISTINCT FROM` now lower to null-total
  `CASE` expressions using the existing vectorized null predicates and
  comparisons; executor coverage verifies both-null, one-null, equal, and
  unequal rows, and the differential corpus now exercises nullable grouped
  data on local and distributed paths.
- `TRY_CAST` now lowers to Arrow safe per-element casts for the supported
  primitive, date/time, and decimal targets; invalid values become NULL while
  valid values in the same batch remain intact (`6e4a8a44`). The distributed
  differential corpus also covers invalid text-to-integer casts becoming NULL
  and valid numeric casts remaining usable in aggregates.
- `TRY(CAST(...))` and `TRY(JSON_PARSE(...))` now lower to those same safe
  vectorized evaluators, preserving Trino's NULL-on-invalid-value behavior
  without adding a second execution path. Other arbitrary `TRY(expr)` forms
  remain explicitly rejected until the executor can mask row-local failures
  for every scalar operator.
- `GREATEST` and `LEAST` now fold through the common input type instead of
  forcing results to `DOUBLE`; pairwise comparisons preserve integers, text,
  dates, and other supported comparable values, and any NULL argument produces
  NULL for that row. Focused executor coverage protects both type preservation
  and null propagation.
- `SPLIT_PART` now provides Trino's one-based string partitioning semantics,
  including NULL propagation, NULL for an out-of-range index, and an explicit
  error for non-positive indexes.
- `REGEXP_EXTRACT` now compiles each pattern through the shared regex cache,
  returns capture group 0 by default or a requested zero-based group, and
  preserves NULL results for NULL inputs, non-matches, and missing captures.
- `REGEXP_COUNT(string, pattern)` now counts non-overlapping matches through the
  shared bounded regex cache, preserving NULL inputs and invalid-pattern errors.
- `REGEXP_EXTRACT_ALL(string, pattern)` now returns a typed array of
  non-overlapping matches (or capture group 1 when present), preserving NULL
  inputs and shared regex-cache error behavior.
- `REGEXP_SPLIT` now uses the shared compiled-pattern cache, returns a typed
  text array, preserves trailing empty fields, and propagates NULL inputs.
- `CONCAT_WS` now skips NULL value arguments, preserves empty strings, and
  returns NULL when its separator is NULL.
- `TRANSLATE` now maps Unicode code points by position, keeps source
  characters without a mapping, and removes characters whose target is absent.
- `CHR` and `CODEPOINT` now round-trip Unicode code points, preserve NULLs, and
  reject invalid code points or multi-code-point `CODEPOINT` inputs.
- `STRPOS` now supports positive and negative occurrence indexes with Unicode
  character positions while retaining the two-argument form; zero indexes
  fail explicitly.
- `SPLIT` now returns typed text arrays, preserves NULL inputs, supports a
  positive limit whose final element contains the remainder, and follows
  Trino's character splitting behavior for an empty delimiter.
- `ROW(...)` constructors now produce typed Arrow `StructArray` values with
  null-preserving child fields. `MAP(...)` now produces Arrow `MapArray` values
  from matching list offsets and rejects NULL keys. `MAP_KEYS` and `MAP_VALUES`
  now expose the corresponding list views while preserving map offsets and null
  rows; dereference and keyed lookup remain explicit complex-type gaps.
- `SPLIT_TO_MAP` now parses delimited text into typed text maps, preserves NULL
  rows, and rejects empty delimiters and duplicate keys.
- `SPLIT_TO_MULTIMAP` now groups repeated delimited keys into ordered text
  arrays while preserving map-row NULLs and delimiter validation.
- `ARRAY_POSITION(array, value)` now returns Trino-compatible one-based positions
  for numeric and text lists, returns `0` when absent, preserves NULL list and
  element semantics, and fails explicitly for unsupported element types
  (`df3fa16a`).
- Array subscripts now lower to `ELEMENT_AT(array, index)` with one-based and
  negative-index semantics, null list/index handling, and null for out-of-range
  positions. PostgreSQL-style array slices now lower to `ARRAY_SLICE` for
  numeric and text lists, including omitted bounds, positive or negative
  strides, and null elements. `ARRAY_DISTINCT` now preserves first occurrence
  order and keeps at most one NULL for numeric, text, and boolean lists;
  `ARRAY_SORT` orders those same supported element types with NULL values last;
  `ARRAY_REVERSE` reverses those lists while preserving NULL elements;
  Int32 and temporal DATE32, DATE64, TIME32 second/millisecond, and TIMESTAMP
  microsecond lists are also supported for these operations; broader element
  types remain an explicit boundary.
- Array constructors now execute as typed Arrow lists for boolean, 32/64-bit integer,
  `DATE`/`DATE64`, `TIME32`, floating-point, UTF-8, decimal, and all-NULL elements. Mixed integer and
  floating-point literals are promoted to `DOUBLE`; null elements preserve the
  list's inferred type. Compound and unsupported element types remain explicit
  boundaries.
- `ROW(...).field` now lowers to a typed `ROW_FIELD` projection over Arrow
  structs, and map subscript syntax lowers to `ELEMENT_AT` with typed key
  lookup; row equality and order-independent map equality now compare typed
  values with SQL null semantics, while ordered row and map comparison remain
  open.
- `CARDINALITY` now returns map entry counts as well as array lengths, with
  null map rows preserved as null.

### Current focused validation

The latest focused validation passes 350 `kaveon-server` tests, 119
`kaveon-sql` tests, and 288 `kaveon-exec` library tests (four executor
benchmarks are ignored). The repository differential gate
(`differential_tests::the_differential_sweep_matches_across_parquet_encodings_and_execution_paths`)
also passes, comparing the same corpus across alternate Parquet encodings and
local/distributed execution paths. These are package-level and in-process
qualification snapshots; they do not close the broader multi-worker, AKS, or
cross-connector qualification gates below.

The distributed failure subset is also green in-process: seven retry and
worker-loss tests, six spill/recovery tests, and two cancellation tests passed.
Those tests prove bounded local behavior and simulated worker failure; they do
not substitute for a deployed multi-worker loss-and-recovery run.

On 2026-10-12 the five-worker native soak harness completed 120.5 seconds with
696 exact-result queries and five control cycles, including an intentional
worker loss. All query results matched the DuckDB reference; cancellation and
queue handling, bounded history, exchange cleanup, monitor responsiveness,
bounded RSS growth, and surviving-worker health all passed. The run recorded a
265 ms median and 515 ms p95 latency. This is a stronger local qualification
snapshot, but it is not AKS evidence: it uses the local binary and fixture and
does not qualify Azure storage, TLS, network policy, or node-drain behavior.

The deployed AKS gate also passed on 2026-10-12 after scaling `kaveon-aks` to
three user workers. Nine concurrent exact queries, twelve bounded-pressure
requests, an intentional worker termination during a 504M-row distributed scan,
replacement-worker catalog compatibility, retry identity, exact recovery, and
post-recovery probing all passed on the immutable Engine image recorded in
`docs/qualification/engine-aks-fault-pressure-2026-10-12.json`. This closes the
deployed worker-loss and bounded-pressure gate for that image; node drain,
coordinator restart, sustained soak, and cloud-storage fault injection remain
open.

The same cluster also passed a two-phase native `ANALYZE` durability check on
2026-10-12. `OpenSource.ai_benchmarks.leaderboard` reported its exact 34-row
current statistic before and after a coordinator StatefulSet restart; the new
pod became Ready in 13 seconds. The credential-safe reports are archived in
`docs/qualification/engine-aks-native-analyze-2026-10-12.md`. This proves
durable statistics persistence across a coordinator restart for the tested
table and image; it does not prove exchange-spool cleanup or active-query
recovery, so those coordinator gates remain open.

On the same deployment, `aks-workers-15909223-vmss000002` was cordoned and
drained. Its StatefulSet worker was evicted and recreated; after the node was
uncordoned the replacement became Ready and all three workers reported zero
container restarts. The credential-free result is archived in
`docs/qualification/engine-aks-node-drain-2026-10-12.md`. This closes node
scheduling and worker replacement evidence, but not an in-flight query
surviving node loss; that active-query node-drain gate remains open.

That active-query boundary is now qualified for the declared current workload:
on 2026-10-13 the harness drained the node hosting `kaveon-worker-0` during a
running 504M-row distributed scan. The coordinator observed the loss, created
a retry attempt, returned the exact baseline result, and restored a
catalog-compatible three-worker cluster; the exact post-recovery probe and
bounded-pressure tail also passed. The credential-free report is archived in
`docs/qualification/engine-aks-fault-pressure-2026-10-13-node-drain.md`.
This closes active-query worker recovery during node drain for that image and
workload. Coordinator exchange-spool cleanup and coordinator active-query
restart recovery remain separate gates.

An idle coordinator restart on 2026-10-13 also found zero retained exchange
files both before and after restart, with three active and catalog-compatible
workers after readiness. The report is archived in
`docs/qualification/engine-aks-coordinator-cleanup-2026-10-13.md`. This closes
the zero-retained-file cleanup check for an idle restart; it does not claim
active-query survival during coordinator loss.

The current image then passed a bounded 60-second AKS mixed-workload soak on
2026-10-13: four concurrent clients completed 88 exact-result requests, with
all three workers Ready, zero unexpected restarts, and zero retained exchange
or spill files before and after. Evidence is archived in
`docs/qualification/engine-aks-current-soak-2026-10-13.md`. This closes the
declared bounded-soak check, but is not a multi-hour endurance or broad
capacity claim.

The same harness then ran for five minutes on the current image and completed
416/416 exact-result requests with the same worker, restart, and retained-file
checks green. That sustained report is archived in
`docs/qualification/engine-aks-current-soak-2026-10-13-300s.md`. This closes
the declared five-minute sustained-soak check; upgrade/rollback, storage fault
injection, and longer endurance remain separate gates.

Coordinator-loss behavior is now also qualified as fail-closed on 2026-10-13:
a 504M-row scan was observed running, the coordinator was restarted, and the
in-flight request terminated with a connection interruption rather than a
partial result. A fresh post-restart connection returned an exact result, all
three workers were Ready, and retained coordinator exchange files did not grow.
Evidence is archived in
`docs/qualification/engine-aks-coordinator-loss-2026-10-13.md`. This closes
clean interruption and fresh-query recovery; in-flight query survival across
coordinator loss remains intentionally unsupported and documented.

The distributed/local join-mode differential now also includes an inner
residual predicate (`key equality AND value inequality`); local and
distributed result sets match and all reservations are released.

The same differential corpus now covers SQL Lab's SQL Server-style bracketed
identifiers (`SELECT DISTINCT [region] ...`) across all Parquet encodings and
execution paths. The parser normalizes identifier-shaped brackets before
planning, preventing them from reaching workers as an `ARRAY` scalar call.
The deployed AKS image also returned identical exact results for bracketed and
plain `year` forms over `OpenSource.climate_energy.energy_annual`; the live
qualification is archived in
`docs/qualification/engine-aks-bracketed-identifier-2026-10-13.md`.

The AKS StatefulSets were subsequently rolled to the immutable Engine image
`kaveonacr.azurecr.io/kaveon-engine@sha256:fe468d403a53b82667427952db8121cd491b30238455c83a393b7e6cdfe63570`,
built from commit `3c4a943b`. On that image, a distributed `QUALIFY` query
using `ROW_NUMBER()` returned the same rows as its equivalent derived-table
filter, and the bracketed-identifier query again matched the plain form. This
also verifies that the earlier live `QUALIFY` parse failure was stale-image
drift rather than an implementation failure. Evidence is archived in
`docs/qualification/engine-aks-qualify-latest-2026-10-13.md` and
`docs/qualification/engine-aks-bracketed-latest-2026-10-13.md`.

The ordering subset is green in-process as well: eight planner, fragment, and
API tests cover distributed scan inputs, TopN lowering, OFFSET retention,
exchange execution, and spill-aware final merge. Global ORDER BY still ends in
a coordinator merge by design; a fully parallel ordered result stream remains
open.

Track 1 remains partial because global ordering still uses a centralized merge
and coordinator-loss recovery is intentionally fail-closed rather than
survivable for an in-flight query. Deployed worker-loss recovery, active-query
node-drain recovery, bounded pressure, spill/retry, cancellation cleanup, the
five-minute mixed-workload soak, and coordinator-restart cleanup are qualified
for the declared three-worker image and workload. The current HTTP exchange
spool is bounded, replayable, and incrementally decoded; network backpressure
under sustained pressure, cloud-storage fault injection, longer endurance,
upgrade/rollback, and deployed cancellation under those conditions remain
open.

The fault-pressure runner now includes the active-query cancellation probe and
records `checks.cancellation_cleanup`; the merged SLO artifact consumes that
field rather than inferring cleanup from a successful query response
(`097e2084`, `b90b067e`).

The live pod-loss qualification on 13 October 2026 passed its declared
three-worker workload: concurrent exact results, active cancellation cleanup,
worker termination and exact retry, replacement-worker compatibility, bounded
pressure, and a post-recovery probe. The run terminated worker `kaveon-worker-2`
after task execution began; the retry returned the same `504,000,000` result
and SHA-256 as baseline, and the replacement rejoined with the same catalog
snapshot. The complete credential-safe report is archived in
`docs/qualification/engine-aks-fault-pressure-2026-10-13.md` and its adjacent
JSON evidence file. Admission refusals are recorded separately and retried with
bounded backoff; they are not counted as successful statements. This is a
positive Track 1 evidence slice, not the release SLO completion artifact.

The coordinator-loss qualification on 14 October 2026 also passed its declared
cleanup contract. Restarting the coordinator interrupted the in-flight request
as expected, all three workers and the coordinator became ready again, the
exact post-restart probe returned `24`, and retained coordinator exchange files
were zero. The result is archived in
`docs/qualification/engine-aks-coordinator-loss-2026-10-14.md`. This qualifies
restart cleanup and fail-closed recovery only; it makes no active-query survival
or multi-coordinator availability claim.

### Track 1 distributed execution SLO

The SLO is scoped to a declared qualification workload and image; it is not a
promise for arbitrary data, hardware, or cloud topology. A release candidate
must satisfy all of these gates in one archived, reproducible qualification
artifact:

- **Correctness:** 100% of accepted statements return the expected typed result
  hash. Admission refusals are reported separately and never counted as
  successful work; an execution error or silent mismatch fails the gate.
- **Reliability:** at least 99.9% of accepted statements complete successfully
  during the five-minute mixed-workload window, with zero silent wrong results.
  The worker-loss exercise must recover the exact result after retry, and
  cancellation must leave no exchange or spill residue.
- **Latency and throughput:** every run publishes p50, p95, p99, and maximum
  end-to-end latency, plus successful exact-result QPS. There is no universal
  latency number independent of data shape; the comparison target remains the
  separately declared 1.90x Trino throughput objective for its matched suite.
- **Bounded resources:** peak process/pod memory stays within its configured
  limit, exchange and spill bytes remain accounted, and cleanup reaches zero
  retained temporary files after cancellation and restart checks.

The gate is fail-closed: missing samples, missing task metrics, omitted worker
loss or cancellation evidence, or a report from a different image/configuration
leaves the SLO **pending**. The current three-worker evidence covers only the
declared local qualification workload; sustained network pressure, cloud-store
fault injection, coordinator-loss survival, longer endurance, and
upgrade/rollback still block an SLO-complete claim.

The 13 October 2026 AKS comparison (`a20`) completed all six alternating rounds
on the pinned three-worker topology with all twelve exact-result cases passing,
TLS and authentication enforced, stable co-tenant identities, and successful
StatefulSet restoration. Kaveon measured 2.844 successful exact-result QPS
against Trino's 1.577 QPS (1.8035x) on the immutable ADLS fixture. The run is
therefore valid evidence that Kaveon is faster on this declared workload, but
it does not meet the 1.90x objective. The claim gate remains closed, and the
report is archived as `tmp/20261013-a15/report-a20.json` with its gate output in
`tmp/20261013-a15/claim-gate-a20.json`. During this run the coordinator also
waited through asynchronous catalog-snapshot publication instead of converting
a transient worker-convergence window into an HTTP 500; that behavior is in
`cf6615be` and covered by the server test suite (359 passed). The same suite
also includes the bracketed-identifier regression (`SELECT DISTINCT [region]`)
that prevents Studio's SQL Server-style completion from being interpreted as
an `ARRAY` expression by workers; a deployed image must include commit
`6818e65f` or later for that protection to be present.
Track 2 remains partial because prepared statements are now available through
the Engine HTTP lifecycle and scalar/array/map `EXECUTE ... USING` is supported by
the Trino adapter. The raw Trino statement route also accepts owner-scoped SQL
`PREPARE name FROM ...`, `EXECUTE name`, and `DEALLOCATE PREPARE name`, using
the same typed binder and normal SQL parser. The adapter still lacks a
wire-compatible full protocol;
correlated non-aggregate and other unsupported scalar subqueries, ordered map
comparisons, broader array element types, and broader typed DML are not yet
implemented. Quantified array comparisons now cover all scalar comparison
operators with NULL and empty-array tests; nested array literals and arrays of
typed row/map values now materialize as typed container results. Equality-key
outer joins now carry residual
`ON` predicates through the distributed hash join and apply null extension only
after the complete predicate is evaluated.
Equality-correlated `IN` and `NOT IN` forms whose projected value is the
correlation key are now included in the exact local/distributed differential
corpus, including one equality key with a residual predicate; multi-key and
deeper correlations remain explicitly rejected. A `DISTINCT` wrapper around
the projected key is transparent to this keyed semi/anti lowering, matching
Trino's duplicate-insensitive membership semantics.
Grouped `SUM(DISTINCT ...)` and `AVG(DISTINCT ...)` now preserve their exact
distinct-value state through the executable fragment wire contract, so the
distributed final merge unions values before calculating the result. The
multi-encoding differential corpus covers these grouped forms; older fragments
without the field remain compatible as non-distinct aggregates.
Positional and named parameter cardinality is checked before planning; extra
values fail closed instead of being silently ignored.
Column-key `GROUPING SETS ((a), (b), ...)`, the grand-total `GROUPING SETS (())`,
`CUBE`, and `ROLLUP` now lower to typed `UNION ALL` aggregate branches and are covered by SQL and physical-
planner tests and a local execution regression. `GROUPING(column)` and
`GROUPING_ID(...)` are materialized for expanded column-key branches. Expansion
is bounded at 4,096 aggregate branches; CUBE expansions with 16 or more keys
remain explicitly unsupported. The compatibility claim is intentionally limited to
column-key grouping expressions, including ordinary keys carried alongside a
grouping specification.
`MIN(DISTINCT ...)` and `MAX(DISTINCT ...)` are accepted as Trino-compatible
extrema (deduplication does not change an extremum), with grouped distributed
differential coverage alongside the sum/average cases.
Multiset set operations are now implemented
and covered end to end. Positional
 scalar parameters, including named placeholders, are supported through the
HTTP statement API; wire-compatible Trino prepare/describe semantics remain
an explicit unsupported case.
Plan-only `EXPLAIN` now uses the normal binder and optimizer and returns
logical, optimized, or physical/distributed plan rows without starting a scan,
worker task, cache entry, or exchange. `GREATEST` and `LEAST` preserve their
common input type and propagate NULL arguments; both behaviors have focused
executor and statement-path coverage.

These slices do not close Tracks 1 and 2's qualification gates; the broader
three-worker corpus, complex types, correlated/unsupported scalar shapes,
prepared-statement coverage, worker-loss, and exact-result differential runs
are still required.

The current local validation baseline is green on the same checkout: the full
`kaveon-server` suite passed **367 tests**, and the full CLI suite passed **211
tests** (one ignored). The set-operation coverage includes `UNION ALL`, distinct and
multiset `INTERSECT`/`EXCEPT`, dictionary-versus-plain values, duplicate
counts, null handling, planner lowering, and exchange-input consumption. This
is implementation and regression evidence for the covered shapes; it does not
replace the deployed differential corpus or the remaining complex-type and
wire-protocol gates.

The deployed cache evidence harness is checked in at
`engine/qualification/cache_slo.py`. The AKS qualification is archived at
`docs/qualification/cache-slo-aks-2026-10-13.json`; it reports warm-hit
percentiles, exact hashes, bypass behavior, cache bounds/counters,
administrator clear behavior, authorization isolation, and coordinator
restart-to-miss. Shared cross-coordinator invalidation, TTL-expiry evidence,
and cache-backed transactional reads remain outside that qualification.

## 9/10 exit gate

Do not claim 9/10 until all of the following are true:

1. No known silent wrong-result path exists in the declared SQL corpus.
2. Distributed joins and aggregates pass worker-loss, spill, cancellation, and
   snapshot tests on the deployed topology.
3. At least the declared lake and SQL-source connector matrix passes pushdown,
   auth, retry, and cancellation tests.
4. Optimizer choices are backed by current statistics and plan evidence.
5. Matched benchmarks include exact result hashes, failures, p50/p95/p99,
   CPU, memory, spill, network, and storage measurements.
6. Recovery, backup/restore, upgrade, and rollback evidence is archived.
7. Every unsupported feature is rejected clearly and documented; no fallback is
   silent or coordinator-only without telemetry.

The current Engine readiness rubric remains the source for the existing 8/10
qualification. This tracker records the additional work required for the
broader Trino comparison: `docs/engineering/engine-readiness-qualification.md`.
