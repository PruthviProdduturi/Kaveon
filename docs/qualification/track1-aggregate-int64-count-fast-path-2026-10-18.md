# Track 1 — Int64 grouped-count fast path qualification

## Scope

The columnar aggregate now fuses hashing, slot resolution, group creation, and
`COUNT(*)` updates for the common single `Int64` group-key shape. This avoids
the general path's packed-row scratch buffer, separate hash/slot arrays, and
second accumulator pass. Null keys remain one SQL group and duplicate keys
continue to increment the same count.

## Evidence

From `engine/`:

```text
cargo test -p kaveon-exec columnar_aggregate::tests:: -- --nocapture
test result: ok; 9 passed; 0 failed
```

The focused regression feeds `[4, NULL, 4, 9, NULL]` through the fused path
and verifies three groups with counts `4 → 2`, `9 → 1`, and `NULL → 2`. The
remaining columnar aggregate corpus also passes, including dictionary and
UTF-8 keys, encoded partial merges, malformed-state rejection, and table
growth admission.

## Boundary

This is a narrow single-key `Int64` plus `COUNT(*)` optimization. It does not
claim a general vectorized hash aggregate or a ClickBench wall-clock result;
multi-key, mixed-aggregate, text, and high-cardinality exchange shapes retain
their existing paths.
