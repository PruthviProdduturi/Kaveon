# T-SQL temporal `TRY_CAST` qualification — 2026-11-03

Kaveon's bounded SQL Server compatibility path qualifies direct
`TRY_CAST(... AS DATE)` and `TRY_CAST(... AS DATETIME2)` projections. Invalid
text follows the safe-cast contract and returns typed NULL rather than failing
the query; valid ISO date and timestamp text retains the target type.

Evidence:

- `distributed_tsql_try_cast_temporal_targets_match_local` compares the exact
  local result with a three-worker execution over the same immutable fixture.
- The authoritative T-SQL corpus adds `try_cast_temporal` and records the
  invalid-value NULL boundary.
- Numeric `TRY_CAST` and invariant `TRY_PARSE` remain separately qualified;
  culture-aware `TRY_PARSE ... USING` is still rejected.

This is a bounded read-compatibility qualification, not a claim of complete
SQL Server conversion-style or locale emulation.
