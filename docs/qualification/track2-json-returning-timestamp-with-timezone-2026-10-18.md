# Track 2: JSON_VALUE TIMESTAMP WITH TIME ZONE coercion

## Contract

`JSON_VALUE` accepts a typed `RETURNING TIMESTAMP WITH TIME ZONE` target. An offset-bearing timestamp string is parsed as an instant and normalized to Kaveon's canonical UTC timestamp representation.

## Exact qualification

```sql
SELECT JSON_VALUE(
  JSON_PARSE('{"created":"2024-01-02 03:04:05+02:00"}'),
  '$.created' RETURNING TIMESTAMP WITH TIME ZONE
) AS created_value
FROM events
LIMIT 1
```

The local planner and the in-process three-worker distributed fragment path both return a timestamp-with-time-zone value with epoch microseconds `1704157445000000` (`2024-01-02 01:04:05 UTC`).

## Reproduce

From `engine/`:

```powershell
cargo test -p kaveon-sql json_value_timestamp_with_timezone_returning_lowers_through_cast -- --nocapture
cargo test -p kaveon-server distributed_json_value_timestamp_with_timezone_returning_matches_local_semantics -- --nocapture
```
