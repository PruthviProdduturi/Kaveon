# Track 2: distributed JSON scalar `RETURNING BOOLEAN`

## Scope

This qualification covers typed SQL/JSON boolean extraction, including JSON
null propagation:

```sql
SELECT
  JSON_VALUE(JSON_PARSE('{"enabled":true}'), '$.enabled' RETURNING BOOLEAN),
  JSON_VALUE(JSON_PARSE('{"enabled":false}'), '$.enabled' RETURNING BOOLEAN),
  JSON_VALUE(JSON_PARSE('{"enabled":null}'), '$.enabled' RETURNING BOOLEAN)
```

## Evidence

`differential_tests::distributed_json_value_returning_boolean_matches_local`
plans the expression locally and runs the same plan through three workers. The
three output rows are exactly `true`, `false`, and `NULL` in both paths, and
the query memory pool returns to zero after execution.

Command:

```text
cargo test -p kaveon-server distributed_json_value_returning_boolean_matches_local -- --nocapture
```

Result: 1 passed, 0 failed.

## Boundary

This covers scalar boolean coercion only. Full SQL/JSON coercion, arbitrary
RETURNING expressions, and external-client/cloud qualification remain open.
