# Track 1 — Int64 grouped-sum fast path qualification

## Scope

The columnar aggregate now fuses hashing, slot resolution, group creation, and
`SUM(Int64)` updates for the common single `Int64` group-key shape. It avoids
the general path's packed-row scratch buffer and separate accumulator pass.
Null aggregate values do not contribute to the sum or non-null count, while a
NULL group key remains a valid SQL group.

## Evidence

From `engine/`:

```text
cargo test -p kaveon-exec columnar_aggregate::tests:: -- --nocapture
test result: ok; 10 passed; 0 failed
```

The focused regression feeds keys `[4, NULL, 4, 9, NULL]` and values
`[10, 99, NULL, -3, NULL]`, verifying exact grouped states for duplicate and
NULL keys. The full columnar aggregate corpus also passes, including encoded
partial merges, malformed-state rejection, dictionary/text keys, and growth
admission.

## Boundary

This is limited to one `Int64` key and one `SUM(Int64)` aggregate. It does not
claim a general vectorized aggregate or a ClickBench wall-clock result;
multi-key and mixed-aggregate shapes retain their existing paths.
