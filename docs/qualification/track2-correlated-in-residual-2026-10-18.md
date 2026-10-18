# Track 2: correlated `IN` residual membership

## Contract

A correlated `IN` subquery returns an outer row when at least one right row
matches both the correlation key and its residual predicate. Duplicate right
matches remain existential, while residual-failing and NULL-key rows do not
count.

## Exact qualification

```sql
SELECT l.id
FROM (VALUES (1), (2), (3)) AS l(id)
WHERE l.id IN (
  SELECT r.value
  FROM (VALUES (1, 1), (1, 2), (2, 1), (CAST(NULL AS BIGINT), 9))
       AS r(value, score)
  WHERE r.value = l.id AND r.score > l.id
)
ORDER BY l.id
```

Expected and observed result: `1` from both the local planner and the
three-worker in-process distributed fragment path. Only the `(1, 2)` row
satisfies the residual; the duplicate `(1, 1)` row, `(2, 1)`, and the
NULL-key row do not create membership.

## Reproduce

From `engine/`:

```powershell
cargo test -p kaveon-server distributed_correlated_in_residual_matches_local_semantics -- --nocapture
```
