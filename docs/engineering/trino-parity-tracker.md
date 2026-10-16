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
| Distributed joins | Partial | Partitioned and broadcast hash joins, residuals, outer joins, semi/anti joins, measured local join spill, bounded AKS outer-join/skew evidence, a clean-baseline AKS fault/pressure rerun with forced worker-loss retry and exact recovery, and an exact-repeat bounded high-cardinality self-join over the 504M-row table. | AKS disk-exhaustion, larger high-cardinality joins, and sustained mixed-workload qualification. |
| Distributed aggregates | Implemented | Partial/final aggregation, exact distinct, AVG, grouped spill, adaptive partials, hybrid final merge, and a current-image five-run full-table grouped aggregate with exact repeated results across all three workers (observed p95 26.453s). | Larger-scale AKS pressure, skew behavior, and a broader published latency SLO. |
| Windows | Partial | Distributed row-number and compatible partitioned windows are qualified; the current image passes an exact-repeat partitioned `ROW_NUMBER()` plus `QUALIFY` gate across three workers. Mixed-key and unpartitioned windows remain centralized for correctness. | Parallel execution for the remaining window shapes. |
| Global ORDER BY and TopN | Partial | Inputs distribute and a coordinator merge preserves exact global ordering; ordinal keys that survive wildcard expansion are rebound to the final output scope, generic `ORDER BY … LIMIT` is applied only after the global merge, `FETCH FIRST/NEXT … WITH TIES` preserves the boundary peer set, and already-sorted integer worker candidates use a bounded k-way merge fast path. The current image passes an exact-repeat multi-column `NULLS LAST` + `OFFSET`/`LIMIT` gate across three workers. | Fully parallel ordered result stream without a coordinator bottleneck. |
| Memory admission and spill | Implemented | Per-query reservations, resource groups, bounded spill, cancellation cleanup, and fail-closed limits. | Five-minute mixed-workload soak and cloud spill evidence. |
| Cancellation and retry | Qualified (bounded) | Client cancellation and retry propagate through stages and exchange cleanup. | Long-running federated cancellation under pressure. |
| Worker-loss recovery | Qualified (bounded) | Replacement-worker recovery preserves exact results and snapshot pinning for the declared three-worker gate. | Node-drain and repeated-loss endurance. |
| Coordinator restart | Qualified (bounded) | Idle and active-query cleanup are fail-closed; an in-flight query is interrupted safely, workers reregister, and the exact post-restart probe passes on the declared topology. | Multi-coordinator/failover survival with in-flight query continuity. |
| Snapshot consistency | Implemented | Fragments pin catalog and Delta versions for a statement and its retries. | Cross-coordinator recovery qualification. |
| Runtime dynamic filtering | Partial | Build-side filters reduce probe work before hashing; eligible single-key and composite-key (independent safe `IN` projections) Delta scans, Iceberg manifest scans, local Parquet file/directory scans, ADLS single-object readers, and object-directory scans accept build-side predicates before decoding; the operator interface applies every safe conjunction child before falling back. Composite predicates now reach the composite storage branch, NULL build keys are excluded from the safe projections, and partitioned execution has an exact-result composite-key regression. Directory partition values are folded first, so matching files are pruned and physical-column residuals reach the decoder. Object Delta sources retain pinned add-action statistics, and Iceberg sources retain manifest facts, so a late filter can prune remaining files before they open. ADLS single-object readers defer lane/decoder startup until the first batch, allowing a late predicate to reach row-group and lane construction. Query scan telemetry exposes accepted runtime-filter count and value count, with focused object-Delta, Iceberg, and ADLS regressions; the local ADLS evidence is retained in `track1-adls-runtime-filter-local-2026-10-15.md`. The deployed image passes an exact-repeat three-worker Delta join probe with three accepted filters carrying four values per run; the evidence is in `track1-aks-runtime-filter-join-current-2026-10-16.md`. | Qualify measurable file/row-group pruning on representative cloud datasets; remaining type/exchange edges and cloud snapshot qualification remain open. |
| Join reordering and skew mitigation | Partial | Statistics-aware planning can reorder eligible inner equi-joins, preserve the original output order through an explicit projection, and choose a bounded broadcast build; skew splitting and a general cost-based join enumeration policy are not implemented. | Skew policy, multi-join cost model, and plan-regression corpus. |
| Connector federation | Partial | Native Parquet/Delta/Iceberg paths exist; registered SQL sources are passthrough API paths. | Authenticated multi-source pushdown and cross-source execution qualification. |
| Upgrade, rollback, and sustained soak | Missing | Deployment artifacts exist; the full operational acceptance run is not complete. | Upgrade/rollback rehearsal and sustained mixed workload. |

