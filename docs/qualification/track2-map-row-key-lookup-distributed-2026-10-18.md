# Track 2: map lookup with comparable ROW keys

## Scope

Trino map keys may use any comparable type. Before this change,
`MAP_CONTAINS_KEY` and `MAP_ELEMENT_AT` only selected scalar Arrow key
representations and rejected a comparable `ROW` key even though the map
constructor and recursive ROW equality already supported it.

## Qualification

The local evaluator test `map_lookup_accepts_comparable_row_keys` constructs a
`MAP(ARRAY[ROW(7, 'alpha')], ARRAY[99])` and verifies both lookup functions
return `true` and `99`. The distributed server differential
`distributed_map_lookup_row_key_matches_local_rows` runs the equivalent query
through the three-worker fragment path, compares its canonical rows to local
execution, and checks that the query memory pool returns to zero.

```powershell
cargo test -p kaveon-exec map_lookup_accepts_comparable_row_keys -- --nocapture
cargo test -p kaveon-server --bin kaveon-server distributed_map_lookup_row_key -- --nocapture
```

Both tests passed on 2026-10-18. Primitive keys retain their specialized
typed lookup paths; only nested comparable keys use the recursive comparison
fallback. MAP ordering remains unsupported because Trino does not define an
ordering for MAP values.

