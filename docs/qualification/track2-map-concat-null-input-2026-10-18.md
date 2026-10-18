# Track 2 qualification: NULL input to `MAP_CONCAT`

Date: 2026-10-18

## Result

Typed NULL map casts are now represented recursively in `CastTarget`, and the
SQL surface accepts the required typed NULL map input:

```sql
MAP_CONCAT(
  CAST(NULL AS MAP(VARCHAR, BIGINT)),
  MAP(ARRAY['a'], ARRAY[1])
)
```

The focused local/three-worker differential
`distributed_map_concat_null_input_matches_local` verifies that a NULL map
input propagates NULL through `MAP_CONCAT` and that the query memory pool drains
after both execution paths.

## Boundary

The current case covers a typed NULL map plus a non-empty map. Empty-map and
rightmost-key replacement cases remain covered by the existing map differential
corpus; broader nested-map casts remain outside this bounded change.
