# Track 1 — Null-key distributed window qualification

## Scope

Hash-partitioned windows must treat all NULL partition keys as one SQL partition before the worker evaluates ordered frames. This prevents rows with a NULL key from being split into independent worker-local partitions.

## Evidence

The execution regression uses interleaved NULL and non-NULL partition keys with an ordered `ROW_NUMBER()` window and observes the exact per-partition sequence:

```text
cargo test -p kaveon-exec --lib window::tests::partitioned_window_keeps_null_keys_together_with_ordered_frames --quiet
cargo fmt --all -- --check
cargo clippy -p kaveon-exec --all-targets -- -D warnings
```

The planner's partitioned-window path hashes the declared partition key and the worker window operator retains NULL-key rows together, while the ordered frame is evaluated after the exchange.

## Boundary

This is a focused local execution qualification. A deployed multi-worker run with NULL-heavy partitions and larger skew remains required before changing the broader distributed-window maturity claim.
