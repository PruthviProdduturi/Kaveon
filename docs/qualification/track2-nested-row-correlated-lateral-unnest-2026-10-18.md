# Track 2 qualification: nested correlated lateral UNNEST over ROW values

Date: 2026-10-18  
Scope: local evaluator and in-process three-worker fragment execution

## Query

```sql
SELECT x.id, a.base, b.nested
FROM (VALUES (1), (3)) AS x(id)
CROSS JOIN LATERAL UNNEST(ARRAY[ROW(x.id, x.id + 10)]) AS a(id, base)
CROSS JOIN LATERAL UNNEST(ARRAY[a.base, a.base + 100]) AS b(nested)
ORDER BY x.id, a.base, b.nested
```

## Result

The first lateral relation materializes a correlated ROW, and the second
lateral relation reads its projected field for a per-row array. The local and
three-worker distributed plans return the same rows:

```text
1 | 11 | 11
1 | 11 | 111
3 | 13 | 13
3 | 13 | 113
```

The regression also verifies that the query memory pool returns to zero after
both executions. This qualifies a nested lateral chain whose intermediate
relation contains ROW values; arbitrary table functions and deeper nested /
lateral combinations remain outside this result.
