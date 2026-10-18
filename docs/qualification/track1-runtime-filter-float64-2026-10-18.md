# Track 1 — Float64 Parquet runtime-filter qualification

## Scope

Float64 equality literals must cross the runtime-filter boundary without narrowing or rounding when the probe column is Parquet `DOUBLE`. This qualification uses a two-row-group local Parquet fixture and verifies that only the exact matching value is returned after the filter is accepted.

## Evidence

From the `engine` workspace:

```text
cargo test -p kaveon-storage --lib parquet_reader applies_float64_runtime_filter_without_rounding --quiet
cargo fmt --all -- --check
cargo clippy -p kaveon-storage --all-targets -- -D warnings
```

The focused regression writes `1.5`, `2.25`, `3.5`, and `4.75` as Float64 values across two row groups, applies the Float64 runtime literal `2.25`, and observes exactly `[2.25]` after Parquet decoding.

## Boundary

This is local Parquet evidence. Cloud/object/ADLS Float64 qualification and the deployed-image gate remain open; this test does not claim those paths.
