# T-SQL `PARSENAME` qualification — 2026-11-03

Kaveon now accepts the bounded SQL Server `PARSENAME(value, part)` scalar
form for plain, bracket-delimited, and double-quoted dotted names. Delimited
segments may contain dots and escaped closing delimiters; the result removes
the identifier delimiters. Part `1` selects the rightmost segment and part
`4` the leftmost segment. The evaluator returns NULL for a NULL input, NULL or
out-of-range part, empty segments, names with more than four segments, or a
segment longer than SQL Server's `sysname` limit of 128 Unicode characters.

Collation-specific object-name rules remain outside this bounded contract.

Evidence:

- `expr_eval::tests::evaluates_parsename_parts_and_rejects_invalid_shapes`
  verifies right-to-left selection, malformed dotted names, and NULL input.
- `logical_plan::tests::lowers_tsql_parsename_as_a_bounded_scalar_function`
  verifies the SQL planner preserves the function for vectorized execution.
- Cases `parsename` and `parsename_row_local` are included in the versioned
  T-SQL corpus as `planner_qualified` cases with distributed status
  `qualified_for_bounded_plain_dotted_names` and
  `qualified_for_row_local_text_and_part`.
- The focused distributed differential case
  `tsql_parsename_scalar_compatibility` compares dictionary, plain, multi-file,
  clustered, partitioned, and three-worker fragment results exactly. This is
  local distributed evidence; deployed/cloud and external-client qualification
  remain open.
- `tsql_parsename_quoted_identifiers` adds exact local/three-worker coverage
  for bracketed dots, escaped brackets, and double-quoted segments.

Focused command:

```text
KAVEON_DIFF_ONLY=tsql_parsename_scalar_compatibility cargo test -p kaveon-server --bin kaveon-server differential_tests::the_differential_sweep_matches_across_parquet_encodings_and_execution_paths -- --exact --nocapture
```
