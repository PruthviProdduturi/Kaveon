# SQL compatibility gap plan

This is the implementation tracker for closing the gap between Kaveon's
current analytical SQL surface and the read, transactional, and operational
capabilities users commonly expect from Fabric Warehouse, Azure SQL,
PostgreSQL, and Trino.

It is deliberately evidence-based: a capability moves to **qualified** only
after parser, planner, local execution, distributed execution (where
applicable), and result-correctness tests pass. Documentation or parser
acceptance alone is not a parity claim.

## Current boundary

Kaveon Engine is a distributed, columnar lake query engine with a bounded
T-SQL compatibility layer. Fabric/Azure SQL registered sources are API
passthrough sources; they are not executed by Kaveon Engine and cannot be
joined with Engine catalogs in one query today. KaveonDB transactions are a
bounded product-record contract, not a PostgreSQL wire-compatible row store.

The authoritative capability details remain in:

- [Engine SQL compatibility](../reference/engine-sql-compatibility.md)
- [Kaveon–Trino parity tracker](trino-parity-tracker.md)
- [Connector capabilities](../reference/connector-capabilities.md)
- [Fabric comparison](../research/kaveon-engine-vs-fabric-sql-analytics-endpoint.md)

## Closure order

### Phase 1 — T-SQL/Fabric read compatibility

- [x] Create a versioned T-SQL corpus covering `TOP`, bracket identifiers,
  `DATEADD`, `DATEDIFF`, `DATEPART`, `CONVERT`, `TRY_CONVERT`, `ISNULL`,
  `GETDATE`, `SYSDATETIME`, `PIVOT`, `UNPIVOT`, and SQL Server
  pagination.
- [x] Classify every case as accepted, translated, or fail-closed with an
  actionable diagnostic.
- [x] Add exact local and distributed result tests for accepted translations.
  The current 37-case corpus has planner coverage and a completed exact
  three-worker differential sweep; evidence is retained in
  `docs/qualification/tsql-read-compatibility-distributed-2026-10-28.md`.
- [ ] Keep `PIVOT`, `UNPIVOT`, and unsupported `APPLY` forms rejected until
  their semantics are implemented and qualified. `TOP PERCENT` and `TOP WITH
  TIES` are qualified only when their `ORDER BY` keys define a deterministic
  order.
- [ ] Add Fabric/Azure SQL passthrough tests without implying Engine execution.

Progress: the Engine now lowers bounded `TOP n PERCENT`, `TOP n WITH TIES`, `ISNULL`, `DATEADD`, `DATEDIFF`, `DATEPART`, and common SQL Server clock functions,
unstyled `CONVERT`, and unstyled `TRY_CONVERT`, including common SQL Server
datepart abbreviations. Literal DATE styles `0`, `1`, `101`, and `112` now
normalize to ISO values. `ISNULL`, `DATEADD`, `DATEDIFF`, and `DATEPART` now
pass exact local-vs-three-worker differential checks across dictionary, plain,
partitioned, clustered, and fragmented Parquet layouts. `TOP PERCENT` and
`TOP WITH TIES` also pass that sweep when their order keys are deterministic;
without a complete tie-breaker, SQL permits different tied rows and the
qualification is intentionally withheld. Numeric `CONVERT` and
`TRY_CONVERT` casts are also qualified across those layouts; date-style
runtime-column conversion remains explicitly rejected. SQL Server `COUNT_BIG`
now canonicalizes to the Engine's mergeable 64-bit `COUNT` state; it shares
the existing distributed count semantics and does not claim a separate
SQL Server numeric type. `LEN` is qualified with SQL Server's trailing-space
rule, `IIF` uses the existing vectorized conditional path, and `CHARINDEX`
uses a bounded Unicode scalar search with SQL Server's argument order.
`EOMONTH` now lowers to typed month truncation and calendar arithmetic and is
qualified for date/timestamp inputs with an optional row-local month offset.
`DATEFROMPARTS` now builds validated `DATE` values from integer parts with
NULL propagation and distributed differential coverage.
`DATENAME` now returns deterministic English calendar names or textual numeric
parts and is covered by the distributed differential sweep; locale-specific
formatting remains outside the contract. The complete bounded corpus and its
distributed evidence are recorded in
`docs/qualification/tsql-read-compatibility-distributed-2026-10-28.md`.
`ISDATE` now returns a safe boolean validity result for text and temporal
inputs, explicitly refusing numeric epoch coercion.
`ISNUMERIC` now returns a safe boolean result for native numeric and
parseable-text inputs without claiming locale-specific currency parsing.
`PATINDEX` now supports bounded `%` and `_` wildcard matching with Unicode
scalar positions, NULL propagation, and explicit rejection of bracket classes;
the local and three-worker evidence is retained in
`docs/qualification/track2-tsql-patindex-2026-10-28.md`.
`LEFT` and `RIGHT` preserve SQL Server's Unicode scalar character semantics
for bounded text lengths and are covered by the distributed differential
regression.
`REVERSE` preserves Unicode scalar order for text values and is covered by the
same distributed differential path.
`TRIM`, `LTRIM`, and `RTRIM` preserve NULLs and remove only the documented
ASCII-space boundaries on the width-aware text path; they are covered by the
same distributed differential regression.
`TRANSLATE` now has a bounded SQL Server-compatible Unicode scalar mapping
path, including NULL propagation and first-match/extra-character semantics,
and is covered by the versioned planner corpus and three-worker differential
gate.

