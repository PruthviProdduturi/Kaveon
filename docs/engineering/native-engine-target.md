# Native distributed and transactional Kaveon

The product target is Kaveon itself: distributed analytical execution and strong
transactional behavior, with ADLS as the durable data store. This is a target,
not a claim that current Kaveon matches or exceeds Trino or PostgreSQL.

The SQLite-in-memory product metadata experiment is isolated on the local
`wip/adls-product-metadata-prototype` branch at `c44d692`. It was not deployed or
used to cut over PostgreSQL. It may serve as a constraint/SQL compatibility
reference in tests; it is not the native transaction execution architecture.

## Acceptance before product migration

1. Native typed DDL and parameterized INSERT/UPDATE/DELETE execute through
   Kaveon planning/execution. Unsupported operations fail explicitly. A
   metadata adapter does not substitute for the missing engine operators.
2. Reads pin a committed snapshot. Writes validate their read/write sets and
   publish all affected table versions atomically. The exposed isolation level
   must match tested behavior; serializable behavior requires predicate/phantom
   conflict validation as well as write/write conflict detection.
3. Primary, unique, not-null, check and foreign-key constraints survive atomic
   multi-statement updates, competing writers, retries and process restarts.
   Indexes must support bounded point operations without loading every system
   table into coordinator memory for each transaction.
4. ADLS retains table data, schema/index metadata, commit state, idempotency
   records, audit/recovery evidence and Engine definitions. Memory caches and
   temporary execution buffers are expendable. No local database file becomes
   authoritative. Credentials keep their existing managed-secret boundary.
5. Lost commit responses, stale writers, cancellation, coordinator/worker
   termination, missing/corrupt history and storage throttling have explicit,
   tested outcomes. Unknown commit outcome is not reported as rollback.
6. The application migrates all system-table families from PostgreSQL using a
   consistent inventory, deterministic reconciliation, write fencing and a
   demonstrated rollback procedure. Reads and writes must not silently fall
   back to PostgreSQL after declared cutover.
7. Dashboard create/save/reopen, charts, filters, datasets, favorites, ownership,
   role checks and SQL Lab queries pass against the migrated catalog. Restart
   recovery must preserve those results and subsequent edits.

## Performance evidence

Measure distributed analytics against Trino and transactional application
workloads against PostgreSQL as separate suites. Use the same documented data,
equivalent resource budgets, concurrency, query semantics and cache conditions.
Record result hashes, throughput, p50/p95/p99 latency, errors/conflicts,
bytes/rows scanned, memory, network, storage requests and cost per workload.
Report repeated measurements and unsupported cases; do not derive an overall
rating or a percentage lead from a single successful query.

The first analytical suite can reuse the imported NYC Taxi tables and joins to
the zone lookup, plus existing exact-result Engine fixtures. The transactional
suite needs point lookups, insert/update/delete, multi-table changes, contended
keys, range predicates, commit retries and dashboard API workflows. Start with
small correctness runs on the existing three-worker AKS deployment before
spending compute on scale tests.

ADLS-only durable commits introduce storage round trips. Low-latency OLTP is a
separate performance requirement that must be measured; matching transaction
correctness does not imply matching PostgreSQL latency. The user selected both
workloads: general-purpose OLTP alongside distributed analytics, including lakehouse transactions and product metadata. Neither suite
can be omitted from the eventual acceptance claim.

## Current implementation boundary

`product_manifest`, `product_commit`, `product_metrics` and `AdlsConditionalCommit`
are transaction-publication foundations. They do not implement native row
mutations, constraints, SQL transactions or the completed migration. The
[ADLS protocol](adls-transaction-protocol.md) records remaining limitations.
PostgreSQL and the existing SQLite Engine definition catalog remain live until
their respective ADLS replacement gates pass.
