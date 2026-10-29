# T-SQL `CONVERT` runtime date styles — 2026-10-29

Kaveon Engine now accepts the bounded SQL Server/Fabric date-style forms over
text columns:

- `CONVERT(DATE, text, style)` for styles `0`, `1`, `23`, `101`, `102`, `103`, `104`, `105`, `106`, `107`, `110`, `111`, `112`, `20`, `21`, `120`, `121`, and `126`;
- `TRY_CONVERT(DATE, text, style)`, returning a typed NULL for malformed rows;
- nullable UTF-8 and LargeUtf8 inputs, with canonical Date32 output.

Literal inputs continue to normalize during planning. Column inputs use the
vectorized `CONVERT_DATE_STYLE_*`/`TRY_CONVERT_DATE_STYLE_*` evaluator, so
format parsing is performed per row without a coordinator-side scan.

The contract deliberately rejects unsupported style codes, dynamic styles,
character-set conversion, and non-DATE style targets. Strict `CONVERT` fails
on the first malformed non-NULL value; `TRY_CONVERT` preserves SQL Server's
NULL-on-conversion-failure behavior.

## Evidence

- Planner regression:
  `logical_plan::tests::lowers_tsql_date_convert_styles`.
- Executor regression:
  `expr_eval::tests::evaluates_tsql_runtime_date_styles_with_try_nulls`.
- Distributed differential case:
  `tsql_runtime_date_style` in
  `engine/crates/server/src/differential_tests.rs`.

This is a bounded date-style slice, not full SQL Server conversion parity.
