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
- Exchange producers stream output, but consumers still assemble the complete
  payload before decoding. This is a memory and latency blocker for large
  federated exchanges.
- Windows gather to one task. Set operations and final sort/TopN also have
  centralized merge stages. Compatible windows with a shared non-empty
  `PARTITION BY` are now hash-partitioned across workers; unpartitioned and
  mixed-key windows remain centralized for correctness. These are not yet
  Trino-class parallel plans across the full window corpus.
- Dynamic filtering, general join reordering, skew mitigation, and qualified
  coordinator failover are missing.
- Ordinary distributed joins require equality keys; residual predicates are
  only partially supported. Semi/anti joins are not yet partitioned by default.
- Non-recursive CTEs, derived tables, joins, aggregates, `HAVING`, `CASE`, and
  common set operations work. Scalar subqueries, recursive CTEs, `INTERSECT ALL`,
  `EXCEPT ALL`, prepared parameters, and general lake-table DML remain missing.
  Uncorrelated aggregate scalar subqueries in `WHERE` are now supported with
  exact-result differential coverage; projection, correlated, and multi-row
  scalar forms remain explicitly rejected.
  `CAST(... AS DATE)` is now supported with nullable `Date32` execution and
  focused parser/executor coverage. `CAST(... AS TIMESTAMP)` and
  `TIMESTAMP WITH TIME ZONE` now map to Arrow microsecond timestamps with
  nullable execution coverage.
- Numeric array literals are supported as null-preserving Arrow lists; maps and
  structs/row values are not yet represented as complete SQL expression types.
  Complex casts, dereference, `UNNEST`, and broad JSON operations remain open;
  scalar extraction via `JSON_EXTRACT_SCALAR` is implemented for dotted object
  paths.
- There is no Trino-style cross-source federation yet. Registered SQL sources
  are API passthrough paths, while the strongest native Engine path is
  Parquet/Delta/Iceberg. S3 and Iceberg status must be reconciled between the
  connector matrix and older architecture wording before publication.
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
  before Arrow decoding (`8bfb76fa`). The network response is still spooled
  before decode; full async streaming remains open.
- Windows with compatible shared partition keys are hash-partitioned across
  workers (`06e1fb80`). Unpartitioned and mixed-key windows stay centralized.
- Differential coverage now includes `INTERSECT`, `EXCEPT`, and non-recursive
  CTEs (`59aaf0ec`). `CAST(... AS DATE)` now maps to Arrow `Date32` and keeps
  nulls (`bf49f5cb`). Timestamp and timestamp-with-time-zone casts are covered
  in `52fa76c6`.
- Uncorrelated aggregate scalar subqueries in `WHERE`, plus actionable
  multi-row and multi-column diagnostics, are covered in `3a60cf23`.
- SQL placeholders (`$1`, `?`, and named forms) now fail with an explicit
  binding diagnostic rather than being misinterpreted as literals
  (`89499c7b`). Typed request-level parameter transport is still required
  before prepared execution can be enabled.
- A bounded replayable append-only exchange spool foundation is implemented
  with partial-read, fork/retry, cancellation, and writer-failure tests
  (`79ddf443`). It is not yet wired into async HTTP exchange fetching.
- The spool now exposes an incremental Arrow IPC reader with partial-write and
  later-batch coverage (`13f27d74`). HTTP exchange fetches still await complete
  `ArrowPayload` files before entering the replayable input path.
- Decimal casts now validate precision/scale before planning and execute with
  Decimal128 null/scale coverage (`81b610e2`).
- Numeric `ARRAY[...]` literals now execute as null-preserving Arrow
  `List<Float64>` values with parser and executor coverage (`b1fe824f`).
- Three-worker global `ORDER BY`/TopN centralized merge correctness is now
  qualified for NULL ordering and ties against a single-partition reference
  (`bb788709`). Parallel global merge performance remains open.
- A distributed self-join with grouped output now has exact worker-loss
  recovery coverage, including retries and moved exchange spools
  (`43a2f8f9`). AKS worker-loss qualification remains open.
- `JSON_EXTRACT_SCALAR` now supports nested dot paths with null/malformed
  input semantics and executor coverage (`054e898a`).

Track 1 remains partial because distributed global ordering and a full
worker-loss/spill/cancellation corpus still need a correctness gate. The
current exchange spool is bounded and replayable; replacing it with an
append-only network-streaming provider is a separate design task.
Track 2 remains partial because prepared parameters, correlated/projection
scalar subqueries, complex types, multiset set operations, and broader typed
DML are not yet implemented.

These slices do not close Tracks 1 and 2's qualification gates; the broader
three-worker corpus, complex types, scalar/prepared subqueries, worker-loss,
and exact-result differential runs are still required.

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
