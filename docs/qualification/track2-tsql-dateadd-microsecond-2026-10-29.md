# T-SQL `DATEADD` microsecond qualification — 2026-10-29

Status: **qualified for microsecond-or-finer timestamp inputs**.

`DATEADD(microsecond, amount, timestamp)` and its `mcs`/`us` abbreviations
lower to a typed microsecond operator. Microsecond timestamps are updated
exactly; nanosecond timestamps add the corresponding integral number of
nanoseconds. Coarser timestamp inputs are rejected rather than silently
rounding.

## Evidence

- Planner regression is included in `date_add_lowers_trino_units_to_typed_operators`.
- Differential case: `tsql_dateadd_microsecond`, with exact local and
  three-worker-style results.
- Versioned corpus: 50 cases, pinned to the implementation commit.
- Formatting, focused SQL tests, corpus validation, differential sweep, strict
  Clippy, and documentation validation all pass.

## Boundary

This does not claim arbitrary datepart coercion, sub-microsecond rounding, or
DATE/TIME inputs for the microsecond operator.
