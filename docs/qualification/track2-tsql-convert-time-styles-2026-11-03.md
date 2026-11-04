# T-SQL `CONVERT` TIME styles — 2026-11-03

## Boundary

Kaveon qualifies the deterministic SQL Server time-text styles that have an
unambiguous microsecond representation:

- styles `8` and `108`: `HH:MM:SS`;
- style `114`: `HH:MM:SS:mmm`, normalized to `HH:MM:SS.mmm`.

Both `CONVERT(TIME, value, style)` and `TRY_CONVERT(TIME, value, style)` are
supported for text expressions, including row-local expressions. `CONVERT`
fails on a malformed non-NULL value; `TRY_CONVERT` returns a typed NULL.
NULL input remains NULL. Invalid clock fields and malformed millisecond text
are rejected rather than guessed. Other SQL Server time styles, culture-aware
forms, and dynamic style expressions remain explicitly unsupported.

## Evidence

- Implementation: `2e9a016a`.
- Corpus case: `convert_time_styles` in
  `tsql-read-compatibility-corpus-2026-10-28.json`.
- Local planner and evaluator tests cover styles 8, 108, and 114, malformed
  strict input, and `TRY_CONVERT` NULL behavior.
- The distributed differential test
  `distributed_tsql_time_convert_styles_match_local` compares the local
  planner/executor with three worker fragments over row-local valid and
  invalid values and a literal conversion.

This is a bounded T-SQL read-compatibility contract, not general SQL Server
date/time style parity.
