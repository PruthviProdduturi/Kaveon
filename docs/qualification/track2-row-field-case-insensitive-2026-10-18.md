# Track 2: case-insensitive ROW_FIELD dereference

## Contract

Named ROW dereference accepts field names without case sensitivity, matching SQL identifier lookup. NULL parent rows still produce NULL child values, and nested ROW dereference preserves that behavior.

## Exact qualification

```sql
SELECT
  ROW_FIELD(record, 'field0') AS named_first,
  ROW_FIELD(record, 'FIELD0') AS case_named_first,
  ROW_FIELD(ROW_FIELD(record, 'nested'), 'value') AS nested_value
FROM events
```

The local planner and three-worker distributed fragment path return identical values for the lowercase and uppercase field references. A NULL outer ROW remains NULL for both references.

## Reproduce

From `engine/`:

```powershell
cargo test -p kaveon-server distributed_row_subscript_matches_local_and_rejects_invalid_constant_index -- --nocapture
```
