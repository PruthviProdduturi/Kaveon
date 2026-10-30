# Bounded T-SQL PIVOT and UNPIVOT qualification — 2026-10-28

## Contract

Kaveon qualifies two bounded SQL Server/Fabric relational shapes:

- `PIVOT` over a derived source with simple projected columns, one supported
  mergeable aggregate (`SUM`, `AVG`, `MIN`, `MAX`, or `COUNT`, including
  `COUNT(*)`), and a static `IN` list of at most 128 values. It lowers to
  conditional aggregation.
- `UNPIVOT` over a derived source with simple projected columns and a static
  `IN` list of at most 128 source columns. It lowers to `UNION ALL` branches.

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
