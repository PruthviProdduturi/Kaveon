# Track 2 qualification: `ARRAY_POSITION` over `UInt32` arrays

`ARRAY_POSITION` now accepts regular Arrow `List<UInt32>` values and compares
the needle at the list's native width. SQL `UINT32` casts use the same Arrow
type, so the focused differential case exercises parsing, planning, local
evaluation, and the in-process three-worker fragment path.

The focused local regression is:

```text
cargo test -p kaveon-exec evaluates_array_position_for_uint32_lists -- --nocapture
```

The exact local and three-worker differential case is selected with:

```text
$env:KAVEON_DIFF_ONLY = "uint32_array_position"
cargo test -p kaveon-server --bin kaveon-server the_differential_sweep_matches_across_parquet_encodings_and_execution_paths -- --nocapture
```

The test includes `u32::MAX`, a missing value, and one-based position results.
