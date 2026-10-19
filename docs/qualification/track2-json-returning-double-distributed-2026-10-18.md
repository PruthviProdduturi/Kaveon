# Track 2: distributed JSON scalar `RETURNING DOUBLE`

## Scope

This qualification covers typed SQL/JSON floating-point extraction and NULL
propagation:

```sql
SELECT
  JSON_VALUE(JSON_PARSE('{"amount":12.34}'), '$.amount' RETURNING DOUBLE),
  JSON_VALUE(JSON_PARSE('{"amount":null}'), '$.amount' RETURNING DOUBLE)
```

## Evidence

`differential_tests::distributed_json_value_returning_double_matches_local`
plans the expression locally and executes the same plan through three workers.
All three input rows return the exact pair `12.34, NULL` in both paths, and the
query memory pool returns to zero after execution.

Command:

```text
cargo test -p kaveon-server distributed_json_value_returning_double_matches_local -- --nocapture
```

Result: 1 passed, 0 failed.

## Boundary

This covers scalar DOUBLE coercion only. Full SQL/JSON coercion, arbitrary
RETURNING expressions, and external-client/cloud qualification remain open.
