# Track 2: nested map subscript

## Contract

Map subscript access can be chained when a map value is itself a map. Each
subscript resolves one key, preserving the nested value type through local and
distributed execution.

## Exact qualification

```sql
SELECT MAP(
  ARRAY['a', 'b'],
  ARRAY[
    MAP(ARRAY['x'], ARRAY[7]),
    MAP(ARRAY['x'], ARRAY[8])
  ]
)['b']['x'] AS value
FROM events
LIMIT 1
```

Expected and observed result: `8` from both the local planner and the
three-worker in-process distributed fragment path.

## Reproduce

From `engine/`:

```powershell
cargo test -p kaveon-server distributed_nested_map_subscript_matches_local_semantics -- --nocapture
```
