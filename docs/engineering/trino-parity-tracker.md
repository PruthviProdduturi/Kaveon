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
| 1 | Distributed SQL execution | Partial | Three-worker exact-result corpus covering joins, aggregates, windows, sort, TopN, set operations, worker loss, cancellation, and snapshot pinning |
| 2 | SQL compatibility and types | Partial | Differential corpus against Trino/DuckDB covering NULLs, casts, arrays/structs/maps, CTEs, windows, subqueries, set operations, and explicit unsupported errors |
| 3 | Federation connectors and pushdown | Partial | Connector matrix with authenticated reads, predicate/projection/limit pushdown, partition pruning, retries, cancellation, and secret isolation |
| 4 | Statistics and cost-based optimization | Partial | Exact Parquet/Delta/Iceberg statistics, join-order and broadcast decisions, runtime filters, and plan-regression evidence |
| 5 | Memory, spill, and admission | Partial | Concurrent join/aggregate/sort pressure, bounded reservations, spill cleanup, quotas, cancellation, and no silent coordinator fallback |
| 6 | Recovery and operations | Partial | Coordinator/worker restart, exchange loss, retries, upgrade/rollback, backup/restore, and sustained mixed-workload soak |
| 7 | Benchmark and publication gate | Partial | Matched ClickBench, TPC-H/TPC-DS, federation, and Kaveon workloads with hashes, p50/p95/p99, errors, resource use, and cost |

Workstreams **1** and **2** begin in parallel. Workstreams **3**, **4**, and
**7** are being audited in parallel so implementation work is measured against
the same acceptance contract rather than separate informal benchmarks.

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
