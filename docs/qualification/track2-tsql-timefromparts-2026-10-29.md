# T-SQL `TIMEFROMPARTS` qualification — 2026-10-29

Kaveon lowers `TIMEFROMPARTS(hour, minute, seconds, fractions, precision)`
to a typed `TIME64(MICROSECOND)` vectorized operator. This is a bounded
read-compatibility contract, not general SQL Server temporal parity.

## Qualified contract

- Five arguments are required; incorrect arity is rejected during planning.
- Hour, minute, and seconds must be in `0..23`, `0..59`, and `0..59`.
- Precision is supported from `0` through `6`, with the fraction scaled to
  microseconds without floating-point conversion.
- Fractions must be non-negative and smaller than `10^precision`.
- A NULL in any argument produces a NULL result for that row.
- Invalid ranges, precision, and arithmetic overflow fail closed.

## Evidence

- Planner regression: `lowers_tsql_timefromparts_to_typed_time_operator`.
- Executor regression: `time_from_parts_preserves_precision_ranges_and_nulls`.
- Differential case: `tsql_timefromparts` in the local/three-worker sweep.
- Versioned corpus case: `timefromparts` in
  `docs/qualification/tsql-read-compatibility-corpus-2026-10-28.json`.

The corpus remains the authoritative acceptance list; its commit pin is
updated only after the focused tests and documentation validation pass.

## Explicit boundary

This qualification does not claim precision above six fractional digits,
`DATETIMEOFFSETFROMPARTS`, timezone-offset semantics, or procedural T-SQL.
