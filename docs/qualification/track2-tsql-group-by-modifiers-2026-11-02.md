# T-SQL `GROUP BY` modifiers — 2026-11-02

## Scope

This qualification covers the bounded modifier spellings accepted by SQL
Server-compatible clients:

- `GROUP BY region, industry WITH ROLLUP`
- `GROUP BY region, industry WITH CUBE`

The parser retries these forms through the SQL parser dialect that exposes the
shared `GroupByWithModifier` AST, and the planner lowers them to the existing
explicit `ROLLUP(...)`/`CUBE(...)` grouping-set expansion. `WITH TOTALS`, mixed
explicit grouping expressions plus a modifier, and `GROUP BY ALL` remain
explicitly rejected.

## Evidence

- SQL planner tests: `grouping_modifiers_lower_to_explicit_grouping_sets` and
  `unsupported_grouping_modifiers_fail_closed`.
- Three-worker differential test:
  `distributed_group_by_rollup_and_cube_modifiers_match_explicit_forms`.
- The differential test compares canonical result rows from modifier syntax
  with the corresponding explicit grouping-set query and asserts zero residual
  memory after each execution.

## Boundary

This is read compatibility only. It does not add `WITH TOTALS`, arbitrary
grouping modifier combinations, or a claim of full SQL Server `GROUP BY`
parity. Exact result ordering remains subject to the query's `ORDER BY`.
