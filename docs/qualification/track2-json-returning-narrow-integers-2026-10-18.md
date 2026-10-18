# Track 2 SQL compatibility qualification — JSON narrow integer RETURNING

## Boundary

`JSON_VALUE` and `JSON_QUERY` accept Trino's bounded typed `RETURNING`
clauses for `TINYINT` and `SMALLINT`. The parser lowers these forms to the
existing exact typed cast path, so range validation and malformed values retain
the normal cast behavior.

## Evidence

The differential case `json_scalar_path_returning_narrow_integers` in
`engine/crates/server/src/differential_tests.rs` extracts JSON integer values
as `TINYINT` and `SMALLINT`, then compares local and distributed results over
dictionary, plain, multi-file, clustered, partitioned, and hive-order Parquet
layouts.

Focused validation:

```powershell
$env:KAVEON_DIFF_ONLY = "json_scalar_path_returning_narrow_integers"
cargo test -p kaveon-server the_differential_sweep_matches_across_parquet_encodings_and_execution_paths -- --nocapture
```

The focused differential test passed with exact matching results across all
layouts and execution paths.

## Remaining scope

This adds only the bounded narrow integer RETURNING types. Full SQL/JSON
coercion, path, format, and external-client/cloud differential coverage remain
open.
