# Track 2 SQL compatibility qualification — nested ROW ordering

## Boundary

Nested ROW values participate in Trino-style lexicographic ordered comparison
when every compared leaf is orderable. Comparison descends into the first
non-equal nested field, preserves three-valued NULL behavior for a NULL leaf,
and does not treat a NULL nested field as a false comparison.

## Evidence

The differential case `nested_row_lexicographic_ordering` in
`engine/crates/server/src/differential_tests.rs` covers:

- a nested non-equal field deciding `<`;
- a later nested row deciding `>` before the outer field;
- a NULL nested leaf producing an unknown ordered result.

The case was run through the local planner and the in-process three-worker
fragment path over dictionary, plain, multi-file, clustered, partitioned, and
hive-order Parquet layouts. Focused validation:

```powershell
$env:KAVEON_DIFF_ONLY = "nested_row_lexicographic_ordering"
cargo test -p kaveon-server the_differential_sweep_matches_across_parquet_encodings_and_execution_paths -- --nocapture
```

The focused differential test passed with exact matching results across all
layouts and execution paths.

## Remaining scope

This qualifies nested orderable ROW leaves only. Maps remain intentionally
non-orderable, and external-client, cloud, and broader complex-type ordering
qualification remain open.
