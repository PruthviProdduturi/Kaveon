# T-SQL `DATEFIRST` midweek qualification — 2026-11-03

## Boundary

The Engine accepts a literal `SET DATEFIRST 3` prefix and carries the selected
weekday origin into the immutable distributed plan. `DATEPART(dw, ...)` uses
the same value on the coordinator and every worker; this is a read-compatibility
qualification, not a claim of session mutation or general T-SQL parity.

## Evidence

- Corpus case: `set_datefirst_wednesday`.
- SQL: `SET DATEFIRST 3; SELECT DATEPART(dw, event_date) FROM t`.
- Differential case: `tsql_set_datefirst_wednesday`.
- The differential sweep compares local execution with three-worker execution
  across dictionary, plain, partitioned, clustered, and fragmented Parquet
  layouts.
- Focused command:

  ```text
  KAVEON_DIFF_ONLY=tsql_set_datefirst_wednesday cargo test -p kaveon-server --bin kaveon-server the_differential_sweep_matches_across_parquet_encodings_and_execution_paths -- --nocapture
  ```

- Result: one focused test passed with zero failures.

## Limits

Only literal values from 1 through 7 are accepted. Invalid values and dynamic
session expressions remain explicitly rejected. The implementation does not
claim compatibility with session state that changes after a statement has been
planned.
