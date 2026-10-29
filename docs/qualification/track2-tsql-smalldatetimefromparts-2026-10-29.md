# T-SQL `SMALLDATETIMEFROMPARTS` qualification — 2026-10-29

Status: **qualified for the bounded read contract**.

The Engine lowers `SMALLDATETIMEFROMPARTS(year, month, day, hour, minute)`
to its typed date/timestamp operators. The result has minute precision:
seconds and fractional seconds are zero. Arity errors are rejected during
planning rather than reaching a worker.

## Evidence

- Implementation: `62cff92b`.
- Planner regression:
  `lowers_tsql_smalldatetimefromparts_to_minute_timestamp`.
- Versioned corpus: 48 cases, pinned at `3c4af55f` in
  `tsql-read-compatibility-corpus-2026-10-28.json`.
- Differential case: `tsql_smalldatetimefromparts`, executed against the
  local and three-worker-style paths with exact result comparison.
- Gates passed: `cargo fmt --all -- --check`, focused SQL tests, corpus
  boundary test, differential sweep, strict Clippy for SQL/server, and
  documentation validation.

## Boundary

This qualifies valid calendar parts and the bounded five-argument constructor
only. It does not claim `TIMEFROMPARTS`, `DATETIME2FROMPARTS`,
`DATETIMEOFFSETFROMPARTS`, invalid-date coercion parity, or general T-SQL
procedural compatibility.
