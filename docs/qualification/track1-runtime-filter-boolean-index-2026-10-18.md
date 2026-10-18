# Track 1 — boolean runtime-filter index

## Scope

Single-key boolean joins now use a typed hash index for probe-side runtime
filtering, including exact `IN` pushdown values. Null probe keys remain
rejected by the filter, while duplicate build values are represented once in
the storage predicate and still preserve join output semantics in the join
index.

## Evidence

From `engine/`:

```text
cargo test -p kaveon-exec join::tests::boolean_runtime_filter_uses_typed_index_and_storage_predicate -- --nocapture
test result: ok; 1 passed; 0 failed
```

The regression checks duplicate and null build values, exact boolean `IN`
values, and the probe mask for `false`, `true`, and `NULL` rows.

## Boundary

This qualifies the local typed boolean runtime-filter path. It does not claim
deployed cloud pruning, composite boolean-key specialization, or broader
runtime-filter endurance.
