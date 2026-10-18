# Distributed nested map/ROW null-safe comparison qualification

Date: 2026-10-18

## Scope

This qualification covers recursive `IS NOT DISTINCT FROM` comparison across maps, rows, NULL map values, and a nested ROW containing a map. Map key order differs between operands, while null-safe comparison must remain two-valued and order-independent.

```sql
SELECT MAP(ARRAY['a', 'b'], ARRAY[10, CAST(NULL AS BIGINT)])
         IS NOT DISTINCT FROM
       MAP(ARRAY['b', 'a'], ARRAY[CAST(NULL AS BIGINT), 10]) AS maps_same,
       ROW(1, CAST(NULL AS BIGINT))
         IS NOT DISTINCT FROM
       ROW(1, CAST(NULL AS BIGINT)) AS rows_same,
       ROW(MAP(ARRAY['a'], ARRAY[CAST(NULL AS BIGINT)]))
         IS NOT DISTINCT FROM
       ROW(MAP(ARRAY['a'], ARRAY[CAST(NULL AS BIGINT)])) AS nested_same
FROM (VALUES (1), (2), (3)) AS input(id)
ORDER BY input.id
```

## Result

The local planner and three-worker fragment path return three exact `true | true | true` rows. The query memory pool returns to zero after both executions. The regression is `distributed_map_row_null_safe_comparison_matches_local` in `engine/crates/server/src/differential_tests.rs`.

## Boundary

This qualifies recursive null-safe equality for nested maps and rows through local and in-process distributed execution. It does not claim map ordering, general ordering comparisons for MAP values, external-client wire qualification, or cloud qualification.
