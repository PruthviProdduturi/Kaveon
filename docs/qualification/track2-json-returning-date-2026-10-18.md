# Track 2: JSON_VALUE DATE coercion

## Contract

`JSON_VALUE` supports a typed `RETURNING DATE` target. The extracted ISO date
string is routed through Kaveon's checked date cast, preserving the same value
through local and distributed execution.

## Exact qualification

```sql
SELECT JSON_VALUE(
  JSON_PARSE('{"day":"2024-01-02"}'), '$.day' RETURNING DATE
) AS day_value
FROM events
LIMIT 1
```

Expected and observed result: `2024-01-02` from both the local planner and the
three-worker in-process distributed fragment path.

## Reproduce

From `engine/`:

```powershell
cargo test -p kaveon-sql json_value_date_returning_lowers_through_cast -- --nocapture
cargo test -p kaveon-server distributed_json_value_date_returning_matches_local_semantics -- --nocapture
```
