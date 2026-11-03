# T-SQL `YEAR` / `MONTH` / `DAY` aliases — 2026-11-03

The Engine now qualifies the common SQL Server date-part aliases
`YEAR(value)`, `MONTH(value)`, and `DAY(value)`. Each alias lowers to the
existing typed `DATE_PART` evaluator with an explicit date-part literal. This
keeps null propagation, DATE/TIMESTAMP input typing, and local/distributed
execution on one implementation path rather than duplicating calendar logic.

## Boundary

- Exactly one DATE or TIMESTAMP expression is accepted.
- The result is the corresponding integer date part and preserves NULL input.
- The aliases are read-only scalar expressions and work in projections and
  predicates wherever the existing `DATE_PART` path is valid.
- Wrong arity, unsupported input types, and procedural SQL remain rejected by
  the normal planner/evaluator diagnostics.

## Evidence

- Planner regression: `lowers_tsql_year_month_day_aliases` in
  `engine/crates/sql/src/logical_plan.rs`.
- Distributed differential case: `tsql_year_month_day_aliases` in
  `engine/crates/server/src/differential_tests.rs`.
- The authoritative corpus is now 140 cases in
  `docs/qualification/tsql-read-compatibility-corpus-2026-10-28.json`.

The qualification is intentionally limited to the typed date-part contract;
it does not claim general SQL Server temporal or procedural parity.
