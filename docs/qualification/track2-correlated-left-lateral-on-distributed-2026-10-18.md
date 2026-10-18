# Correlated `LEFT JOIN LATERAL ... ON` qualification

Date: 2026-10-18

## Scope

This qualification covers a correlated `LEFT JOIN LATERAL` whose derived relation uses the outer row in both its filter and an explicit `ON` predicate. The unmatched outer row must be retained with NULL-extended right columns.

```sql
SELECT u.user_id,
       o.actions
FROM (VALUES (1), (9999)) AS u(user_id)
LEFT JOIN LATERAL (
  SELECT events.user_id, events.actions
  FROM events
  WHERE events.user_id = u.user_id
    AND events.actions > u.user_id
) AS o ON o.user_id = u.user_id
ORDER BY u.user_id, o.actions
```

## Result

The local planner and three-worker fragment path return the same exact rows, including the `9999 | NULL` unmatched outer row. The query memory pool returns to zero after both executions. The regression is `correlated_left_lateral_join_on_matches_local_null_extension` in `engine/crates/server/src/differential_tests.rs`.

## Boundary

This qualifies correlated `LEFT JOIN LATERAL` with an explicit `ON` predicate through local and in-process distributed execution. It does not claim correlated lateral limits, arbitrary Apply shapes, external-client wire qualification, or cloud qualification.
