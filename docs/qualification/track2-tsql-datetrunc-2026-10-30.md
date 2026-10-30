# T-SQL `DATETRUNC` qualification — 2026-10-30

## Status

`DATETRUNC` is qualified for the bounded read contract through the existing
typed `DATE_TRUNC` execution path.

## Contract

The supported form is `DATETRUNC(datepart, date_or_timestamp)`, where the
datepart and temporal input use the same calendar and precision rules as
`DATE_TRUNC`. The function preserves the input's temporal type and timezone
metadata. Unsupported dynamic dateparts remain rejected by the existing
DATE_TRUNC validation.

## Evidence

- Planner regression: `lowers_tsql_datetrunc_to_date_trunc`.
- The versioned corpus records `datetrunc` as `planner_qualified` with
  `distributed_status: qualified_via_date_trunc`.
- The complete SQL crate suite passes with the corpus count synchronized.

This qualification is a compatibility alias, not a new evaluator or a claim
of broader SQL Server temporal behavior.
