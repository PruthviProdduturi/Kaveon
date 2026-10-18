# Distributed JSON `ON EMPTY DEFAULT` qualification

Date: 2026-10-18

## Scope

This qualification covers a literal `ON EMPTY DEFAULT` action on `JSON_VALUE` when the requested path is absent. The expression is evaluated for three input rows and compared between the local planner and the three-worker fragment path.

```sql
SELECT JSON_VALUE(
  JSON_PARSE('{"x":7}'),
  '$.missing' ON EMPTY DEFAULT 'fallback'
) AS value
FROM (VALUES (1), (2), (3)) AS input(id)
ORDER BY input.id
```

## Result

Both execution paths return three exact `fallback` rows. The query memory pool returns to zero after local and distributed execution. The regression is `distributed_json_value_default_on_empty_matches_local` in `engine/crates/server/src/differential_tests.rs`.

## Boundary

This qualifies literal default handling for an empty path through local and in-process distributed execution. It does not expand the supported surface to typed `RETURNING` with defaults, expression defaults, full JSONPath coercion, or external-client/cloud qualification.
