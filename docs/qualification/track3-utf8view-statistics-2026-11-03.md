# `Utf8View` statistics qualification — 2026-11-03

## Contract

The statistics layer treats Arrow `Utf8View` columns as logical text for
distinct-count sketches and exact text bounds. `fold_distinct` and
`fold_distinct_row` hash the same UTF-8 bytes as ordinary `Utf8` and
`LargeUtf8` arrays, and full profiles compute lexicographic min/max values,
while quantile sketching remains intentionally unavailable for text.

This keeps `ANALYZE` and statistics-backed distinct answers valid when a
Parquet reader supplies a view-backed text array. It does not change the
documented UTF-8 storage contract or claim UTF-16 SQL Server semantics.

## Evidence

- `sketch::tests::array_folds_hash_the_canonical_text_row_by_row_and_state_their_error`
  covers `Utf8View` alongside ordinary UTF-8 and dictionary inputs.
- `table_statistics::tests::utf8_view_profiles_preserve_text_bounds_and_distinct_values`
  covers NULLs, text bounds, and repeated values in a full profile.
- `cargo test -p kaveon-core --lib` — 92 passed
- `cargo test -p kaveon-storage --lib table_statistics::tests::utf8_view_profiles_preserve_text_bounds_and_distinct_values` — passed
- `cargo clippy -p kaveon-core --all-targets -- -D warnings` — passed
- `cargo clippy -p kaveon-storage --all-targets -- -D warnings` — passed
- `cargo fmt --all -- --check` — passed

## Boundary

This is a statistics/sketch correctness fix. It does not add quantiles for
text, alter query planning, or broaden unsupported storage types.
