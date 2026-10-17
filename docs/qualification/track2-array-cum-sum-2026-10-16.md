# Track 2 `ARRAY_CUM_SUM` qualification

This qualification records the bounded Trino-compatible numeric
`ARRAY_CUM_SUM` surface added on 2026-10-16.

## Semantics covered

- signed and unsigned integer elements are accumulated as checked `bigint`
  values;
- `real` and `double` elements are accumulated as `double` values;
- a null array remains null;
- null elements produce a null at that position and at every following
  position, matching Trino's null-tail behavior;
- integer overflow fails closed with an `ARRAY_CUM_SUM bigint overflow` error;
- the regular and large Arrow list layouts use the same semantics.

## Evidence

From `engine/` on the pinned Rust 1.98.1 toolchain:

```text
cargo fmt --all
cargo test -p kaveon-exec array_cum_sum_preserves_null_tail_and_overflow_contract --quiet
cargo test -p kaveon-server trino_show_functions_exposes_supported_function_metadata --quiet
cargo clippy --workspace --all-targets -- -D warnings
cargo test -p kaveon-server differential_tests::the_differential_sweep_matches_across_parquet_encodings_and_execution_paths --quiet
```

All commands passed. The differential sweep completed with 1 test passed, 0
failed, and 426 filtered tests.

This is a bounded numeric function qualification. Higher-order array
functions, lambda semantics, and the remaining complex element types remain
open in the parity tracker.
