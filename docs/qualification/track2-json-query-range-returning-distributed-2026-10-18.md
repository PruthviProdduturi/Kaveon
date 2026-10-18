# Distributed JSON array range with typed JSON output

Date: 2026-10-18

## Scope

This qualification covers `JSON_QUERY` over an inclusive array range with a typed `RETURNING JSON` result. The same constant expression is evaluated for three input rows so the local and distributed paths are compared as row-producing query results.

```sql
SELECT JSON_QUERY(
  JSON_PARSE('{"items":[{"id":1},{"id":2},{"id":3}]}'),
  '$.items[1 to 2].id' RETURNING JSON
) AS ids
FROM (VALUES (1), (2), (3)) AS input(id)
ORDER BY input.id
```

## Result

Both the local planner and the three-worker fragment path return three exact rows, each containing `[2,3]`. The query memory pool returns to zero after both executions. The regression is `distributed_json_query_array_range_returning_json_matches_local` in `engine/crates/server/src/differential_tests.rs`.

## Boundary

This qualifies the implemented bounded array-range and typed JSON materialization path through local and in-process distributed execution. It does not claim complete JSONPath coercion, error/default action coverage, or external-client qualification.
