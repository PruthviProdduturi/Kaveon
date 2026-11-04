# T-SQL `TRY_PARSE` — 2026-10-29

## Scope

Kaveon accepts bounded invariant-culture SQL Server forms:

```sql
TRY_PARSE(value AS BIGINT)
TRY_PARSE(value AS DATE)
TRY_PARSE(value AS DATETIME2)
TRY_PARSE(value AS DATETIMEOFFSET)
TRY_CONVERT(DATETIMEOFFSET, value)
CONVERT(DATETIMEOFFSET, value)
```

The parser lowers them to the existing `TRY_CAST` evaluator, preserving NULL
on conversion failure and the Engine's normal typed numeric and temporal
semantics. Temporal parsing is limited to invariant input forms accepted by
the existing safe DATE, DATETIME2, and fixed-offset DATETIMEOFFSET casts.
DATETIMEOFFSET values are normalized to the Engine's UTC instant representation
with a `+00:00` timezone marker. Culture-aware `USING` clauses and
locale-specific
parsing remain explicitly outside this qualification; they are not rewritten
or silently interpreted.

## Evidence

- Planner regression: `lowers_tsql_try_parse_numeric`.
- Exact local-vs-three-worker differential:
  `differential_tests::distributed_tsql_try_parse_matches_local`.
- The planner regression also verifies `TRY_PARSE(value AS DATE)` lowers to
  `TRY_CAST_DATE32`.
- Exact local-vs-three-worker differential for mixed valid/invalid invariant
  DATE values: `differential_tests::distributed_tsql_try_parse_invariant_date_matches_local`.
- Exact local-vs-three-worker differential for mixed valid/invalid invariant
  DATETIME2 values:
  `differential_tests::distributed_tsql_try_parse_invariant_datetime2_matches_local`.
- Exact local-vs-three-worker differential for mixed valid/invalid invariant
  fixed-offset DATETIMEOFFSET values:
  `differential_tests::distributed_tsql_try_parse_invariant_datetimeoffset_matches_local`.
- The same differential covers unstyled `TRY_CONVERT(DATETIMEOFFSET, ...)`
  through the shared safe-cast path, including invalid input returning NULL.
- Direct `CONVERT(DATETIMEOFFSET, ...)` uses the same fixed-offset parser but
  fails the statement when a non-NULL input is invalid, matching strict cast
  behavior.
- The versioned T-SQL corpus records `try_parse_numeric` with
  `qualified_for_invariant_numeric_parsing`, `try_parse_invariant_date`, and
  `try_parse_invariant_datetime2` with `qualified_for_invariant_temporal_inputs`,
  and `try_parse_invariant_datetimeoffset` with
  `qualified_for_invariant_offset_temporal_inputs`.

This is a bounded SQL Server compatibility alias, not a claim of full CLR-style
locale parsing or general Fabric type-conversion parity.
