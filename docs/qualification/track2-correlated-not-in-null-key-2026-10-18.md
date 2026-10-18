# Track 2: correlated `NOT IN` NULL key

## Contract

For a keyed correlated `NOT IN`, a right-side NULL correlation key cannot
match a non-NULL outer key. It must be discarded before anti-join membership
is evaluated; otherwise a partitioned operator can incorrectly treat that
unrelated NULL as a relation-wide `NOT IN` poison value.

## Exact qualification

```sql
SELECT l.id
FROM (VALUES (1), (2), (3)) AS l(id)
WHERE l.id NOT IN (
  SELECT r.value
  FROM (VALUES (1, true), (CAST(NULL AS BIGINT), true), (2, false))
       AS r(value, keep)
  WHERE r.value = l.id AND r.keep
)
ORDER BY l.id
```

Expected and observed result: `2`, `3` from both the local planner and the
three-worker in-process distributed fragment path. The binder filters NULL
correlation keys only for correlated anti joins; uncorrelated `NOT IN` keeps
its relation-wide NULL-aware behavior.

## Reproduce

From `engine/`:

```powershell
cargo test -p kaveon-server distributed_correlated_not_in_null_residual_matches_local_semantics -- --nocapture
```
