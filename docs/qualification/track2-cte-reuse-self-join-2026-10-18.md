# Track 2: repeated CTE self join

## Contract

A named non-recursive CTE may be referenced more than once in the same query.
Each reference receives its own relation qualifier, and the CTE column list
must be preserved so join keys remain discoverable by the planner.

## Exact qualification

```sql
WITH base(id, label) AS (VALUES (1, 'a'), (2, 'b'), (3, 'c'))
SELECT l.id, r.label
FROM base l
JOIN base r ON l.id = r.id
WHERE l.id > 1
ORDER BY l.id
```

Expected and observed result: `2 | b`, `3 | c` from both the local planner and
the three-worker in-process distributed fragment path. CTE definition column
aliases are retained and each reference is qualified before join predicate
routing, allowing the equality key to reach the hash join.

## Reproduce

From `engine/`:

```powershell
cargo test -p kaveon-server distributed_cte_reuse_self_join_matches_local_semantics -- --nocapture
```
