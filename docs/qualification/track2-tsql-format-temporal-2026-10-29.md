# Temporal `FORMAT` and runtime date formatting qualification

Date: 2026-10-29  
Scope: bounded temporal formatting in the Engine read path  
Status: qualified for the declared invariant patterns

## Contract

The Engine accepts `FORMAT(value, pattern)` for typed `DATE` and `TIMESTAMP`
values and lowers it to the vectorized temporal formatter used by `TO_CHAR`.
`DATE_FORMAT` and `TO_CHAR` now honor their per-row pattern argument instead of
discarding it and emitting a fixed ISO string.

The qualified pattern vocabulary is:

- Trino/MySQL-style `%Y`, `%m`, `%d`, `%H`, `%i`, `%s`, `%f`, and `%%`;
- invariant date/time forms `yyyy`, `YYYY`, `MM`, `dd`, `DD`, `HH`, `HH24`,
  `mm`, `MI`, `ss`, `SS`, `fff`, and `FFF`.

Literal punctuation and other characters are retained. Numeric formatting,
culture-specific SQL Server `FORMAT` overloads, and locale-dependent names are
explicitly outside this contract and fail closed through the existing temporal
type checks.

## Evidence

- Planner lowering: `lowers_tsql_format_to_bounded_temporal_formatter` in
  `engine/crates/sql/src/logical_plan.rs`.
- Vectorized local evaluation: `formats_temporal_values_with_runtime_patterns`
  in `engine/crates/exec/src/expr_eval.rs`, covering DATE, timestamp, NULL
  propagation, SQL format tokens, and invariant SQL Server tokens.
- Distributed differential sweep:
  `the_differential_sweep_matches_across_parquet_encodings_and_execution_paths`
  in `engine/crates/server/src/differential_tests.rs`, including
  `tsql_format_temporal` across dictionary, plain, partitioned, clustered, and
  fragmented layouts and local versus three-worker execution.

## Gate result

The targeted planner and executor tests passed, and the complete differential
sweep passed with the new case. The versioned T-SQL corpus is now 87 cases and
the tracker pin must be updated to the qualification commit before release.
