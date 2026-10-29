# T-SQL styled timestamp literals — 2026-10-29

## Scope

The Engine now lowers bounded literal `CONVERT` and `TRY_CONVERT` calls into
`DATETIME`, `DATETIME2`, `SMALLDATETIME`, and `DATETIMEOFFSET` through its
typed microsecond timestamp cast. ISO date-time styles `120`, `121`, and `126`
preserve the supplied time and fractional text; date-only styles produce
midnight. The contract is planner-qualified for literals only.

## Evidence

- Planner regression: `lowers_styled_tsql_timestamp_literals`.
- Corpus case: `convert_timestamp_styles_literal`.
- The versioned corpus records this as `planner_qualified`; distributed
  qualification is intentionally not claimed for a literal-only planner shape.

## Boundary

Styled conversion of runtime text columns to timestamp types remains rejected
until a vectorized timestamp-style evaluator and local/three-worker differential
evidence are available. This does not claim full SQL Server temporal parsing or
timezone conversion parity.
