# Track 2: typed JSON query defaults

## Contract

`JSON_QUERY` may combine `RETURNING JSON` with a literal `ON EMPTY DEFAULT`
object. The default object is returned as JSON through the same typed cast path
used for a present object.

## Exact qualification

```sql
SELECT JSON_QUERY(
  JSON_PARSE('{}'),
  '$.missing' RETURNING JSON ON EMPTY DEFAULT '{"fallback":1}'
) AS fallback_value
FROM events
LIMIT 1
```

Expected and observed result: `{"fallback":1}` from both the local planner and
the three-worker in-process distributed fragment path.

## Reproduce

From `engine/`:

```powershell
cargo test -p kaveon-server distributed_typed_json_query_default_matches_local_semantics -- --nocapture
```
