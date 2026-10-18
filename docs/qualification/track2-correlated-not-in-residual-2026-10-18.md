# Track 2: correlated `NOT IN` residual membership

## Contract

A correlated `NOT IN` subquery is evaluated per outer row. A right row is a
member only when both its correlation key equals the outer key and its
residual predicate is TRUE. Rows failing the residual, including NULL-key
rows, cannot poison another outer row.

## Exact qualification

```sql
SELECT l.id
FROM (VALUES (1), (2), (3)) AS l(id)
WHERE l.id NOT IN (
  SELECT r.value
  FROM (VALUES (1, 1), (1, 2), (2, 1), (CAST(NULL AS BIGINT), 9))
       AS r(value, score)
  WHERE r.value = l.id AND r.score > l.id
)
ORDER BY l.id
```

Expected and observed result: `2`, `3` from both the local planner and the
three-worker in-process distributed fragment path. The `value = id` row for
`id = 1` with score `2` is the only matching membership; the `id = 2` row
fails the residual and the NULL-key row never correlates.

## Reproduce

From `engine/`:

```powershell
cargo test -p kaveon-server distributed_correlated_not_in_residual_matches_local_semantics -- --nocapture
```
