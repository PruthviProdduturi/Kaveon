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
  `GETDATE`, `SYSDATETIME`, `STRING_AGG`, `PIVOT`, `UNPIVOT`, and SQL Server
  pagination.
- [x] Classify every case as accepted, translated, or fail-closed with an
  actionable diagnostic.
- [ ] Add exact local and distributed result tests for accepted translations.
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

### Phase 2 — Transactional SQL surface

- [ ] Extend typed product DML to general typed KaveonDB tables.
- [ ] Add source-backed `INSERT … SELECT` with bounded plans.
- [ ] Add `MERGE` and broader conflict handling only with explicit revision/CAS
  semantics. Product `ON CONFLICT DO NOTHING` is now bounded to the product id
  and covered by parser and API tests; `DO UPDATE`, named constraints, and
  arbitrary conflict targets remain rejected. Product `RETURNING id, revision`
  is implemented from the staged revision/CAS write set. Savepoints are
  implemented for the product transaction session and covered by parser,
  catalog, and API tests.
- [ ] Add durable journal/recovery, snapshot isolation, and restart tests.
- [ ] Add constraints, indexes, and catalog-backed schema enforcement.
- [ ] Publish a supported transaction/isolation matrix rather than claiming
  PostgreSQL equivalence.

### Phase 3 — Optimizer and statistics

- [ ] Persist column distinct counts, histograms, null fractions, and bounds.
- [ ] Use statistics for join ordering, broadcast choice, selectivity, and
  partition pruning.
- [ ] Add plan-vs-runtime diagnostics and regression thresholds.
- [ ] Qualify result-cache behavior separately from data-cache behavior.

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

