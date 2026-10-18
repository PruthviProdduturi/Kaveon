# Track 2 qualification: correlated `UNNEST(MAP_ENTRIES(...))`

Date: 2026-10-18  
Scope: local evaluator and in-process three-worker fragment execution

## Query

```sql
SELECT x.id, u.key, u.value, u.ord
FROM (VALUES (1), (3)) AS x(id)
CROSS JOIN LATERAL UNNEST(
  MAP_ENTRIES(MAP(ARRAY[x.id, x.id + 10], ARRAY[100, 200]))
) WITH ORDINALITY AS u(key, value, ord)
ORDER BY x.id, u.ord
```

## Result

The correlated map entries are evaluated independently for each left row. The
local and three-worker distributed plans return the same rows:

```text
1 | 1  | 100 | 1
1 | 11 | 200 | 2
3 | 3  | 100 | 1
3 | 13 | 200 | 2
```

The regression also verifies that the query memory pool returns to zero after
both executions. This qualifies the row-valued `MAP_ENTRIES` member of the
allow-listed correlated lateral UNNEST family. Arbitrary table-function
expressions and deeper nested/lateral combinations remain outside this result.
