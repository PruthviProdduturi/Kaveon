# Track 2: JSON_VALUE TIMESTAMP coercion

## Contract

`JSON_VALUE` supports a typed `RETURNING TIMESTAMP` target. An ISO timestamp
string is routed through Kaveon's checked timestamp cast and retains the same
instant through local and distributed execution.

## Exact qualification

```sql
SELECT JSON_VALUE(
  JSON_PARSE('{"created":"2024-01-02 03:04:05"}'),
  '$.created' RETURNING TIMESTAMP
) AS created_value
FROM events
LIMIT 1
```

Expected and observed result: `2024-01-02T03:04:05` from both the local
planner and the three-worker in-process distributed fragment path.

## Reproduce

From `engine/`:

```powershell
cargo test -p kaveon-sql json_value_timestamp_returning_lowers_through_cast -- --nocapture
cargo test -p kaveon-server distributed_json_value_timestamp_returning_matches_local_semantics -- --nocapture
```
