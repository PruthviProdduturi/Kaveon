# Distributed correlated `EXISTS OFFSET` qualification

Date: 2026-10-18

## Scope

This qualification covers a correlated `EXISTS` subquery with a positive `OFFSET`. The subquery is keyed by the outer `user_id`; `OFFSET 1` requires at least two matching inner rows for the outer row to qualify.

```sql
SELECT t.user_id,
       t.actions
FROM events t
WHERE EXISTS (
  SELECT *
  FROM events o
  WHERE o.user_id = t.user_id
  OFFSET 1
)
ORDER BY t.user_id, t.actions
LIMIT 12
```

## Result

The local planner and three-worker fragment path return the same exact 12 rows, and the query memory pool returns to zero after both executions. The regression is `distributed_correlated_exists_offset_matches_local` in `engine/crates/server/src/differential_tests.rs`.

## Boundary

This qualifies equality-correlated `EXISTS` with positive `OFFSET` through local and in-process distributed execution. It does not claim residual correlated offsets, arbitrary Apply shapes, correlated lateral limits, external-client wire qualification, or cloud qualification.
