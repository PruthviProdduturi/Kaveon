# Track 2: typed JSON defaults

## Contract

`JSON_VALUE` may combine a supported typed `RETURNING` clause with literal
`ON EMPTY DEFAULT` and `ON ERROR DEFAULT` actions. The default evaluator emits
the literal result and the normal typed cast then enforces the requested
return type for both missing paths and malformed documents.

## Exact qualification

```sql
SELECT
  JSON_VALUE(JSON_PARSE('{}'), '$.missing'
    RETURNING BIGINT ON EMPTY DEFAULT '7' ON ERROR DEFAULT '9') AS missing_value,
  JSON_VALUE('not-json', '$.value'
    RETURNING BIGINT ON EMPTY DEFAULT '7' ON ERROR DEFAULT '9') AS malformed_value
FROM events
LIMIT 1
```

Expected and observed result: `7 | 9` from both the local planner and the
three-worker in-process distributed fragment path.

## Reproduce

From `engine/`:

```powershell
cargo test -p kaveon-sql json_default_with_returning_lowers_through_cast -- --nocapture
cargo test -p kaveon-server distributed_typed_json_default_matches_local_semantics -- --nocapture
```
