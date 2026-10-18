# Correlated scalar UNION with deterministic LIMIT

The Engine now supports a bounded correlated scalar subquery whose relation is
formed by `UNION ALL` and whose result is selected with `ORDER BY ... LIMIT 1`.
Each UNION branch may carry the same equality correlation. The binder
normalizes branch-local correlation aliases to one UNION output key before the
per-key `ROW_NUMBER()` lowering, so the distributed join sees one key rather
than one generated key per branch.

Qualification query:

```sql
SELECT e.user_id,
       (SELECT value
        FROM (
          SELECT i.actions AS value
          FROM events i
          WHERE i.user_id = e.user_id
          UNION ALL
          SELECT i.actions + 10 AS value
          FROM events i
          WHERE i.user_id = e.user_id
        ) AS candidates
        ORDER BY value
        LIMIT 1) AS first_value
FROM events e
ORDER BY e.user_id
LIMIT 12;
```

The server differential test `distributed_correlated_scalar_union_limit_matches_local`
compares exact canonical rows from the local operator and the in-process
three-worker fragment path. Both paths return 12 rows and the query memory
pool returns to zero after each execution.

Boundary: this qualifies equality-correlated, one-column scalar `UNION ALL`
with deterministic ordering and `LIMIT 1`. Correlated UNION branches with
residual predicates, multi-column scalar output, unbounded paging, or deeper
Apply/lateral nesting remain outside this qualification.
