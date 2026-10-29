# T-SQL `DATETIME2FROMPARTS` qualification — 2026-10-29

Status: **qualified for the bounded read contract**.

The Engine lowers `DATETIME2FROMPARTS(year, month, day, hour, minute,
second, fractions, precision)` through typed date, second, and millisecond
operators. Precision values `0` through `3` are represented exactly by the
Engine's timestamp path; precision above `3` is rejected during planning to
avoid silently discarding sub-millisecond data.

## Evidence

- Implementation: the follow-on commit to the current branch after `62cff92b`.
- Planner regression:
  `lowers_tsql_datetime2fromparts_with_bounded_precision`.
- Versioned corpus: 49 cases, including a precision-3 constructor.
- Differential case: `tsql_datetime2fromparts`, executed against the local and
  three-worker-style paths with exact result comparison.
- Gates passed: formatting, focused SQL tests, corpus boundary test,
  differential sweep, strict Clippy for SQL/server, and documentation
  validation.

## Boundary

This does not claim SQL Server's 100-nanosecond precision (`precision = 4`–`7`),
`TIMEFROMPARTS`, `DATETIMEOFFSETFROMPARTS`, or general procedural T-SQL parity.
