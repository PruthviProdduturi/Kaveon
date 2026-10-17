# Track 2 Decimal128 `ARRAY_AVERAGE` qualification

`ARRAY_AVERAGE` now accepts Decimal128 list elements and returns a `double`,
preserving the existing null and empty-array behavior.

From `engine/` on Rust 1.98.1:

```text
cargo fmt --all
cargo test -p kaveon-exec array_average_and_sum_accept_decimal_elements --quiet
cargo clippy --workspace --all-targets -- -D warnings
cargo test -p kaveon-server differential_tests::the_differential_sweep_matches_across_parquet_encodings_and_execution_paths --quiet
```

All commands passed. The differential sweep completed with 1 test passed, 0
failed, and 426 filtered tests. Broader higher-order array and complex-type
coverage remains open in the parity tracker.
