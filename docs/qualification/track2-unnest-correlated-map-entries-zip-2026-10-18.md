# Track 2 qualification: correlated map entries zipped with an array

Date: 2026-10-18  
Scope: local evaluator and in-process three-worker fragment execution

## Query

```sql
SELECT x.id, u.key, u.value, u.extra, u.ord
FROM (VALUES (1), (3)) AS x(id)
CROSS JOIN LATERAL UNNEST(
  MAP_ENTRIES(MAP(ARRAY[x.id], ARRAY[x.id + 10])),
  ARRAY[x.id, x.id + 20]
) WITH ORDINALITY AS u(key, value, extra, ord)
ORDER BY x.id, u.ord
```

## Result

The correlated map entries and array are evaluated for each left row, zipped
to the longest input, and NULL-padded for the shorter map-entry input. The
local and three-worker distributed plans return the same rows:

```text
1 | 1    | 11   | 1  | 1
1 | NULL | NULL | 21 | 2
3 | 3    | 13   | 3  | 1
3 | NULL | NULL | 23 | 2
```

The regression also verifies that the query memory pool returns to zero after
both executions. This qualifies the multi-input correlated lateral shape for
row-valued `MAP_ENTRIES`; arbitrary table functions and deeper nested/lateral
combinations remain outside this result.
