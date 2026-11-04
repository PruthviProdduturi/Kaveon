# T-SQL `DATEPART(tzoffset, ...)` qualification — 2026-11-03

Kaveon now supports the bounded SQL Server `DATEPART(tzoffset, value)` form
for timestamp values carrying a fixed `UTC`, `Z`, or `±HH:MM` timezone. The
result is the signed offset in minutes, and NULL source values remain NULL.
Naive timestamps, named time zones, malformed offsets, and offsets outside
the fixed-offset range are rejected rather than guessed.

## Evidence

- `expr_eval::tests::date_part_tzoffset_reads_fixed_timestamp_timezone`
- Differential case `tsql_datepart_tzoffset_fixed_offset` across the local,
  partitioned, clustered, fragmented, and three-worker paths.
- Engine implementation commit: `cfbf13f3`.

The feature composes with the already-qualified fixed-offset `AT TIME ZONE`
lowering; it does not claim named-zone or daylight-saving semantics.
