# T-SQL styled timestamp literals — 2026-10-29

## Scope

The Engine now lowers bounded literal `CONVERT` and `TRY_CONVERT` calls into
`DATETIME`, `DATETIME2`, `SMALLDATETIME`, and `DATETIMEOFFSET` through its
typed microsecond timestamp cast. ISO date-time styles `120`, `121`, and `126`
preserve the supplied time and fractional text; date-only styles produce
midnight. Runtime text-column conversion now has a vectorized evaluator path:
ISO styles `20`, `21`, `120`, `121`, and `126` preserve the time, while the
date-only styles use the existing exact date parser and midnight semantics.

## Evidence

- Planner regression: `lowers_styled_tsql_timestamp_literals`.
- Executor regression: `evaluates_tsql_runtime_timestamp_styles_with_try_nulls`.
- Distributed differential: `distributed_tsql_runtime_timestamp_styles_match_local`
  compares ISO styles `20`, `21`, `120`, `121`, and `126`, plus date-only style
  `101`, across three workers.
- Corpus case: `convert_timestamp_styles_literal`.
- The versioned corpus records this as `planner_qualified` with distributed
  status `qualified_for_literal_iso_and_date_only_styles`. The focused
  differential exercises the literal-only projection through dictionary,
  plain, multi-file, clustered, partitioned, and three-worker fragment paths.

## Boundary

Styles `20`, `21`, `120`, `121`, `126`, and `101` are qualified through the
local and three-worker differential. External-client qualification remains
open. This does not claim full SQL Server temporal parsing, timezone conversion
parity, or precision beyond the Engine's microsecond timestamp representation.
