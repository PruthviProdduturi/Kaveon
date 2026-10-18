# Track 2: CTE scope in subquery membership

## Contract

A non-recursive CTE remains visible inside a nested subquery while the outer
query references the same CTE. Definition column aliases and per-reference
qualifiers must survive both query levels so membership keys bind correctly.

## Exact qualification

```sql
WITH base(id, label) AS (VALUES (1, 'a'), (2, 'b'), (3, 'c'))
SELECT outer_base.id
FROM base outer_base
WHERE outer_base.id IN (
  SELECT inner_base.id
  FROM base inner_base
  WHERE inner_base.id > 1
)
ORDER BY outer_base.id
```

Expected and observed result: `2`, `3` from both the local planner and the
three-worker in-process distributed fragment path.

## Reproduce

From `engine/`:

```powershell
cargo test -p kaveon-server distributed_cte_scope_survives_subquery_membership -- --nocapture
```
