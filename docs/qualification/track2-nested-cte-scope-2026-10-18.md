# Track 2: nested CTE scope

## Contract

A non-recursive CTE body may contain its own `WITH` clause. The nested
definition must remain visible while lowering the outer CTE, then the outer
relation must retain its declared column names for the final projection.

## Exact qualification

```sql
WITH outer_rows(id, label) AS (
  WITH inner_rows(id, label) AS (
    VALUES (1, 'a'), (2, 'b'), (3, 'c')
  )
  SELECT id, label FROM inner_rows WHERE id > 1
)
SELECT id, label FROM outer_rows ORDER BY id
```

Expected and observed result: `2 | b`, `3 | c` from both the local planner
and the three-worker in-process distributed fragment path.

## Reproduce

From `engine/`:

```powershell
cargo test -p kaveon-server distributed_nested_cte_scope_matches_local_semantics -- --nocapture
```
