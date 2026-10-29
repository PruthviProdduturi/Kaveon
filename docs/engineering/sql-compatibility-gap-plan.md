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

- [ ] Create a versioned T-SQL corpus covering `TOP`, bracket identifiers,
  `DATEADD`, `DATEDIFF`, `DATEPART`, `CONVERT`, `TRY_CONVERT`, `ISNULL`,
  `GETDATE`, `SYSDATETIME`, `STRING_AGG`, `PIVOT`, `UNPIVOT`, and SQL Server
  pagination.
- [ ] Classify every case as accepted, translated, or fail-closed with an
  actionable diagnostic.
- [ ] Add exact local and distributed result tests for accepted translations.
- [ ] Keep `TOP PERCENT`, `TOP WITH TIES`, `PIVOT`, `UNPIVOT`, and unsupported
  `APPLY` forms rejected until their semantics are implemented and qualified.
- [ ] Add Fabric/Azure SQL passthrough tests without implying Engine execution.

Progress: the Engine now lowers `ISNULL`, `DATEADD`, `DATEDIFF`, `DATEPART`,
unstyled `CONVERT`, and unstyled `TRY_CONVERT`, including common SQL Server
datepart abbreviations. The focused planner regression passes; execution and
distributed corpus evidence are still required before this becomes a fully
qualified compatibility claim.

### Phase 2 — Transactional SQL surface

- [ ] Extend typed product DML to general typed KaveonDB tables.
- [ ] Add source-backed `INSERT … SELECT` with bounded plans.
- [ ] Add `MERGE`, conflict handling, `RETURNING`, and savepoints only with
  explicit revision/CAS semantics.
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

