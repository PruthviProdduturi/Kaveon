# T-SQL `FORMATMESSAGE` qualification — 2026-11-03

Kaveon qualifies the bounded SQL Server `FORMATMESSAGE` scalar form for a
literal template with positional text or numeric arguments. Supported tokens
are `%s`, `%d`, and `%%`; argument order and literal percent signs are
preserved. Templates and rendered values remain bounded by the Engine's
documented limits. Dynamic templates, unsupported conversion tokens, missing
or extra arguments, and oversized output remain explicit rejections.

Evidence:

- `expr_eval::tests::evaluates_formatmessage_literal_template` verifies
  positional substitution, numeric formatting, and escaped percent signs.
- `logical_plan` coverage preserves the function for vectorized execution.
- Case `formatmessage_bounded_template` is included in the versioned
  145-case T-SQL corpus as `planner_qualified` with distributed status
  `qualified_for_literal_templates_and_positional_text_or_numeric_arguments`.
- The focused differential case
  `tsql_formatmessage_scalar_compatibility` compares dictionary, plain,
  multi-file, clustered, partitioned, and three-worker fragment results
  exactly. This is local distributed evidence; deployed/cloud and
  external-client qualification remain open.

Focused command:

```text
KAVEON_DIFF_ONLY=tsql_formatmessage_scalar_compatibility cargo test -p kaveon-server --bin kaveon-server differential_tests::the_differential_sweep_matches_across_parquet_encodings_and_execution_paths -- --exact --nocapture
```
