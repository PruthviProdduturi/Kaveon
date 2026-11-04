# T-SQL `DATEPART(week, ...)` qualification — 2026-11-03

The SQL Server `DATEPART(week|wk|ww, value)` aliases now use the documented
default `DATEFIRST 7` boundary: Sunday starts a week and the week containing
January 1 is week 1. This is distinct from the generic Engine/Trino week
extractor. Quarter aliases remain on the same typed calendar path.

## Evidence

- `expr_eval::tests::date_part_tsql_week_uses_sunday_first_week_one`
- Differential case `tsql_datepart_week_quarter_alias` across local,
  partitioned, clustered, fragmented, and three-worker execution.
- Engine implementation commit: `5b2d92c7`.

Session-level `SET DATEFIRST` and locale-specific week rules remain outside
the bounded contract.
