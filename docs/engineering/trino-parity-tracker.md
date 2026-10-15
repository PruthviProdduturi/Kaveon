# Kaveon–Trino parity tracker

This is the current capability matrix for Kaveon's distributed SQL runtime and
Trino-compatible SQL surface. It is a reviewed snapshot, not a session log.
Implementation history lives in Git; reproducible measurements live under
`docs/qualification/`. A row is **Qualified** only when its stated acceptance
evidence passes against exact-result references.

## Status vocabulary

- **Qualified** — the declared acceptance test passes and its evidence is retained.
- **Implemented** — code and focused tests pass, but the deployment or workload gate is open.
- **Partial** — a useful slice exists, with a defined boundary still open.
- **Missing** — no supported implementation exists.

## Track 1 — distributed SQL execution

| Capability | State | Current contract | Acceptance evidence still required |
|---|---|---|---|
| Worker discovery and scheduling | Qualified (bounded) | Coordinator assigns fragments and retries tasks across compatible workers; the declared three-worker Docker/AKS topology has exact skew, forced-loss, repeated-pressure, and graceful node-drain evidence. | Additional supported topologies and endurance qualification. |
| Hash exchange and repartition | Qualified (bounded) | Authenticated Arrow exchange, stable partitioning, bounded payloads, replayable spools, fail-closed live quota rejection, cleanup, 900-second ADLS soak, and a qualified AKS object-store fault/retry run with zero retained files. | Longer mixed-workload endurance, broader topologies, and cross-region/object-store qualification. |
| Distributed joins | Partial | Partitioned and broadcast hash joins, residuals, outer joins, semi/anti joins, bounded join spill, and bounded AKS outer-join/skew evidence. | Disk-exhaustion, forced join-spill recovery, larger high-cardinality joins, and measured join-spill qualification. |
| Distributed aggregates | Implemented | Partial/final aggregation, exact distinct, AVG, grouped spill, adaptive partials, hybrid final merge. | Larger-scale AKS pressure and published p95 SLO. |
| Windows | Partial | Distributed row-number and compatible partitioned windows are qualified; mixed-key and unpartitioned windows remain centralized for correctness. | Parallel execution for the remaining window shapes. |
| Global ORDER BY and TopN | Partial | Inputs distribute and a coordinator merge preserves exact global ordering; ordinal keys that survive wildcard expansion are rebound to the final output scope, generic `ORDER BY … LIMIT` is applied only after the global merge, `FETCH FIRST/NEXT … WITH TIES` preserves the boundary peer set, and already-sorted integer worker candidates use a bounded k-way merge fast path. | Fully parallel ordered result stream without a coordinator bottleneck. |
| Memory admission and spill | Implemented | Per-query reservations, resource groups, bounded spill, cancellation cleanup, and fail-closed limits. | Five-minute mixed-workload soak and cloud spill evidence. |
| Cancellation and retry | Qualified (bounded) | Client cancellation and retry propagate through stages and exchange cleanup. | Long-running federated cancellation under pressure. |
| Worker-loss recovery | Qualified (bounded) | Replacement-worker recovery preserves exact results and snapshot pinning for the declared three-worker gate. | Node-drain and repeated-loss endurance. |
| Coordinator restart | Qualified (bounded) | Idle and active-query cleanup are fail-closed; an in-flight query is interrupted safely, workers reregister, and the exact post-restart probe passes on the declared topology. | Multi-coordinator/failover survival with in-flight query continuity. |
| Snapshot consistency | Implemented | Fragments pin catalog and Delta versions for a statement and its retries. | Cross-coordinator recovery qualification. |
| Runtime dynamic filtering | Partial | Build-side filters reduce probe work before hashing; eligible single-key and composite-key (independent safe `IN` projections) Delta scans, local Parquet file/directory scans, single-object Parquet readers, and object-directory scans accept build-side predicates before decoding; the operator interface applies every safe conjunction child before falling back. Composite predicates now reach the composite storage branch, NULL build keys are excluded from the safe projections, and partitioned execution has an exact-result composite-key regression. Directory partition values are folded first, so matching files are pruned and physical-column residuals reach the decoder. | Remaining type/exchange edge cases and cloud snapshot qualification. |
| Join reordering and skew mitigation | Partial | Statistics-aware planning can reorder eligible inner equi-joins, preserve the original output order through an explicit projection, and choose a bounded broadcast build; skew splitting and a general cost-based join enumeration policy are not implemented. | Skew policy, multi-join cost model, and plan-regression corpus. |
| Connector federation | Partial | Native Parquet/Delta/Iceberg paths exist; registered SQL sources are passthrough API paths. | Authenticated multi-source pushdown and cross-source execution qualification. |
| Upgrade, rollback, and sustained soak | Missing | Deployment artifacts exist; the full operational acceptance run is not complete. | Upgrade/rollback rehearsal and sustained mixed workload. |

## Track 2 — SQL compatibility and types

