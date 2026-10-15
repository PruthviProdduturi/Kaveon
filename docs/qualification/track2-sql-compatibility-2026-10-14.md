# Track 2 SQL compatibility qualification — 2026-10-14

**Scope:** local SQL/parser/optimizer/executor regression on the current
working tree. This is semantic qualification, not a claim of complete Trino
wire or feature parity.

## Validation

From `engine/`:

```powershell
cargo test --workspace --no-fail-fast --quiet
```

The complete workspace run passed with zero failures. The run included the
server differential sweep (388 tests, 4 ignored benchmark cases), SQL parser
and binder coverage, optimizer tests, execution and storage tests, transaction
and CLI tests, and the remaining workspace targets.

From the repository root:

```powershell
node scripts/validate-docs.mjs
```

Documentation validation passed: 156 Markdown files, 39 routes, and 9 SVGs.

## Current qualified local surface

The current tests cover the implemented Trino-shaped surface including core
SELECT semantics, joins and semi/anti joins, CTEs and derived relations, set
operations, grouping sets, windows and QUALIFY, arrays/maps/rows, JSON paths,
prepared statements and typed parameters, metadata statements, global ordering
and limits, and local/distributed exact-result execution.

## Explicit remaining boundaries

The full Track 2 claim remains open. The compatibility reference still records
recursive CTEs, broader correlated/lateral subqueries, unsupported complex-type
combinations, broad lake-table DML, full external-client prepare/describe/bind
interoperability, and the remaining distributed-window shapes as open. Those
boundaries are rejected or documented explicitly; they are not counted as
implemented parity.

This report is local evidence. AKS workload and external Trino-client
qualification are separate gates in `docs/engineering/trino-parity-tracker.md`.

## Scalar-query follow-up

Commit `b6a80ec5` adds the missing Trino-compatible implicit single-row
relation for scalar `SELECT` statements without `FROM`. The focused SQL suite
passes 148 tests, the server suite passes 388 tests, and a rebuilt Docker
coordinator returned `SELECT CAST(1 AS BIGINT) AS value` as one typed `Int64`
row and `SELECT 1 UNION ALL SELECT 2 ORDER BY n DESC` as `2, 1`. The full
workspace gate was rerun after the change and all targets passed. This closes
that specific compatibility gap; recursive CTEs, broader lateral forms, and
the other boundaries in the matrix remain open.
