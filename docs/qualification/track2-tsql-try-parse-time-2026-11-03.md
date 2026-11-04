# `TRY_PARSE(... AS TIME)` qualification

Status: qualified for the bounded invariant read contract.

Kaveon now lowers `TRY_PARSE(value AS TIME)` to the existing
`TRY_CAST_TIME` execution path. Text values use the Engine's microsecond
precision time representation; malformed and NULL inputs return typed NULL.
Culture-aware `USING` forms remain explicitly rejected because this contract
does not provide locale-dependent parsing.

Evidence:

- SQL logical-plan regression coverage verifies the lowering target.
- The server differential test
  `distributed_tsql_try_parse_invariant_time_matches_local` compares local
  execution with three-worker execution on valid and malformed inputs.
- The authoritative compatibility corpus records the case as
  `try_parse_invariant_time`.

This qualification does not claim arbitrary locale formats, date-time values
in a TIME target, or precision beyond the Engine's microsecond contract.

The same invariant path is qualified for `DATETIME` and `SMALLDATETIME`.
Both targets use the Engine's microsecond timestamp representation; the
`SMALLDATETIME` name does not imply SQL Server's minute-rounding storage model.
The three-worker regression
`distributed_tsql_try_parse_invariant_datetime_targets_match_local` covers
valid and malformed values for both targets.
