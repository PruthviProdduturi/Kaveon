# Native TIME runtime-filter qualification — 2026-10-18

## Scope

This qualification closes a bounded executor gap in Track 1. A single-key
`TIME32` or `TIME64` build side now creates a native typed runtime-filter index
instead of falling back to per-row composite-key materialisation. The filter
retains the exact Arrow time unit and produces checked integer storage `IN`
literals, so it can prune the probe before hash probing without changing SQL
NULL or unit semantics.

## Evidence

- `kaveon-exec` covers `TIME32(MILLISECOND)` and `TIME64(NANOSECOND)` with
  duplicate build keys, an unmatched probe key, and a NULL probe. It asserts
  the native index, exact two-value storage predicate, and row mask.
- `kaveon-server` compares local and three-worker execution of a TIME join;
  both return the same two matching rows and release the query reservation.

Commands:

```text
cargo test -p kaveon-exec time_runtime_filter_uses_native_typed_index_and_exact_storage_predicate -- --nocapture
cargo test -p kaveon-server distributed_time_join_runtime_filter_matches_local -- --nocapture
```

Both focused tests passed locally on 2026-10-18.

## Boundary

This is an executor and partitioned-runtime qualification. It does not claim
cloud row-group reduction or ordered Arrow result streaming; deployed/cloud
statistics pruning and broader exchange/topology qualification remain tracked
separately.
