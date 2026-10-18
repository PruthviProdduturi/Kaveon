# Track 1 — Int64 grouped-count-and-sum fast path qualification

## Scope

The columnar aggregate now fuses the common single `Int64` group-key shape
with `COUNT(*)` and `SUM(Int64)` together. One probe resolves the group slot,
increments the count, and updates the sum state, avoiding the general path's
packed-row scratch buffer and second accumulator pass. Duplicate keys,
NULL group keys, and NULL sum values retain SQL semantics.

## Evidence

From `engine/`:

```text
cargo test -p kaveon-exec columnar_aggregate::tests:: -- --nocapture
test result: ok; 11 passed; 0 failed
```

The focused regression feeds keys `[4, NULL, 4, 9, NULL]` and values
`[10, 99, NULL, -3, NULL]`, verifying exact count and sum states for all three
groups. The complete columnar aggregate corpus also passes the individual
fused count and sum paths, encoded partial merges, malformed-state rejection,
dictionary/text keys, and growth admission.

## Boundary

This is limited to one `Int64` key with exactly `COUNT(*)` plus `SUM(Int64)`.
Other aggregate combinations retain their existing paths; this evidence does
not claim general mixed-aggregate vectorization or a ClickBench wall-clock
result.
