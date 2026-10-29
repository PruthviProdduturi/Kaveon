# T-SQL `DATETIME2FROMPARTS` qualification — 2026-10-29

Status: **qualified for the bounded read contract**.

The Engine lowers `DATETIME2FROMPARTS(year, month, day, hour, minute,
second, fractions, precision)` through typed date, second, millisecond, and
microsecond operators. Precision values `0` through `6` are represented
exactly by the Engine's microsecond timestamp path; precision `7` is rejected
during planning because 100-nanosecond storage is not representable.

## Evidence

- Implementation: `0e8441d0`.
- Planner regression:
  `lowers_tsql_datetime2fromparts_with_bounded_precision`.
- Versioned corpus: 75 cases, including a precision-6 constructor.
- Differential case: `tsql_datetime2fromparts`, executed against the local and
  three-worker-style paths with exact result comparison.
- Focused precision-6 differential:
  `distributed_tsql_datetime2fromparts_precision_six_matches_local`.
- Gates passed: formatting, full SQL-library tests, focused distributed
  differential, strict workspace Clippy, corpus boundary test, and
  documentation validation.

## Boundary

This does not claim SQL Server's 100-nanosecond precision (`precision = 7`),
`TIMEFROMPARTS`, `DATETIMEOFFSETFROMPARTS`, or general procedural T-SQL parity.
