# Track 2: SQL/JSON path mode prefixes

## Contract

SQL/JSON path literals may begin with `lax` or `strict`. The mode prefix is
removed before structural path extraction; the existing `JSON_VALUE` and
`JSON_QUERY` function variants continue to control strict error behavior.

## Exact qualification

```sql
SELECT
  JSON_VALUE(JSON_PARSE('{"x":7}'), 'lax $.x') AS lax_value,
  JSON_QUERY(JSON_PARSE('{"items":[1,2]}'), 'strict $.items') AS strict_items
FROM events
LIMIT 1
```

Expected and observed result: `7 | [1,2]` from both the local planner and the
three-worker in-process distributed fragment path.

## Reproduce

From `engine/`:

```powershell
cargo test -p kaveon-sql json_path_mode_prefixes_remain_in_path_literals -- --nocapture
cargo test -p kaveon-server distributed_json_path_mode_prefixes_match_local_semantics -- --nocapture
```
