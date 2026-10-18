# Track 2: null-aware `NOT IN` subquery

## Contract

`NOT IN` follows Trino's three-valued logic. If the subquery contains a NULL
key and no equal key is found, the predicate is UNKNOWN, so the probe row is
filtered. A distributed plan must observe NULLs from every right-side
partition; treating each worker's partition as an independent anti join is
incorrect.

## Exact qualification

The regression statement is:

```sql
SELECT id
FROM (VALUES (1), (2), (3)) AS l(id)
WHERE id NOT IN (
  SELECT value
  FROM (VALUES (2), (CAST(NULL AS BIGINT))) AS r(value)
)
ORDER BY id
```

Expected and observed result: zero rows for both the local planner and the
three-worker in-process distributed fragment path. The right side is forced
through the broadcast path for null-aware anti joins so every worker sees the
complete NULL fact. `NOT EXISTS` remains an ordinary anti join and is not
changed by this rule.

## Reproduce

From `engine/`:

```powershell
cargo test -p kaveon-server distributed_not_in_null_subquery_matches_local_semantics -- --nocapture
```

The test also asserts that the query memory pool is fully drained after both
execution paths.
