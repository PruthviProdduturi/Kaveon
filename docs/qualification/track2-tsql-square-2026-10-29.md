# T-SQL `SQUARE` — 2026-10-29

`SQUARE(value)` is qualified as a SQL Server compatibility spelling for the
existing vectorized `POWER(value, 2)` operator. The lowering preserves numeric
NULL propagation and uses the same local and distributed execution path as
`POWER`.

Evidence:

- Planner regression: `lowers_tsql_square_to_power`.
- Exact local-vs-three-worker differential:
  `differential_tests::distributed_tsql_square_matches_local`.

This qualification covers numeric scalar inputs; it does not add a separate
numeric type or claim locale-specific SQL Server coercion behavior.
