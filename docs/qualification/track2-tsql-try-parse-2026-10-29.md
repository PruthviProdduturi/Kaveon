# T-SQL `TRY_PARSE` — 2026-10-29

## Scope

Kaveon accepts the bounded invariant-culture SQL Server form:

```sql
TRY_PARSE(value AS BIGINT)
```

The parser lowers it to the existing `TRY_CAST` evaluator, preserving NULL on
conversion failure and the Engine's normal typed numeric semantics. Culture-aware
`USING` clauses and locale-specific parsing remain explicitly outside this
qualification; they are not rewritten or silently interpreted.

## Evidence

- Planner regression: `lowers_tsql_try_parse_numeric`.
- Exact local-vs-three-worker differential:
  `differential_tests::distributed_tsql_try_parse_matches_local`.
- The versioned T-SQL corpus records the case as `try_parse_numeric` with
  `qualified_for_invariant_numeric_parsing`.

This is a bounded SQL Server compatibility alias, not a claim of full CLR-style
locale parsing or general Fabric type-conversion parity.
