# T-SQL `FORMATMESSAGE` qualification — 2026-11-03

Kaveon qualifies the bounded SQL Server `FORMATMESSAGE` scalar form for a
literal template with positional text or numeric arguments. Supported tokens
are `%s`, `%d`, `%i`, `%u`, and `%%`; argument order and literal percent signs
are preserved. `%i` and `%d` use signed decimal text, while `%u` accepts the
Engine's unsigned integer inputs. Templates and rendered values remain bounded by the Engine's
documented limits. Dynamic templates, unsupported conversion tokens, missing
or extra arguments, and oversized output remain explicit rejections.

Evidence:

- `expr_eval::tests::formatmessage_renders_bounded_positional_templates`
  verifies positional text, signed `%i`, unsigned `%u`, and escaped percent
  substitution, including a negative signed value and NULL propagation for
  both a template and a substitution argument.
- `logical_plan` coverage preserves the function for vectorized execution.
- Case `formatmessage_bounded_template` is included in the versioned
  159-case T-SQL corpus as `planner_qualified` with distributed status
  `qualified_for_literal_templates_and_positional_text_or_numeric_arguments`.
- The focused differential case
  `tsql_formatmessage_scalar_compatibility` compares dictionary, plain,
  multi-file, clustered, partitioned, and three-worker fragment results
  exactly for `%s`, `%i`, `%u`, and `%%`. This is local distributed evidence; deployed/cloud and
  external-client qualification remain open.

Focused command:

```text
KAVEON_DIFF_ONLY=tsql_formatmessage_scalar_compatibility cargo test -p kaveon-server --bin kaveon-server differential_tests::the_differential_sweep_matches_across_parquet_encodings_and_execution_paths -- --exact --nocapture
```
