# Track 2 qualification: `ARRAY_DISTINCT` nested arrays

Date: 2026-10-18

## Contract

`ARRAY_DISTINCT` compares nested array elements structurally and retains the
first occurrence. Equal nested arrays collapse to one value, while arrays
with a different element order remain distinct.

## Focused evidence

The distributed differential corpus includes
`array_distinct_nested_arrays` across Parquet layouts and the three-worker
fragment path. It evaluates `[ [1,2], [1,2], [2,1] ]` and compares the exact
two-element result with Trino.

Run from `engine/`:

```powershell
$env:KAVEON_DIFF_ONLY = "array_distinct_nested_arrays"
cargo test -p kaveon-server --bin kaveon-server the_differential_sweep_matches_across_parquet_encodings_and_execution_paths -- --nocapture
```
