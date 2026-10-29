# Bounded T-SQL `CROSS APPLY`: distributed qualification

## Scope

This qualification covers the bounded SQL Server/Fabric form where `CROSS
APPLY` is followed by a parenthesized derived `SELECT` or `VALUES` relation
with an explicit alias. The parser lowers this form to Kaveon’s existing
lateral-join path; it does not introduce a second execution implementation.

Correlated derived APPLY, table-valued functions, and arbitrary APPLY
expressions remain explicitly rejected. Bounded uncorrelated `OUTER APPLY` is
qualified separately in
[`track2-tsql-outer-apply-distributed-2026-10-29.md`](track2-tsql-outer-apply-distributed-2026-10-29.md).
That boundary prevents a
syntactically accepted query from silently changing null-extension or
function-evaluation semantics.

## Evidence

- SQL parser regression: `parser::tests::bounded_cross_apply_lowers_to_lateral_join`.
- Logical-plan regression: `logical_plan::tests::bounded_cross_apply_lowers_to_a_lateral_plan`.
- Distributed differential case: `tsql_bounded_cross_apply` in
  `server::differential_tests::CASES`.
- The differential gate compares the bounded query across all six local
  Parquet layouts and the three-worker execution path with exact row and value
  equality.
- The compatibility corpus records this case as `planner_qualified` and
  `distributed_status: qualified`.

## Contract

The qualification is intentionally narrow: the derived relation must be
parenthesized, uncorrelated, its body must begin with `SELECT` or `VALUES`, and
it must have an alias. Correlated forms remain fail-closed until their
null-extension and row-by-row evaluation semantics have a dedicated runtime
qualification.

