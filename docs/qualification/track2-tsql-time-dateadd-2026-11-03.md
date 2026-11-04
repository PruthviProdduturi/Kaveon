# T-SQL `DATEADD` over `TIME` — 2026-11-03

## Boundary

Kaveon qualifies fixed-duration SQL Server `DATEADD` arithmetic over
microsecond `TIME` values for `hour`, `minute`, `second`, `millisecond`, and
`microsecond` units. Results preserve the microsecond `TIME` type and wrap
with `rem_euclid(24h)` at midnight. NULL source or offset values remain NULL;
overflowing intermediate arithmetic produces a typed NULL rather than a
worker-dependent result. Calendar units (`day`, `week`, `month`, `quarter`,
and `year`) and nanosecond offsets remain outside this contract because a
time-of-day has no calendar date and the Engine's TIME contract is
microsecond precision.

## Evidence

- Implementation: `7b509256`.
- Corpus case: `time_dateadd` in
  `tsql-read-compatibility-corpus-2026-10-28.json`.
- Local planner and executor tests cover hour, millisecond, and microsecond
  offsets, midnight wrapping, and NULL propagation.
- The distributed differential test
  `distributed_tsql_time_dateadd_matches_local` compares the local result
  with three worker fragments over row-local offsets and deterministic
  ordering.

This is a bounded T-SQL temporal contract, not general SQL Server datepart or
sub-microsecond precision parity.
