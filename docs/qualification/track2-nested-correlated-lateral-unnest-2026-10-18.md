# Track 2 qualification: nested correlated lateral UNNEST

Date: 2026-10-18  
Scope: local evaluator and in-process three-worker fragment execution

## Query

```sql
SELECT x.id, a.value, b.nested
FROM (VALUES (1), (3)) AS x(id)
CROSS JOIN LATERAL UNNEST(ARRAY[x.id, x.id + 10]) AS a(value)
CROSS JOIN LATERAL UNNEST(ARRAY[a.value, a.value + 100]) AS b(nested)
ORDER BY x.id, a.value, b.nested
```

## Result

The second lateral UNNEST is evaluated against each row produced by the first
lateral UNNEST. The local and three-worker distributed plans return the same
rows:

```text
1 | 1  | 1
1 | 1  | 101
1 | 11 | 11
1 | 11 | 111
3 | 3  | 3
3 | 3  | 103
3 | 13 | 13
3 | 13 | 113
```

The regression also verifies that the query memory pool returns to zero after
both executions. This qualifies a two-level correlated lateral UNNEST chain;
arbitrary table functions and deeper nested/lateral combinations remain
outside this result.
