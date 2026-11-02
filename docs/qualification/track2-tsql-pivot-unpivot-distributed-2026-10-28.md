# Bounded T-SQL PIVOT and UNPIVOT qualification — 2026-10-28

## Contract

Kaveon qualifies two bounded SQL Server/Fabric relational shapes:

- `PIVOT` over a derived source with simple projected columns, one supported
  mergeable aggregate (`SUM`, `AVG`, `MIN`, `MAX`, or `COUNT`, including
  `COUNT(*)`), and a static `IN` list of at most 128 values. It lowers to
  conditional aggregation.
- `UNPIVOT` over a derived source with simple projected columns and a static
  `IN` list of at most 128 source columns. It lowers to `UNION ALL` branches.

An outer `ORDER BY` is retained by both rewrites. The ordered PIVOT case is
qualified below; UNPIVOT is intentionally qualified only for unordered row
equivalence because its current UNION execution does not yet provide a stable
cross-worker ordering contract.

The rewrite is bounded before parsing; arbitrary source expressions, dynamic
lists, unbounded table shapes, and `APPLY` remain fail-closed.

## Evidence

The existing differential sweep was run with `KAVEON_DIFF_ONLY` for each case:

- `tsql_bounded_pivot`
- `tsql_bounded_unpivot`

Each case returned exact matching rows across dictionary, plain, split-file,
partitioned, Hive-order, and clustered Parquet layouts, plus the in-process
three-worker fragment path for dictionary, partitioned, and clustered layouts.
Both runs completed with no worker failures, no mismatches, and no retained
memory reservations.

The current distributed corpus uses `ORDER BY region` for the bounded PIVOT
case and marks it qualified. The UNPIVOT corpus case remains unordered and
does not claim ordered-result parity.