The current AKS wire corpus is retained in `docs/qualification/track2-aks-wire-corpus-fetch-percent-current-2026-10-16.md` and its raw JSON. All twelve declared SQL shapes passed twice with identical results on the current immutable `sha256:60de4cf5…` Engine image, with three catalog-compatible workers. The new literal `FETCH FIRST 50 PERCENT ROWS ONLY` case returned 12 rows on both runs. The eleven-shape lateral report remains available for history.

## Track 2 — SQL compatibility and types

| Capability | State | Current contract | Acceptance evidence still required |
|---|---|---|---|
| Core SELECT surface | Implemented | WHERE, projection (including scalar SELECT statements without `FROM`, top-level and derived `VALUES` relations with column aliases, and typed DATE/TIME literal results), CASE, casts, GROUP BY, `COUNT_IF`, aggregate `FILTER (WHERE ...)` for supported aggregate functions (lowered to null-preserving inputs), bounded GROUPING SETS/CUBE/ROLLUP lowering with `GROUPING()`/`GROUPING_ID()`, HAVING, ORDER BY (including select ordinals), LIMIT/OFFSET, `FETCH … WITH TIES` and literal `FETCH … PERCENT`, DISTINCT, `MIN(DISTINCT x)`/`MAX(DISTINCT x)` extrema semantics, NULL semantics, date/time and exact decimal literal paths across distributed fragments. The current AKS image passes exact-repeat wire coverage for FILTER, GROUPING SETS, FETCH WITH TIES, and literal FETCH PERCENT. | Broader differential corpus against the pinned Trino version. |
| CTEs and derived tables | Implemented | Non-recursive CTEs and derived relations bind through the normal optimizer; the current AKS image passes exact-repeat CTE wire coverage. | Recursive CTE support remains open. |
| Joins and subqueries | Partial | Equi/residual joins, `JOIN ... USING (...)` for base relations (including dictionary-text and composite keys), correct inner/outer unqualified wildcard output with right/full-key coalescing, qualified wildcard output, EXISTS, IN/NOT IN, bounded correlated scalar aggregate/count forms, keyed correlated EXISTS/IN/NOT IN with residual predicates, positive top-level `LIMIT` (including keyed correlated `IN`/`NOT IN`, where membership is existential, ordered membership with hidden sort projections, ungrouped correlated scalar aggregates, and one-column non-aggregate scalar relations), one-column scalar `LIMIT 0` null-row lowering, positive `OFFSET` over ungrouped scalar aggregates as NULL, zero-row `LIMIT 0` for correlated `IN`/`NOT IN` empty-set lowering, no-op `OFFSET 0`, bounded positive `OFFSET` with one or more equality correlations and no residual inside correlated `EXISTS`/`NOT EXISTS`, and one-column non-aggregate scalar `OFFSET … LIMIT` (ordered and unordered) forms, plus statically single-row scalar literals such as `SELECT (SELECT 42)` are supported. These bounded correlated shapes are covered by the local/distributed differential sweep and lower to semi/anti or grouped joins through the binder. USING is lowered to qualified equality keys; unqualified `*` emits one merged key while `relation.*` keeps only that relation's columns. Composite/lateral derived inputs and broader correlated forms remain open. | Positive correlated `OFFSET` with residual correlations, multi-column scalar limits, unbounded or `OFFSET`-only non-aggregate scalar forms, `UNION` and other non-keyed Apply forms, composite USING over derived/lateral inputs, and deeper nested/lateral shapes. |
| Set operations | Implemented | UNION, INTERSECT, EXCEPT, including ALL forms, with exact local/distributed tests. | Additional complex-type differential cases. |
| Windows and QUALIFY | Partial | ROW_NUMBER, RANK, DENSE_RANK, NTILE, PERCENT_RANK, CUME_DIST, NTH_VALUE, value-window `IGNORE NULLS` (including offset skipping for LAG/LEAD), aggregate windows, frames, and the qualified QUALIFY shape; the current AKS image passes exact-repeat partitioned frame and partitioned `ROW_NUMBER()` plus `QUALIFY` wire cases. | Full modifier and distributed-window corpus. |
| Arrays | Partial | Typed numeric, text, boolean, temporal, decimal, nested-list, and bounded array-function paths, including scalar-cast `ARRAY_JOIN`, null-aware `ARRAYS_OVERLAP`, one-based and negative `ELEMENT_AT` indexing for both regular and large Arrow lists. The current AKS image also passes the distributed `SELECT DISTINCT year ... ORDER BY year` regression that previously surfaced `unknown scalar function: ARRAY`. | Remaining complex element and full Trino function coverage. |
| Maps and rows | Partial | Typed ROW/MAP construction, lookup (including `Int32`, `UInt64`, and `LargeUtf8` map keys), equality, map entries/concat, nested materialization. | Broader dereference, ordered-comparison, and complex-type differential coverage. |
| JSON | Partial | JSON extraction, scalar extraction, `JSON_QUERY`/`JSON_VALUE` aliases, `JSON_EXISTS`, dotted and quoted-bracket paths over narrow and large UTF-8 values, array length/contains across supported integer, floating-point, and Decimal128 needles, size, canonical `JSON_FORMAT` over narrow and large UTF-8 values, zero-based/negative-index `JSON_ARRAY_GET`, and typed materialization, including `LargeUtf8` JSON parsing. `JSON_PARSE(...)` and the typed `JSON '…'` literal form are implemented and pass the bounded AKS wire probe on the current image, including exact-repeat `JSON_VALUE` and array-length checks. | Full coercion, complete JSON path semantics, and broader external-client differential coverage. |
| Row-producing UNNEST | Partial | `UNNEST(ARRAY[...], ...)`, `SEQUENCE(start, stop[, step])`, and the supported array-valued functions `ARRAY_CONCAT`, `ARRAY_DISTINCT`, `ARRAY_FLATTEN`, `ARRAY_REPEAT`, `ARRAY_REVERSE`, `ARRAY_SLICE`, `ARRAY_SORT`, `ARRAY_ZIP`, `MAP_KEYS`, `MAP_VALUES`, and `MAP_ENTRIES` in `FROM` expand constant arrays locally and through the fragment planner; `MAP_ENTRIES` row elements and `ARRAY[ROW(...)]` elements can be aliased into separate key/value or struct-field columns. The equivalent constant `CROSS JOIN LATERAL UNNEST(...)` function form is also lowered. `CROSS JOIN LATERAL UNNEST(ARRAY[row.array_col, ...])` and correlated `SEQUENCE` expressions evaluate each array against each input batch, repeat left rows, null-pad multiple arrays to the longest input, and preserve one-based `WITH ORDINALITY` or zero-based `WITH OFFSET` (using the dialect-supported offset alias form). The parser routes correlated lateral UNNEST through its table-shaped AST, so ordinality and offset are retained. Local and distributed exact-result regressions cover correlated arrays, correlated sequences, array-function input, map-key and map-entry input, ordinality, offset, and NULL padding. Arbitrary table-function expressions and broader nested/lateral shapes remain unsupported. | Correlated/lateral table-function combinations beyond the allow-listed array expressions and the remaining Trino shape corpus. |
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




