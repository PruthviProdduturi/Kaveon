# Track 2 qualification: NULL input to `MAP_CONCAT`

Date: 2026-10-18

## Result

The runtime already has a fail-closed NULL-row path in `MAP_CONCAT`, but the
SQL surface cannot currently express the required typed NULL map input:

```sql
MAP_CONCAT(
  CAST(NULL AS MAP(VARCHAR, BIGINT)),
  MAP(ARRAY['a'], ARRAY[1])
)
```

Planning fails with `unsupported CAST target type: MAP(VARCHAR, BIGINT)` before
local or distributed execution. This is a typed complex-cast gap, not a
runtime result claim. It remains open until `CastTarget` can preserve nested
map key/value types and the evaluator can cast NULL values to that map type.

## Boundary

Do not mark NULL map concatenation as qualified. The next implementation must
add a typed map cast, then compare local and three-worker results for NULL,
empty, and non-empty maps with rightmost-key replacement.
