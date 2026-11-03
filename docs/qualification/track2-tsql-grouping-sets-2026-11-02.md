# T-SQL `GROUPING SETS` qualification — 2026-11-02

## Scope

This qualification covers the bounded SQL Server/Fabric spelling:

```sql
SELECT region, platform, COUNT(*) AS n
FROM events
GROUP BY GROUPING SETS ((region), (platform))
```

The planner lowers each grouping set to the existing finite grouping-set
expansion. The result preserves NULL subtotal keys and aggregate semantics;
ordering remains unspecified unless the query supplies `ORDER BY`.

## Evidence

- SQL planner coverage is provided by the existing grouping-set lowering tests.
- Three-worker differential test:
  `distributed_grouping_sets_match_the_local_result`.
- The differential test compares canonical result rows from distributed
  execution with the local reference and verifies that the query memory pool
  returns to zero.

## Boundary

This is a bounded read-compatibility qualification. It does not claim support
for unbounded grouping-set expansion, arbitrary grouping expressions, or
cross-source grouping. The existing branch cap and planner diagnostics remain
the contract for oversized or unsupported shapes.
