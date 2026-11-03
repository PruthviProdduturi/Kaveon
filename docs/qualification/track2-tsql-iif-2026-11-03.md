# T-SQL `IIF` qualification — 2026-11-03

Kaveon accepts the bounded SQL Server `IIF(condition, true_value,
false_value)` scalar form. The planner lowers it to the existing searched
`CASE` expression, so local and distributed execution share the same typed
branch selection and three-valued condition semantics. The function requires
exactly three arguments; arbitrary function/table-valued forms are not part of
this qualification.

Evidence:

- `engine/crates/sql/src/logical_plan.rs::lowers_tsql_iif_to_searched_case`
- `engine/crates/server/src/differential_tests.rs::distributed_tsql_iif_matches_local`
- the pinned corpus case `iif_row_branch` in `tsql-read-compatibility-corpus-2026-10-28.json`
- implementation commit `a938c054`

The distributed differential compares the three-worker result with the local
reference and verifies that the memory pool is fully released. This qualifies
the bounded scalar read shape, not full SQL Server `IIF` coercion rules,
procedural use, or general T-SQL compatibility.
