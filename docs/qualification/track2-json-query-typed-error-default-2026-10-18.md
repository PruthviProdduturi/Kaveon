# Track 2: typed JSON query error defaults

## Contract

`JSON_QUERY` may combine `RETURNING JSON` with a literal `ON ERROR DEFAULT`
object. A malformed document selects that default object, and the normal JSON
cast preserves it as a typed JSON result.

## Exact qualification

```sql
SELECT JSON_QUERY(
  'not-json', '$.missing'
  RETURNING JSON ON ERROR DEFAULT '{"fallback":2}'
) AS fallback_value
FROM events
LIMIT 1
```

Expected and observed result: `{"fallback":2}` from both the local planner and
the three-worker in-process distributed fragment path.

## Reproduce

From `engine/`:

```powershell
cargo test -p kaveon-sql json_query_typed_error_default_lowers_through_cast -- --nocapture
cargo test -p kaveon-server distributed_typed_json_query_error_default_matches_local_semantics -- --nocapture
```
