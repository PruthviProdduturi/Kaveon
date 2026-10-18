# Track 2 qualification: `ARRAY_POSITION` over `UInt16` arrays

`ARRAY_POSITION` accepts regular Arrow `List<UInt16>` values and compares the
needle at the list's native width. SQL `UINT16` casts already map through the
SQL type and core Arrow representation, so this coverage exercises parsing,
planning, local evaluation, and the in-process three-worker fragment path.

The focused local regression is:

```text
cargo test -p kaveon-exec evaluates_array_position_for_uint16_lists -- --nocapture
```

The exact local and three-worker differential case is selected with:

```text
$env:KAVEON_DIFF_ONLY = "uint16_array_position"
cargo test -p kaveon-server --bin kaveon-server the_differential_sweep_matches_across_parquet_encodings_and_execution_paths -- --nocapture
```

The case includes `u16::MAX` (`65535`), a missing value, and one-based
position results.
