# Track 2 qualification: correlated `UNNEST(MAP_KEYS(...))`

Date: 2026-10-18  
Scope: local evaluator and in-process three-worker fragment execution

## Query

```sql
SELECT x.id, u.key, u.ord
FROM (VALUES (1), (3)) AS x(id)
CROSS JOIN LATERAL UNNEST(
  MAP_KEYS(MAP(ARRAY[x.id, x.id + 10], ARRAY[100, 200]))
) WITH ORDINALITY AS u(key, ord)
ORDER BY x.id, u.ord
```

## Result

The correlated map is evaluated independently for each left row. Both the
local plan and the three-worker distributed plan return the same ordered rows:

```text
1 | 1  | 1
1 | 11 | 2
3 | 3  | 1
3 | 13 | 2
```

The regression also verifies that the query memory pool returns to zero after
the local and distributed executions. This closes the `MAP_KEYS` member of the
allow-listed correlated lateral UNNEST family. Arbitrary table-function
expressions and deeper nested/lateral combinations remain outside this result.
