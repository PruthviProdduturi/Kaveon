# Custom `DATEFIRST` read qualification — 2026-11-03

## Contract

Kaveon's bounded SQL Server read layer accepts `SET DATEFIRST 1` through
`SET DATEFIRST 7` before a query. The prefix is removed during parsing and
each `DATEPART(dw|weekday|w, ...)` / `DATEPART(week|wk|ww, ...)` expression is
lowered to an immutable internal function carrying the selected value. This
keeps the result deterministic across local and distributed workers without
mutable session state.

Values outside `1..7`, non-numeric values, and malformed session prefixes fail
closed. The default `DATEFIRST 7` path remains unchanged. Generic Trino
`DATE_PART` keeps its existing numbering.

## Evidence

- Implementation: `76391f46` (`feat(sql): qualify bounded tsql datefirst reads`).
- Parser regression: `parser::tests::accepts_only_default_tsql_read_session_prefixes`.
- Executor regression: `expr_eval::tests::date_part_tsql_week_uses_sunday_first_week_one`,
  including a Monday-first Sunday result of `7`.
- Three-worker differential case: `tsql_set_datefirst_monday` in
  `the_differential_sweep_matches_across_parquet_encodings_and_execution_paths`.
- Focused command:
  `KAVEON_DIFF_ONLY=tsql_set_datefirst_monday cargo test -p kaveon-server --bin kaveon-server the_differential_sweep_matches_across_parquet_encodings_and_execution_paths`
  — passed.

## Boundary

This qualifies the bounded read prefix and date-part expressions only. It does
not claim general mutable SQL Server session state, locale settings, or
cross-source execution.
