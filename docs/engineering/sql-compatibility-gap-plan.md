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
  `GETDATE`, `SYSDATETIME`, bounded `DATE_BUCKET`, `PIVOT`, `UNPIVOT`, and `CROSS APPLY`,
  explicit unsupported APPLY forms, and SQL Server pagination.
- [x] Classify every case as accepted, translated, or fail-closed with an
  actionable diagnostic.
- [x] Add exact local and distributed result tests for accepted translations.
  The current 90-case corpus has planner coverage and focused exact
  three-worker differential evidence; the versioned corpus and distributed
  evidence are retained in
  `docs/qualification/tsql-read-compatibility-distributed-2026-10-28.md`.
- [ ] Keep unbounded `PIVOT`, `UNPIVOT`, and unsupported `APPLY` forms rejected
  until their semantics are implemented and qualified. Bounded derived-source
  `PIVOT`, `UNPIVOT`, and `CROSS APPLY` lower to conditional aggregation,
  `UNION ALL`, or the existing lateral path and are qualified across local
  layouts and three-worker fragments; see the corresponding qualification
  records under `docs/qualification/`. Unbounded `PIVOT`/`UNPIVOT`, arbitrary
  correlated APPLY, and table-valued APPLY remain explicitly rejected.
  bounded derived-source `OUTER APPLY` is also qualified through the existing
  left-lateral path, including NULL extension. Unbounded table-valued APPLY,
  arbitrary correlated APPLY, and unbounded PIVOT/UNPIVOT remain explicitly
  rejected. `TOP PERCENT` and `TOP WITH
  TIES` are qualified only when their `ORDER BY` keys define a deterministic
  order.
- [x] Add Fabric/Azure SQL passthrough routing tests without implying Engine
  execution. Native Engine catalog resolution and Fabric/Azure SQL passthrough
  separation are qualified in
  `docs/qualification/track1-fabric-passthrough-routing-2026-10-29.md`.

Progress: the Engine now lowers bounded `TOP n PERCENT`, `TOP n WITH TIES`, `ISNULL`, `CONCAT`, `CONCAT_WS`, `CHOOSE`, character-set `TRIM` in BOTH/LEADING/TRAILING modes, `DATEADD`, `DATEDIFF`, `DATEPART`, and common SQL Server clock functions,
unstyled `CONVERT`, and unstyled `TRY_CONVERT`, including common SQL Server
datepart abbreviations. Runtime DATE and timestamp styles have qualified local
and three-worker coverage for the declared style set. Literal DATE styles `0`, `1`, `101`, and `112` now
normalize to ISO values. `ISNULL`, `CONCAT`, `CONCAT_WS`, `CHOOSE`, `DATEADD`, `DATEDIFF`, and `DATEPART` now
pass exact local-vs-three-worker differential checks across dictionary, plain,
partitioned, clustered, and fragmented Parquet layouts. `TOP PERCENT` and
`TOP WITH TIES` also pass that sweep when their order keys are deterministic;
without a complete tie-breaker, SQL permits different tied rows and the
qualification is intentionally withheld. Numeric `CONVERT` and
`TRY_CONVERT` casts are also qualified across those layouts. SQL Server `COUNT_BIG`
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
Bracket-quoted identifiers remain identifiers in `DISTINCT` projections and
ordered pagination shapes; the local/three-worker evidence across Parquet
encodings is retained in
`docs/qualification/track2-tsql-bracket-identifiers-2026-10-29.md`.
`QUOTENAME` now supports the bounded SQL Server identifier quoting surface
(square brackets and double quotes, 128-Unicode-character limit) with local
and three-worker evidence in
`docs/qualification/track2-tsql-quotename-2026-10-29.md`.
`REPLICATE` now canonicalizes to the bounded Unicode `REPEAT` path for
non-negative literals and bounded row-local BIGINT expressions, returning NULL
for negative counts; it is included in the distributed corpus.
`LEFT` and `RIGHT` preserve SQL Server's Unicode scalar character semantics
for bounded text lengths and are covered by the distributed differential
regression.
`REVERSE` preserves Unicode scalar order for text values and is covered by the
same distributed differential path.
`TRIM`, `LTRIM`, and `RTRIM` preserve NULLs and remove only the documented
ASCII-space boundaries on the width-aware text path. Character-set `TRIM`
now qualifies BOTH, LEADING, and TRAILING forms with local and three-worker
differential evidence in `docs/qualification/track2-tsql-trim-characters-2026-10-29.md`
and `docs/qualification/track2-tsql-trim-directional-characters-2026-10-29.md`.
`TRANSLATE` now has a bounded SQL Server-compatible Unicode scalar mapping
path, including NULL propagation and first-match/extra-character semantics,
and is covered by the versioned planner corpus and three-worker differential
gate. `DATE_BUCKET` buckets for year, quarter, month, day, hour, minute, and
second lower to the existing typed `DATE_TRUNC` path and have exact
local/distributed coverage (see `docs/qualification/track2-tsql-date-bucket-2026-10-29.md`); row-local and
typed-literal custom origins are qualified, while unsupported origin shapes
and dateparts remain explicitly rejected. Row-local BIGINT widths are guarded
at runtime: zero, negative, or NULL widths return typed NULL.
SQL Server `FORMAT` now lowers to the bounded temporal formatter for typed
DATE/TIMESTAMP values. Invariant patterns cover the common `yyyy`, `MM`, `dd`,
`HH`, `mm`, `ss`, and fractional-second tokens; numeric and culture-specific
overloads remain rejected. The same evaluator now honors the supplied pattern
for `DATE_FORMAT`/`TO_CHAR`, with exact local and three-worker evidence in
`docs/qualification/track2-tsql-format-temporal-2026-10-29.md`.

