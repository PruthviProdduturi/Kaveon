# Track 2 qualification: `FILTER(array, lambda)`

Kaveon supports Trino's bounded one-parameter `FILTER` higher-order array
function. The predicate runs once per element, NULL predicates are treated as
false, and the result preserves the input element type. A NULL outer array
remains NULL. Lambda parameters shadow outer columns, while other outer-row
columns remain available to the predicate.

## Evidence

```powershell
cargo test -p kaveon-sql lowers_filter_lambda_with_one_parameter -- --nocapture
cargo test -p kaveon-exec filter_applies_one_parameter_lambda_and_discards_null_predicates -- --nocapture
$env:KAVEON_DIFF_ONLY = 'array_filter_lambda'
cargo test -p kaveon-server --bin kaveon-server differential_tests::the_differential_sweep_matches_across_parquet_encodings_and_execution_paths -- --nocapture
```

The focused parser and executor tests pass. The isolated differential case
passes across dictionary, plain, multi-file, partitioned, clustered, and
three-worker execution paths.

## Boundary

This qualifies a one-parameter scalar boolean predicate. General higher-order
array functions (`TRANSFORM`, `REDUCE`, `ZIP_WITH` with additional lambda
shapes), external-client wire coverage, and cloud qualification remain open.
