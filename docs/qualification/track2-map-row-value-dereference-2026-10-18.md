# Track 2: MAP value ROW projection

## Contract

Projection schema inference follows a map lookup into a ROW value, then resolves the selected ROW field even when the fragment schema is probed before rows are available. Field names remain case-insensitive.

## Exact qualification

```sql
SELECT ROW_FIELD(
  MAP_ELEMENT_AT(
    MAP(ARRAY['a'], ARRAY[ROW(7, 'ok')]),
    'a'
  ),
  'FIELD0'
) AS value
FROM events
LIMIT 1
```

The local planner and in-process three-worker distributed fragment path both return `7` with an `Int64` result field. The projection resolver derives the map's ROW value type before dereferencing `FIELD0`.

## Reproduce

From `engine/`:

```powershell
cargo test -p kaveon-server distributed_map_row_value_dereference_matches_local_schema -- --nocapture
```
