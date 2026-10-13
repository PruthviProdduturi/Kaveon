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

## Audit snapshot — October 12, 2026

The initial repository audit confirms that the Engine has real distributed
foundations, but several operators are still centralized or narrow:

- Hash repartition, broadcast, joins, partial/final aggregates, spill, retry,
  cancellation, and snapshot fields exist. Cluster qualification remains open
  for skew, spill pressure, rolling worker loss, and cancellation stress.
- Exchange producers stream output, and HTTP consumers now pump each response
  into a bounded replayable append-only spool while Arrow decodes batches
  incrementally. The old second contiguous payload copy is gone; network
  backpressure, pressure qualification, and sustained federated-exchange
  evidence remain open.
- Windows gather to one task. Set operations and final sort/TopN also have
  centralized merge stages. Compatible windows with a shared non-empty
  `PARTITION BY` are now hash-partitioned across workers; unpartitioned and
  mixed-key windows remain centralized for correctness. These are not yet
  Trino-class parallel plans across the full window corpus.
- Static predicates are pushed into eligible build scans, including broadcast
  joins; the build-side key set is not yet propagated to the probe scan at
  runtime. General dynamic filtering, join reordering, skew mitigation, and
  qualified coordinator failover remain open.
- Ordinary distributed joins require equality keys; residual predicates are
  only partially supported. Semi/anti joins are not yet partitioned by default.
- Non-recursive CTEs, derived tables, joins, aggregates, `HAVING`, `CASE`, and
  common set operations work. Uncorrelated aggregate scalar subqueries in
  `WHERE` and bounded projection scalar subqueries are supported when they
  return one ungrouped, single-column aggregate. Correlated, multi-row, and
  other scalar forms, recursive CTEs, the full prepared-statement protocol,
  and general lake-table DML remain missing. `INTERSECT ALL` and `EXCEPT ALL`
  now preserve multiplicity through local and distributed execution.
  Typed positional `$1`, sequential `?`, and named `:name` parameters are now
  validated and bound through the HTTP statement API; the full prepare/describe/
  bind/execute protocol remains open.
  Uncorrelated aggregate scalar subqueries in `WHERE` are now supported with
  exact-result differential coverage; projection, correlated, and multi-row
  scalar forms remain explicitly rejected.
  `CAST(... AS DATE)` is now supported with nullable `Date32` execution and
  focused parser/executor coverage. `CAST(... AS TIMESTAMP)` and
  `TIMESTAMP WITH TIME ZONE` now map to Arrow microsecond timestamps with
  nullable execution coverage.
- Numeric array literals are supported as null-preserving Arrow lists, and
  `CARDINALITY` returns their nullable list length; `ROW(...)` now produces
  typed Arrow structs; `MAP(...)` now builds Arrow maps from matching key/value
  lists. Field dereference, lookup, and row/map comparison remain open. Scalar, DATE,
  timestamp,
  and DECIMAL casts are supported; complex cast targets fail with typed
  planner diagnostics. Dereference, `UNNEST`, other array operations, and
  broad JSON operations remain open; scalar extraction via
  `JSON_EXTRACT_SCALAR` is implemented for dotted object paths.
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
| Windows, sort, TopN, set operations | Partial | Distributed result hashes and null/order semantics |
| Snapshot pinning | Partial | Catalog mutation during an in-flight query never mixes generations |
| Worker loss and exchange recovery | Partial | Mid-stage loss with no missing or double-counted partitions |

## Workstream 2 — SQL compatibility and types

