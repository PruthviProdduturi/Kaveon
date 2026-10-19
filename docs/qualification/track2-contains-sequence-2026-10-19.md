# Track 2 qualification: `CONTAINS_SEQUENCE`

The Engine supports Trino's bounded `CONTAINS_SEQUENCE(array, sequence)`
function for regular and nested comparable array elements. It returns true
only when the sequence occurs in consecutive order, treats two NULL elements
as equal, preserves NULL for a NULL input array, and returns true for an empty
sequence. Matching uses the Engine's recursive null-safe equality path so
arrays, rows, and maps do not fall back to string representations.

Evidence:

- `contains_sequence_matches_consecutive_values_with_nulls` in the execution
  evaluator tests consecutive values and NULL elements.
- `contains_sequence` in the distributed differential corpus compares the
  local and three-worker paths across the standard Parquet layouts.
- `cargo clippy --workspace --all-targets -- -D warnings` passes.

This is a scalar array function qualification. Full external-client and cloud
qualification remains part of the broader Track 2 boundary.
