# Track 1 qualification: empty-build runtime-filter short circuit

## Scope

An inner or semi join whose bounded build partition contains zero rows has an
empty result regardless of the probe relation. The partitioned executor now
returns an empty runtime-filter operator in that case, so it does not open or
decode probe batches. Outer joins retain their existing path because an empty
build must still emit unmatched probe rows for those join types.

## Evidence

- `engine/crates/exec/src/partitioned.rs`
- `empty_build_runtime_filter_short_circuits_probe`
- The short circuit is gated to `JoinType::Inner | JoinType::Semi` and occurs
  after the existing residual/key checks.

## Validation

Run from `engine/`:

```text
cargo test -p kaveon-exec --lib empty_build_runtime_filter_short_circuits_probe
cargo fmt --all -- --check
cargo clippy -p kaveon-exec --lib -- -D warnings
```

This is a bounded executor qualification. It does not qualify larger join
topologies, disk exhaustion, or sustained mixed-workload behavior.
