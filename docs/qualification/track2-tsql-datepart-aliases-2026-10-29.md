# T-SQL `DATEPART` alias qualification — 2026-10-29

The Engine now accepts the SQL Server datepart aliases `weekday`, `dw`, and
`w` for the existing day-of-week result, plus `dayofyear`, `dy`, and `y` for
the existing day-of-year result. The implementation does not change
`DATEFIRST` or claim SQL Server session-dependent weekday numbering; it keeps
Kaveon's documented deterministic calendar convention.

The same aliases are accepted by `DATEADD` as day-equivalent offsets. This
does not claim business-day or calendar-holiday behavior.

Evidence:

- Executor regression: `date_part_accepts_tsql_weekday_and_dayofyear_aliases`.
- Planner regression: `lowers_common_tsql_scalar_aliases`.
- Differential cases: `tsql_datepart_weekday_alias`,
  `tsql_datepart_dayofyear_alias`, `tsql_dateadd_weekday_alias`, and
  `tsql_dateadd_dayofyear_alias`.
- Corpus cases: `datepart_weekday_alias`, `datepart_dayofyear_alias`,
  `dateadd_weekday_alias`, and `dateadd_dayofyear_alias`.
