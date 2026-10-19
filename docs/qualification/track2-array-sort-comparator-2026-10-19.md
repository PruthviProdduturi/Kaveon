# `ARRAY_SORT(array, comparator)`

## Scope

Kaveon supports Trino's two-argument `ARRAY_SORT` form for a comparator with
exactly two parameters. The lambda is lowered into the serialized logical plan
and evaluated for each pair of elements using the normal vectorized expression
semantics for comparisons, `CASE`, casts, and scalar functions. Null elements
are passed to the comparator as typed nullable values, so a comparator can
choose its own null ordering. The comparator must return a non-NULL integer;
the sign determines the ordering.

The supported lambda is non-capturing: references to the two declared
parameters are bound locally, while outer relation columns are not captured.

## Evidence

Local parser, logical-plan, and evaluator tests:

```powershell
cargo test -p kaveon-sql parses_two_parameter_lambda_for_array_sort -- --nocapture
cargo test -p kaveon-sql lowers_array_sort_comparator_as_a_serializable_lambda_expression -- --nocapture
cargo test -p kaveon-exec array_sort_comparator_uses_lambda_body_and_preserves_result_type -- --nocapture
cargo test -p kaveon-exec array_sort_comparator_orders_nulls_explicitly -- --nocapture
```

The full server differential corpus includes the comparator query and compares
the exact result across dictionary/plain/multi-file/partitioned/clustered
Parquet layouts and the three-worker fragment path:

```powershell
cargo test -p kaveon-server --bin kaveon-server differential_tests::the_differential_sweep_matches_across_parquet_encodings_and_execution_paths -- --nocapture
```

Result on the final comparator implementation: **1 passed, 0 failed**
in 433.26 seconds. The evaluator qualification also covers explicit NULL
ordering in the comparator unit test above.

## Boundary

Capturing outer columns, comparator results that are NULL or non-integer,
arbitrary lambda expressions outside `ARRAY_SORT`, and broader table-function
or lateral shapes remain explicitly rejected. These boundaries prevent a
closure from silently widening a scan or changing distributed exchange
semantics.