| Capability | State | Current contract | Acceptance evidence still required |
|---|---|---|---|
| Core SELECT surface | Implemented | WHERE, projection (including scalar SELECT statements without `FROM`, top-level and derived `VALUES` relations with column aliases, and typed DATE/TIME literal results), CASE, casts, GROUP BY, `COUNT_IF`, bounded GROUPING SETS/CUBE/ROLLUP lowering with `GROUPING()`/`GROUPING_ID()`, HAVING, ORDER BY (including select ordinals), LIMIT/OFFSET, `FETCH … WITH TIES`, DISTINCT, `MIN(DISTINCT x)`/`MAX(DISTINCT x)` extrema semantics, NULL semantics, date/time and exact decimal literal paths across distributed fragments. | Broader differential corpus against the pinned Trino version. |
| CTEs and derived tables | Implemented | Non-recursive CTEs and derived relations bind through the normal optimizer. | Recursive CTE support remains open. |
| Joins and subqueries | Partial | Equi/residual joins, `JOIN ... USING (...)` for base relations (including dictionary-text and composite keys), correct inner/outer unqualified wildcard output with right/full-key coalescing, qualified wildcard output, EXISTS, IN/NOT IN, bounded correlated scalar aggregate/count forms, keyed correlated EXISTS/IN/NOT IN with residual predicates, and statically single-row scalar literals such as `SELECT (SELECT 42)` are supported. These bounded correlated shapes are covered by the local/distributed differential sweep and lower to semi/anti or grouped joins through the binder. USING is lowered to qualified equality keys; unqualified `*` emits one merged key while `relation.*` keeps only that relation's columns. Composite/lateral derived inputs and broader correlated forms remain open. | Correlated LIMIT/OFFSET/UNION and other non-keyed Apply forms, composite USING over derived/lateral inputs, and deeper nested/lateral shapes. |
| Set operations | Implemented | UNION, INTERSECT, EXCEPT, including ALL forms, with exact local/distributed tests. | Additional complex-type differential cases. |
| Windows and QUALIFY | Partial | ROW_NUMBER, RANK, DENSE_RANK, NTILE, PERCENT_RANK, CUME_DIST, NTH_VALUE, value-window `IGNORE NULLS` (including offset skipping for LAG/LEAD), aggregate windows, frames, and the qualified QUALIFY shape. | Full modifier and distributed-window corpus. |
| Arrays | Partial | Typed numeric, text, boolean, temporal, decimal, nested-list, and bounded array-function paths, including scalar-cast `ARRAY_JOIN`, one-based and negative `ELEMENT_AT` indexing for both regular and large Arrow lists. The current AKS image also passes the distributed `SELECT DISTINCT year ... ORDER BY year` regression that previously surfaced `unknown scalar function: ARRAY`. | Remaining complex element and full Trino function coverage. |
| Maps and rows | Partial | Typed ROW/MAP construction, lookup (including `Int32`, `UInt64`, and `LargeUtf8` map keys), equality, map entries/concat, nested materialization. | Broader dereference, ordered-comparison, and complex-type differential coverage. |
| JSON | Partial | JSON extraction, scalar extraction, `JSON_QUERY`/`JSON_VALUE` aliases, `JSON_EXISTS`, dotted and quoted-bracket paths over narrow and large UTF-8 values, array length/contains across supported integer, floating-point, and Decimal128 needles, size, canonical `JSON_FORMAT` over narrow and large UTF-8 values, zero-based/negative-index `JSON_ARRAY_GET`, and typed materialization, including `LargeUtf8` JSON parsing. `JSON_PARSE(...)` and the typed `JSON '…'` literal form are implemented and pass the bounded AKS wire probe on the current image. | Full coercion, complete JSON path semantics, and broader external-client differential coverage. |
| Row-producing UNNEST | Partial | `UNNEST(ARRAY[...], ...)` in `FROM` expands constant arrays locally and through the fragment planner; shorter arrays are null-padded to the longest input, `WITH ORDINALITY` is one-based, and BigQuery-compatible `WITH OFFSET` is zero-based. Both preserve their aliases after all array columns and have distributed differential coverage. Correlated/lateral inputs and arbitrary table-function expressions remain unsupported. | Correlated/lateral table-function semantics and the remaining Trino shape corpus. |
| Prepared statements | Qualified (bounded) | SQL PREPARE/EXECUTE/DEALLOCATE, Trino prepared headers, typed scalar/array/map binding, DESCRIBE INPUT/OUTPUT, ownership isolation; the current immutable AKS image passes the external wire lifecycle and exact distributed result gate. | Broader external-client and complex-type qualification remains open. |
| Parameters | Implemented (declared surface) | Positional, repeated positional, named, nested arrays and string-keyed maps bind without SQL interpolation. | Differential coverage from external Trino clients. |
| Native DML | Partial | Typed multi-row product-record DML and transaction visibility are supported on the declared product tables. | General typed lake-table DML, constraints, and broader transactional SQL. |
| Metadata protocol | Qualified (bounded) | SHOW SESSION, SHOW FUNCTIONS, catalog/schema/table metadata, EXPLAIN, response paging, cancellation links, and session update headers. The current AKS image passes an authenticated Trino-wire sweep for these controls with exact rows and drained pages. | Broader external-client compatibility and release-corpus coverage. |
| Explicit unsupported behavior | Qualified | Unsupported syntax fails closed with actionable SQL error names and no silent coordinator fallback; `TRY` now lowers JSON parse/format/array-get failures to NULL. | Keep rejection corpus in every release gate. |

## Qualification commands

Run from `engine/`:

```powershell
cargo test --workspace --no-fail-fast --quiet
cargo test -p kaveon-server trino_ --quiet
cargo test -p kaveon-server --bin kaveon-server differential_tests --quiet
```

Run from the repository root:

```powershell
node scripts/validate-docs.mjs
```

A parity claim requires the exact-result reference, workload inputs, image or
commit digest, latency/resource measurements, and failure count to be retained
under `docs/qualification/`. Passing focused tests alone does not qualify a
three-worker, cloud, or broad federated claim.