| Capability | Status | Evidence required |
|---|---|---|
| Joins, filters, grouping, ordering | Partial | Differential corpus with typed result hashes |
| CTEs and subqueries | Partial | Nested and correlated cases, including empty and NULL inputs |
| Windows and set operators | Partial | `OVER`, frames, `UNION`, `INTERSECT`, and `EXCEPT` corpus |
| Arrays, maps, structs, JSON | Partial | Parse, plan, execute, and explicit unsupported diagnostics |
| Decimal, timestamp, interval, and casts | Partial | Overflow, timezone, precision, and implicit-cast cases |
| `CASE`, `HAVING`, `FILTER`, `QUALIFY` | Partial | Exact-result and error semantics |
| Native DML and prepared parameters | Partial | `BEGIN`/`COMMIT`/`ROLLBACK`, parameter binding, and transactional visibility |

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
- The spool now exposes an incremental Arrow IPC reader with partial-write and
  later-batch coverage (`13f27d74`); HTTP exchange fetches use it through
  `5dd9d275`.
- Decimal casts now validate precision/scale before planning and execute with
  Decimal128 null/scale coverage (`81b610e2`).
- Numeric `ARRAY[...]` literals now execute as null-preserving Arrow
  `List<Float64>` values with parser and executor coverage (`b1fe824f`).
- Numeric and text `ARRAY[...]` literals now execute as null-preserving Arrow
  lists; `CONTAINS` supports both element types with SQL three-valued null
  semantics (`b2054d10` plus the follow-up execution coverage).
- `CARDINALITY(ARRAY[...])` now parses and evaluates to a nullable `Int64`
  list length, with explicit rejection for non-list values (`1c0e29e5`).
- Numeric-array `CONTAINS(array, value)` now preserves nullable SQL boolean
  semantics for null arrays, search values, and elements, with planner and
  executor coverage (`b2054d10`). Broader element types and array operations
  remain outside the current typed expression model.
- `ARRAY_POSITION(array, value)` now returns the first 1-based position, `0`
  when a value is absent, and matches null elements for numeric and text lists;
  null list inputs remain null. Unsupported list element types retain an
  explicit execution diagnostic, with parser and executor coverage in this
  slice.
- Three-worker global `ORDER BY`/TopN centralized merge correctness is now
  qualified for NULL ordering and ties against a single-partition reference
  (`bb788709`). The input remains distributed, while the exact global merge
  is explicitly finalized by one task; a range exchange for parallel global
  ordering remains open (`7f32d854`).
- A distributed self-join with grouped output now has exact worker-loss
  recovery coverage, including retries and moved exchange spools
  (`43a2f8f9`). AKS worker-loss qualification remains open.
- A three-worker high-cardinality grouped aggregate now has exact local versus
  distributed equivalence under 4 MiB spill pressure, with spill cleanup and
  zero reservation checks (`69d327e9`). Larger-scale and AKS spill gates remain.
- `JSON_EXTRACT_SCALAR` now supports nested dot paths with null/malformed
  input semantics and executor coverage (`054e898a`).
- Typed positional parameters are now exposed through the HTTP statement API,
  validated as JSON scalars, and bound before planning (`0ea777ba`).
- The HTTP parameter path now has an explicit regression test proving validated
  scalar values reach SQL planning without string interpolation (`dcd67679`).
- Correlated scalar subqueries now have explicit rejection and binder-boundary
  coverage; the preserved outer reference documents the remaining Apply/lateral
  runtime requirement (`72c7e302`).
- Running exchange cancellation now marks all dependent stages terminal,
  drains queued work, and emits one idempotent cleanup intent (`c2ea4da2`).
- `ROW(...)` and `MAP(...)` constructors now lower and execute as typed Arrow
  struct/map values; dereference, lookup, and row/map comparison remain typed
  complex-type boundaries (`12d44965`, `8ae24b96`).
- Unsupported complex `CAST` targets such as `JSON` now fail during planning
  with a stable target-type diagnostic; scalar, DATE, timestamp, and DECIMAL
  casts remain the qualified cast surface (`c259e5d1`).
- Ordinary aggregate `FILTER (WHERE ...)` clauses now lower to
  null-preserving `CASE` expressions, including `COUNT(*) FILTER`, so the
  existing aggregate and spill paths retain their exact semantics. Window
  filters, sketch filters, and non-aggregate filters remain explicit
  unsupported diagnostics (`0eac03cc`).
