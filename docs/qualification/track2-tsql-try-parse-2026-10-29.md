# T-SQL `TRY_PARSE` — 2026-10-29

## Scope

Kaveon accepts bounded invariant-culture SQL Server forms:

```sql
TRY_PARSE(value AS BIGINT)
TRY_PARSE(value AS DATE)
```

The parser lowers them to the existing `TRY_CAST` evaluator, preserving NULL
on conversion failure and the Engine's normal typed numeric and temporal
semantics. Temporal parsing is limited to invariant input forms accepted by
the existing safe DATE cast. Culture-aware `USING` clauses and locale-specific
parsing remain explicitly outside this qualification; they are not rewritten
or silently interpreted.

## Evidence

- Planner regression: `lowers_tsql_try_parse_numeric`.
- Exact local-vs-three-worker differential:
  `differential_tests::distributed_tsql_try_parse_matches_local`.
- The planner regression also verifies `TRY_PARSE(value AS DATE)` lowers to
  `TRY_CAST_DATE32`.
- Exact local-vs-three-worker differential for mixed valid/invalid invariant
  DATE values:
  `differential_tests::distributed_tsql_try_parse_invariant_date_matches_local`.
- The versioned T-SQL corpus records `try_parse_numeric` with
  `qualified_for_invariant_numeric_parsing` and `try_parse_invariant_date`
  with `qualified_for_invariant_temporal_inputs`.

This is a bounded SQL Server compatibility alias, not a claim of full CLR-style
locale parsing or general Fabric type-conversion parity.
