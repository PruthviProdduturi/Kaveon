# Track 1 — Shared window sort reuse

## Scope

A single WindowOperator can evaluate several expressions over the same `PARTITION BY` and `ORDER BY` layout. It now computes the Arrow lexsort permutation once per layout and reuses it across frames and functions; each expression still evaluates its own peer groups, frame bounds, and output values.

## Evidence

```text
cargo test -p kaveon-exec window::tests -- --nocapture
result: 13 passed, 0 failed
KAVEON_DIFF_ONLY=partitioned_window_different_orderings cargo test -p kaveon-server --bin kaveon-server differential_tests::the_differential_sweep_matches_across_parquet_encodings_and_execution_paths -- --nocapture
result: 1 passed, 0 failed (40.76s)
cargo check -p kaveon-exec
cargo clippy -p kaveon-exec --all-targets -- -D warnings
cargo fmt --package kaveon-exec -- --check
```

## Boundary

This removes repeated sort work for identical window layouts in one operator. Distinct partition/order layouts still sort independently, and live AKS performance qualification remains open.
