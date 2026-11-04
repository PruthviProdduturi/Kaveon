# `DATENAME(tzoffset, …)` qualification — 2026-11-03

## Scope

This gate qualifies the bounded SQL Server/Fabric `DATENAME(tzoffset, value)`
form for timestamp values carrying a fixed timezone offset. The result is
canonical signed `+HH:MM` or `-HH:MM` text, matching the SQL Server shape.
NULL input remains NULL. Naive timestamps and named timezone values are
rejected explicitly rather than receiving an invented offset.

This is a read-compatibility qualification. It does not claim locale-aware
formatting, daylight-saving timezone database resolution, or arbitrary
timezone expressions.

## Evidence

Implementation revision: `b827340f` (the corpus `engine_commit` pin).

Focused executor regression:

```text
cargo test -p kaveon-exec date_name_reads_fixed_timestamp_timezone_offset -- --nocapture
```

The regression verifies `+05:30`, NULL propagation, and fail-closed rejection
of a timezone-less timestamp.

Planner and distributed exact-result regression:

```text
cargo test -p kaveon-server distributed_tsql_datename_tzoffset_matches_local -- --nocapture
cargo test -p kaveon-sql datename -- --nocapture
```

The distributed case compares local execution with a three-worker execution
over the same fixture and asserts that every returned offset is `+05:30`.

## Boundary

The corpus entry is `datename_tzoffset_fixed_offset`. The implementation is
qualified for fixed-offset timestamp values only; named zones, naive values,
and locale/session-language formatting remain outside the contract and must
continue to fail closed or use a separately qualified path.
