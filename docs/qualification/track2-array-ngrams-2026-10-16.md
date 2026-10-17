# Track 2 `ARRAY_NGRAMS` qualification

This qualification records the bounded Trino-compatible `ARRAY_NGRAMS`
surface added on 2026-10-16.

## Semantics covered

- consecutive, overlapping windows are emitted in input order;
- `n` is a positive integer and invalid or zero values fail closed;
- an `n` larger than the input produces an empty array;
- null arrays or null `n` values remain null;
- regular and large Arrow list inputs preserve the element type, including
  nested values through the generic list path.

## Evidence

From `engine/` on the pinned Rust 1.98.1 toolchain:

```text
cargo fmt --all
cargo test -p kaveon-exec array_ngrams_emits_consecutive_windows_and_empty_oversized_windows --quiet
cargo test -p kaveon-server trino_show_functions_exposes_supported_function_metadata --quiet
cargo clippy --workspace --all-targets -- -D warnings
cargo test -p kaveon-server differential_tests::the_differential_sweep_matches_across_parquet_encodings_and_execution_paths --quiet
```

All commands passed. The differential sweep completed with 1 test passed, 0
failed, and 426 filtered tests.

This is a bounded array-function qualification. Higher-order array functions,
lambda semantics, and remaining complex-element coverage remain open in the
parity tracker.
