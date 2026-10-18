# Track 1 exact-distinct aggregate state qualification

## Scope

This gate covers the exact distinct aggregate state used by distributed partial
and final aggregation. The state uses `ahash::AHashSet` for the hot-path
membership structure. Serialized state remains deterministic because values are
sorted before encoding; this preserves byte-stable exchange and compact-state
merges.

## Evidence

- Commit: `bb3101ea`
- `cargo check -p kaveon-server --all-targets`: passed
- `cargo test -p kaveon-exec --lib distinct --quiet`: 32 passed, 0 failed
- `cargo test -p kaveon-server global_final_aggregate_streams_without_hash_partition_spill --quiet`: 1 passed, 0 failed
- `cargo test -p kaveon-server cancel_running_multi_stage_graph_queues_each_exchange_once --quiet`: 1 passed, 0 failed

The tests cover exact deduplication, compact-state round trips, incremental
partial-state union, and a final aggregate consuming multiple partial states.
This is a bounded correctness and hot-path qualification; it does not claim a
universal percentage improvement or close the larger-scale aggregation gates.