- The filtered-aggregate corpus case now compares dictionary, plain, split,
  clustered, partitioned, and two-worker distributed execution paths with
  exact canonical rows (`56a07582`).
- `QUALIFY` is now rejected during planning with a stable diagnostic instead
  of being parsed and silently ignored; window-aware QUALIFY execution remains
  an explicit Track 2 gap.
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
  qualified (`57db621d`).
- Semi/anti join planning now explicitly qualifies the current distributed
  boundary: the probe remains distributed across four tasks while the
  subquery is broadcast, and runtime build-key propagation is not claimed
  (`cc833eae`).
- Streamed-root task metrics now have a stabilized eventual-read test: the
  metrics endpoint may report `202 Accepted` briefly and is polled until the
  completed metrics record is available (`6cd66cb4`).
- Named `:name` parameters now bind typed scalar values through the HTTP
  statement API, alongside sequential `?` and typed positional `$n`; missing
  names fail before planning (`080c9ace`). `QUALIFY`
  continues to fail closed with a stable planner diagnostic rather than being
  silently ignored.
- `IS DISTINCT FROM` and `IS NOT DISTINCT FROM` now lower to null-total
  `CASE` expressions using the existing vectorized null predicates and
  comparisons; executor coverage verifies both-null, one-null, equal, and
  unequal rows.
- `TRY_CAST` now lowers to Arrow safe per-element casts for the supported
  primitive, date/time, and decimal targets; invalid values become NULL while
  valid values in the same batch remain intact (`6e4a8a44`).
- `ROW(...)` constructors now produce typed Arrow `StructArray` values with
  null-preserving child fields. `MAP(...)` now produces Arrow `MapArray` values
  from matching list offsets and rejects NULL keys. `MAP_KEYS` and `MAP_VALUES`
  now expose the corresponding list views while preserving map offsets and null
  rows; dereference and keyed lookup remain explicit complex-type gaps.
- `ARRAY_POSITION(array, value)` now returns Trino-compatible one-based positions
  for numeric and text lists, returns `0` when absent, preserves NULL list and
  element semantics, and fails explicitly for unsupported element types
  (`df3fa16a`).
- Array subscripts now lower to `ELEMENT_AT(array, index)` with one-based and
  negative-index semantics, null list/index handling, and null for out-of-range
  positions; array slices remain an explicit boundary.
- `ROW(...).field` now lowers to a typed `ROW_FIELD` projection over Arrow
  structs, and map subscript syntax lowers to `ELEMENT_AT` with typed key
  lookup; row equality now compares typed struct fields with SQL null
  semantics, while ordered row and map comparison remain open.
- `CARDINALITY` now returns map entry counts as well as array lengths, with
  null map rows preserved as null.

### Current focused validation

The latest focused validation passes 330 `kaveon-server` tests, 107
`kaveon-sql` tests, and 228 `kaveon-exec` tests (four executor benchmarks are
ignored). These counts are package-level validation snapshots; they do not
close the broader multi-worker, AKS, or cross-connector qualification gates
below.

Track 1 remains partial because global ordering still uses a centralized merge
and a full worker-loss/spill/cancellation corpus still needs a correctness
gate. The
current HTTP exchange spool is bounded, replayable, and incrementally decoded;
network backpressure, sustained pressure evidence, and AKS cancellation
qualification remain open.
Track 2 remains partial because the full prepared-statement protocol,
correlated and other unsupported scalar subqueries, complex types, and broader
typed DML are not yet implemented. Multiset set operations are now implemented
and covered end to end. Positional
 scalar parameters, including named placeholders, are supported through the
HTTP statement API; the full prepared-statement protocol remains an explicit
unsupported case.

These slices do not close Tracks 1 and 2's qualification gates; the broader
three-worker corpus, complex types, correlated/unsupported scalar shapes,
prepared-statement coverage, worker-loss, and exact-result differential runs
are still required.

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
