# Track 2 qualification: `ARRAY_HAS_DUPLICATES` text and unique arrays

Date: 2026-10-18

## Contract

`ARRAY_HAS_DUPLICATES` returns a boolean for primitive VARCHAR arrays and
preserves the false result for a unique integer array. Equality is evaluated
with the array element type rather than stringifying values.

## Focused evidence

The distributed differential corpus includes
`array_has_duplicates_text_and_unique` across Parquet layouts and the
three-worker fragment path. It compares a repeated text array and a unique
integer array with Trino.

Run from `engine/`:

```powershell
$env:KAVEON_DIFF_ONLY = "array_has_duplicates_text_and_unique"
cargo test -p kaveon-server --bin kaveon-server the_differential_sweep_matches_across_parquet_encodings_and_execution_paths -- --nocapture
```
