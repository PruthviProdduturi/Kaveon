# Bounded T-SQL PIVOT and UNPIVOT qualification — 2026-10-28

## Contract

Kaveon qualifies two bounded SQL Server/Fabric relational shapes:

- `PIVOT` over a derived source with simple projected columns, one supported
  mergeable aggregate (`SUM`, `AVG`, `MIN`, `MAX`, or `COUNT`, including
  `COUNT(*)`), and a static `IN` list of at most 128 values. It lowers to
  conditional aggregation.
- `UNPIVOT` over a derived source with simple projected columns and a static
  `IN` list of at most 128 source columns. It lowers to `UNION ALL` branches.

An outer `ORDER BY` is retained by both rewrites. Ordered output is qualified
when the requested keys define a total order; ties retain normal SQL semantics
and are not treated as a deterministic row sequence.

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

The current distributed corpus uses `ORDER BY region` for bounded PIVOT and
`ORDER BY region, metric, measure` for bounded UNPIVOT; both are total-order
cases and are marked qualified.

The full 113-case corpus was rerun on 2026-11-02 after this qualification and
passed exact local/layout/three-worker parity in 680.25 seconds.
