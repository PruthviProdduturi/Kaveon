# T-SQL `PARSENAME` qualification — 2026-11-03

Kaveon now accepts the bounded SQL Server `PARSENAME(value, part)` scalar
form for plain dotted names. Part `1` selects the rightmost segment and part
`4` the leftmost segment. The evaluator returns NULL for a NULL input, NULL or
out-of-range part, empty segments, names with more than four segments, or a
segment longer than SQL Server's `sysname` limit of 128 Unicode characters.

This boundary deliberately excludes bracketed/quoted identifier parsing and
collation-specific object-name rules. Those shapes remain explicit future
compatibility work rather than being silently approximated.

Evidence:

- `expr_eval::tests::evaluates_parsename_parts_and_rejects_invalid_shapes`
  verifies right-to-left selection, malformed dotted names, and NULL input.
- `logical_plan::tests::lowers_tsql_parsename_as_a_bounded_scalar_function`
  verifies the SQL planner preserves the function for vectorized execution.
- Case `parsename` is included in the versioned T-SQL corpus as a
  `planner_qualified` case with distributed status
  `qualified_for_bounded_plain_dotted_names`.
- The focused distributed differential case
  `tsql_parsename_scalar_compatibility` compares dictionary, plain, multi-file,
  clustered, partitioned, and three-worker fragment results exactly. This is
  local distributed evidence; deployed/cloud and external-client qualification
  remain open.

Focused command:

```text
KAVEON_DIFF_ONLY=tsql_parsename_scalar_compatibility cargo test -p kaveon-server --bin kaveon-server differential_tests::the_differential_sweep_matches_across_parquet_encodings_and_execution_paths -- --exact --nocapture
```