`ASCII`, `UNICODE`, and bounded `CHAR` are also qualified: ASCII returns the
leading ASCII scalar and NULL for a non-ASCII leading scalar, UNICODE returns
the leading Unicode scalar, and CHAR accepts only integer byte values 0..255,
returning NULL outside that range. This is an explicit invariant UTF-8/byte
contract, not locale/code-page emulation; local and three-worker evidence is
recorded in `docs/qualification/track2-tsql-character-code-2026-10-29.md`.

### Phase 2 — Transactional SQL surface

- [x] Qualify bounded typed-row DML for KaveonDB-owned tables. The
  `kaveon.product.typed_rows` facade supports revisioned create/update/delete
  with owner isolation and replay-safe `ON CONFLICT DO NOTHING`, plus the
  bounded `ON CONFLICT (id) DO UPDATE SET document_json = excluded.document_json`
  form with table-identity preservation. The same upsert is exercised through
  `/v1/transaction/sql` with `RETURNING id, revision` and a committed
  revision/CAS follow-up. Bounded `ON CONFLICT DO NOTHING RETURNING id,
  revision, document_json` is also qualified: duplicates return no rows and
  stage no write. Arbitrary typed-table schemas and conflict updates
  remain outside this contract. Evidence:
  `docs/qualification/track2-native-dml-typed-row-conflict-ignore-2026-10-28.md`.
- [x] Add bounded source-backed product `INSERT … SELECT`. The qualified form
  copies `id` and `document_json` from one same-kind `kaveon.product` table
  under the pinned transaction snapshot, optionally narrowed by a literal
  `id IN (...)` list, and is capped at 1,000 source records,
  enforces owner isolation, and reuses the existing conflict/CAS path. With an
  explicit ID filter, `RETURNING id, revision, document_json` is also qualified
  from the staged snapshot. General lake-table source DML and unbounded
  source-copy returning remain open.
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

- [x] Persist source-version-bound row counts, bytes, distinct sketches, null
  fractions, quantiles, and bounds for supported Parquet/Delta/Iceberg paths.
  Evidence: `docs/qualification/track3-statistics-planning-boundary-2026-10-28.md`.
- [x] Add a bounded persisted numeric histogram model derived from the
  source-version-bound KLL profile. Histogram boundaries/counts are used for
  range selectivity when present; text and metadata-only columns remain
  bounds/quantile-only. Evidence:
  `docs/qualification/track3-statistics-histogram-2026-10-29.md`.
- [x] Use qualified statistics for bounded local join ordering, broadcast
  choice, selectivity, and reader pruning. Cloud pruning and universal
  cost-based planning remain separate gates; see
  `docs/qualification/track3-statistics-planning-boundary-2026-10-28.md`.
- [x] Add bounded plan-vs-runtime diagnostics. The query-level estimate-vs-actual
  endpoint now compares source-row estimates with finished scan telemetry and
  exposes ordinal per-scan attribution when scan cardinalities match exactly;
  broader per-operator attribution remains open. Runtime-only comparison now exposes a
  conservative doubled-P95 regression threshold after five comparable runs;
  evidence: `docs/qualification/track3-runtime-regression-threshold-2026-10-29.md`;
  optimized and physical scan plans now expose versioned source estimates;
  evidence: `docs/qualification/track3-plan-estimate-visibility-2026-10-29.md`;
  estimate-vs-actual evidence: `docs/qualification/track3-estimate-vs-actual-query-diagnostic-2026-10-29.md`.
- [ ] Extend estimate-vs-actual diagnostics to every physical operator and
  selectivity boundary; this remains open.
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

