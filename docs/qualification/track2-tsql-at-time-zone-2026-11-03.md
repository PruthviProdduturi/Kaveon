# Bounded T-SQL `AT TIME ZONE` qualification — 2026-11-03

Kaveon now qualifies the deterministic fixed-offset slice of SQL Server's
`AT TIME ZONE` expression. A DATE or timestamp is converted to a
microsecond-precision timestamp with timezone metadata for `UTC`, `Z`, or a
literal offset from `-14:00` through `+14:00` (minute precision, with the
fourteen-hour endpoints restricted to `:00`). Naive DATE/timestamp values are
interpreted as wall-clock values in the requested offset and normalized to the
UTC epoch; already-zoned timestamp values preserve their instant while
changing the result timezone metadata. NULL values remain NULL.

Named Windows/IANA zones, DST gap/overlap rules, dynamic zone expressions, and
mixed zones in one batch remain explicit rejections. This is intentionally a
deterministic bounded compatibility slice, not a claim of full SQL Server
timezone-database parity.

Evidence:

```text
cargo test -p kaveon-sql --lib lowers_at_time_zone_for_fixed_offsets_and_rejects_named_zones
cargo test -p kaveon-exec --lib at_time_zone
cargo test -p kaveon-server --bin kaveon-server differential_tests::distributed_tsql_at_time_zone_fixed_offset_matches_local -- --nocapture
```

The distributed regression compares the local planner/executor with the
three-worker fragment path and verifies that the query memory pool is fully
released after both runs.
