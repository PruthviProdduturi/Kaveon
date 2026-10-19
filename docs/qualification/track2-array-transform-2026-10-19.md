# Track 2 qualification: `TRANSFORM(array, lambda)`

Kaveon supports Trino's bounded one-parameter `TRANSFORM` higher-order array
function. The lambda runs once for every element, including typed NULL
elements, and its scalar result becomes the output element. A NULL outer array
remains NULL. Lambda parameters shadow outer columns, while other outer-row
columns remain available to the lambda.

## Evidence

```powershell
cargo test -p kaveon-sql lowers_transform_lambda_with_one_parameter -- --nocapture
cargo test -p kaveon-exec transform_applies_one_parameter_lambda_and_preserves_nulls -- --nocapture
$env:KAVEON_DIFF_ONLY = 'array_transform_lambda'
cargo test -p kaveon-server --bin kaveon-server differential_tests::the_differential_sweep_matches_across_parquet_encodings_and_execution_paths -- --nocapture
```

The focused parser and executor tests pass. The isolated differential case
passes across dictionary, plain, multi-file, partitioned, clustered, and
three-worker execution paths.

## Boundary

This qualifies a one-parameter scalar transform. General higher-order array
functions (`REDUCE`, additional lambda arities), external-client wire
coverage, and cloud qualification remain open.
