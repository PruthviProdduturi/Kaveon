# Track 1 high-cardinality aggregate merge map — 2026-10-18

## Boundary

The coordinator's distributed partial-aggregate merge now uses a hash map for
group lookup and sorts serialized group keys once at the end. This preserves
the prior deterministic result order while changing the high-cardinality merge
work from repeated tree lookup to average constant-time lookup.

## Evidence

The existing exact-result and memory-failure tests pass against the new merge
path:

```text
cargo test -p kaveon-server merges_partial_group_counts -- --nocapture
cargo test -p kaveon-server distributed_aggregate_merge_fails_closed_and_releases_memory -- --nocapture
```

Results: both tests passed. The first verifies deterministic merged group
ordering and additive values; the second verifies bounded admission and zero
residual memory after refusal.

## Remaining boundary

This improves the coordinator partial-aggregate merge. It does not claim a
fully columnar aggregate state exchange or remove the coordinator final-merge
stage for all aggregate shapes.
