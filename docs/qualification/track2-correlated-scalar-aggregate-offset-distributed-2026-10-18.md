# Distributed correlated scalar aggregate `OFFSET` qualification

Date: 2026-10-18

## Scope

This qualification covers an equality-correlated scalar aggregate with a positive `OFFSET`. An ungrouped aggregate produces one scalar row; offsetting that row produces the SQL NULL scalar for every outer row.

```sql
SELECT t.user_id,
       (
         SELECT SUM(i.actions)
         FROM events i
         WHERE i.user_id = t.user_id
         OFFSET 1
       ) AS delayed_total
FROM events t
ORDER BY t.user_id, t.actions
LIMIT 12
```

## Result

The local planner and three-worker fragment path return the same 12 exact rows, with NULL scalar values after the offset. The query memory pool returns to zero after both executions. The regression is `distributed_correlated_scalar_aggregate_offset_matches_local` in `engine/crates/server/src/differential_tests.rs`.

## Boundary

This qualifies equality-correlated ungrouped scalar aggregate `OFFSET` semantics through local and in-process distributed execution. It does not claim residual correlated offsets, multi-column scalar limits, arbitrary Apply shapes, external-client wire qualification, or cloud qualification.
