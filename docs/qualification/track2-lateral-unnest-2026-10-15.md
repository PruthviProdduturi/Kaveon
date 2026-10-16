# Track 2: constant lateral `UNNEST` lowering

## Scope

This qualification covers the SQL parser and logical-plan lowering for the
constant Trino shape:

```sql
SELECT *
FROM (VALUES (1)) AS x(id)
CROSS JOIN LATERAL UNNEST(ARRAY[10, 20]) AS u(value)
```

The parser represents this syntax as `TableFactor::Function`, rather than the
dedicated `TableFactor::UNNEST` variant used by the unqualified form. Kaveon now
lowers that function variant to the same `LogicalPlan::Unnest` node, preserving
the supplied output alias.

## Evidence

- Commit: the implementation commit containing `parses_lateral_function_unnest_in_from`.
- Command: `cargo test -p kaveon-sql`.
- Result: 161 tests passed, 0 failed; the focused test passed.
- Formatting: `cargo fmt --all` passed.

## Boundary

This is a local planner qualification. It does not claim correlated lateral
semantics, arbitrary table functions, or AKS wire qualification. Those remain
open in the parity tracker and require a deployed image plus exact-result
differential evidence.
