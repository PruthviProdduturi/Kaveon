# Track 2 `REPEAT` array overload — 2026-10-18

## Boundary

Trino overloads `REPEAT` for both strings and arrays. Kaveon already had the
bounded typed `ARRAY_REPEAT` evaluator, but the Trino spelling `REPEAT(array,
count)` was rejected by the UNNEST binder as a non-array function and the
scalar path only recognized VARCHAR values.

## Change

The evaluator routes list-valued `REPEAT` calls through the existing typed
`ARRAY_REPEAT` implementation. The UNNEST allow-list accepts `REPEAT`, and its
array-producing expression is normalized to `ARRAY_REPEAT` before planning.
String `REPEAT` behavior remains unchanged.

## Evidence

The distributed three-worker differential query
`UNNEST(REPEAT(ARRAY[1, 2], 2))` returns two independent `[1, 2]` rows and
clears the query memory pool. The focused evaluator regression also checks
nested list boundaries, null inner arrays, and zero counts:

```text
cargo test -p kaveon-exec repeat_alias_supports_array_values -- --nocapture
cargo test -p kaveon-server distributed_repeat_array_alias_preserves_inner_arrays -- --nocapture
```

Both passed locally. General array overload coverage and external pinned-Trino
wire qualification remain open.