### Phase 2 — Transactional SQL surface

- [x] Qualify bounded typed-row DML for KaveonDB-owned tables. The
  `kaveon.product.typed_rows` facade supports revisioned create/update/delete
  with owner isolation and replay-safe `ON CONFLICT DO NOTHING`; arbitrary
  typed-table schemas and conflict updates remain outside this contract. Evidence:
  `docs/qualification/track2-native-dml-typed-row-conflict-ignore-2026-10-28.md`.
- [x] Add bounded source-backed product `INSERT … SELECT`. The qualified form
  copies `id` and `document_json` from one same-kind `kaveon.product` table
  under the pinned transaction snapshot, optionally narrowed by a literal
  `id IN (...)` list, and is capped at 1,000 source records,
  enforces owner isolation, and reuses the existing conflict/CAS path. General
  lake-table source DML remains open.
- [x] Add bounded `MERGE` with explicit revision/CAS semantics. `MERGE`
  accepts one aliased `VALUES` batch of up to 1,000 rows containing `id` and
  `document_json`, an equality `target.id = source.id` predicate, and exactly
  `WHEN MATCHED THEN UPDATE SET document_json = source.document_json` followed
  by `WHEN NOT MATCHED THEN INSERT (id, document_json) VALUES (source.id,
  source.document_json)`. Each source row lowers to the existing
  owner-isolated `Upsert` command, so commit/revision behavior is shared with
  the qualified conflict upsert path. Evidence:
  `docs/qualification/track2-native-dml-merge-2026-10-28.md`.
- [ ] Expand `MERGE` and broader conflict handling only with explicit
  revision/CAS semantics. Product `ON CONFLICT DO NOTHING` and the bounded
  `ON CONFLICT (id) DO UPDATE SET document_json = excluded.document_json`
  upsert are now covered by parser and API tests; named constraints, conflict
  predicates, arbitrary assignments, and arbitrary conflict targets remain
  rejected. Product `RETURNING id, revision`
  is implemented from the staged revision/CAS write set. Savepoints are
  implemented for the product transaction session and covered by parser,
  catalog, and API tests.
- [x] Qualify bounded snapshot isolation and transaction-session restart
  behavior for KaveonDB-owned product records; see
  `docs/qualification/track2-transaction-isolation-matrix-2026-10-28.md`.
- [ ] Add a durable journal/recovery qualification for cloud/object-store
  failures and multi-coordinator restart. This remains distinct from the
  bounded snapshot contract above.
- [x] Qualify primary/unique indexes, typed-schema validation, and bounded
  product-reference enforcement for KaveonDB-owned typed rows. Evidence:
  `docs/qualification/track2-typed-schema-integrity-2026-10-28.md`.
- [ ] Add general SQL constraint/index DDL and catalog-backed enforcement for
  arbitrary user tables and lake-table mutation.
- [x] Publish a supported transaction/isolation matrix rather than claiming
  PostgreSQL equivalence. The bounded product-record contract is recorded in
  `docs/qualification/track2-transaction-isolation-matrix-2026-10-28.md`.

### Phase 3 — Optimizer and statistics

- [ ] Persist column distinct counts, histograms, null fractions, and bounds.
- [ ] Use statistics for join ordering, broadcast choice, selectivity, and
  partition pruning.
- [ ] Add plan-vs-runtime diagnostics and regression thresholds.
- [x] Qualify result-cache behavior separately from data-cache behavior. The
  result cache is keyed by normalized SQL, parameters, catalog snapshot,
  pinned source versions, and time zone; it reports cache execution mode,
  excludes paged results, honors per-statement bypass, applies bounded
  TTL/byte/LRU eviction, and is cleared on catalog publication and product
  transaction commit/replay. Evidence:
  `docs/qualification/track3-result-cache-boundary-2026-10-28.md`.

### Phase 4 — Distributed completeness

- [ ] Finish adaptive joins and aggregate exchange for high-cardinality data.
- [ ] Distribute remaining window and global-sort shapes.
- [ ] Complete correlated/lateral execution or maintain explicit rejection.
- [ ] Qualify object-store retries, spill exhaustion, cancellation, and mixed
  workload endurance on every release image.

### Phase 5 — Federation and client compatibility

- [ ] Add a connector execution abstraction with one-source pushdown first.
- [ ] Add cross-source joins only after snapshot, type, auth, and failure
  semantics are defined.
- [ ] Qualify JDBC/ODBC and external Trino clients against the documented wire
  surface.
- [ ] Add system metadata views with role-filtered visibility.

## Release gate

No phase is complete until its corpus has exact-result evidence, bounded-memory
and cancellation tests, a clean workspace gate, and updated compatibility
documentation. Claims must identify whether they are Engine execution,
registered-source passthrough, or KaveonDB transaction behavior.

