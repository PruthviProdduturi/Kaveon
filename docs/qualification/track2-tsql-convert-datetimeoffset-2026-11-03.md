# T-SQL `CONVERT(DATETIMEOFFSET)` — 2026-11-03

## Scope

Kaveon qualifies the unstyled SQL Server form:

```sql
CONVERT(DATETIMEOFFSET, value)
```

The accepted input boundary is invariant ISO text with a fixed numeric offset
(`Z`, `+HH:MM`, or `-HH:MM`), plus native timestamp values and NULL. Values are
normalized to the Engine's UTC instant representation with a `+00:00` timezone
marker. A non-NULL value that cannot be parsed fails the statement; NULL stays
NULL. Named zones, locale-specific parsing, and dynamic timezone rules are not
claimed.

## Evidence

- Planner regression: `logical_plan::tests::lowers_unstyled_tsql_convert_aliases`.
- Executor regression: `expr_eval::tests::evaluates_strict_datetimeoffset_conversion_and_rejects_invalid_text`.
- Exact local-vs-three-worker differential:
  `differential_tests::distributed_tsql_try_cast_temporal_targets_match_local`.
- The versioned corpus records `convert_datetimeoffset` as
  `qualified_for_invariant_fixed_offset_temporal_inputs`.

This is a bounded conversion contract, not general SQL Server locale or timezone
parity.
