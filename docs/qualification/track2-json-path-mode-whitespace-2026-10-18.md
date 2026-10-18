# Track 2: JSON path mode whitespace

## Contract

SQL/JSON `lax` and `strict` mode prefixes accept one or more whitespace
characters before the root marker. Normalizing that separator must preserve
the same path result across local and distributed execution.

## Exact qualification

```sql
SELECT
  JSON_VALUE(JSON_PARSE('{"x":7}'), 'lax   $.x') AS lax_value,
  JSON_QUERY(JSON_PARSE('{"items":[1,2]}'), 'strict\t$.items') AS strict_items
FROM events
LIMIT 1
```

Expected and observed result: `7 | [1,2]` from both the local planner and the
three-worker in-process distributed fragment path.

## Reproduce

From `engine/`:

```powershell
cargo test -p kaveon-sql json_path_mode_prefixes_accept_whitespace_separators -- --nocapture
cargo test -p kaveon-server distributed_json_path_mode_whitespace_matches_local_semantics -- --nocapture
```
