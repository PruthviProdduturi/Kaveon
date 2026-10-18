# Track 2: chained CTE join

## Contract

A non-recursive CTE may depend on an earlier CTE in the same `WITH` clause.
The derived CTE and the base CTE must retain their column names and relation
qualifiers when joined, so the equality key reaches distributed planning.

## Exact qualification

```sql
WITH base(id, label) AS (VALUES (1, 'a'), (2, 'b'), (3, 'c')),
filtered(id) AS (SELECT id FROM base WHERE id > 1)
SELECT f.id, b.label
FROM filtered f
JOIN base b ON f.id = b.id
ORDER BY f.id
```

Expected and observed result: `2 | b`, `3 | c` from both the local planner
and the three-worker in-process distributed fragment path.

## Reproduce

From `engine/`:

```powershell
cargo test -p kaveon-server distributed_chained_cte_join_matches_local_semantics -- --nocapture
```
