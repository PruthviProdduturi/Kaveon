# Track 2 T-SQL `DATE_BUCKET` subsecond qualification — 2026-11-02

## Scope

The bounded T-SQL `DATE_BUCKET` lowering now accepts `millisecond` and
`microsecond` dateparts, together with SQL Server's `ms`, `mcs`, and `us`
aliases, for timestamp inputs. It reuses the typed `DATE_DIFF`
and timestamp add operators, so the bucket width remains a positive literal or
bounded row-local BIGINT expression and the origin remains the SQL Server
default, a typed timestamp/date literal, or a bounded row-local temporal
expression.

Nanosecond buckets remain explicitly rejected because Kaveon's execution type
contract currently represents timestamps at microsecond precision. The parser
must not accept a nanosecond spelling and silently lose precision.

## Evidence

From `engine/`:

```text
cargo fmt --all -- --check
cargo test -p kaveon-sql bounded_date_bucket_lowers_literal_widths_and_row_local_widths -- --nocapture
cargo test -p kaveon-server distributed_tsql_date_bucket_subsecond_matches_local -- --nocapture
```

Results:

- planner lowering passed for millisecond and microsecond buckets;
- nanosecond input was rejected with the bounded-contract diagnostic;
- the exact local result matched the three-worker distributed result for both
  subsecond units and all three SQL Server aliases;
- the differential query used deterministic ordering and the memory pool
  returned to zero after execution.

## Boundary

This qualifies timestamp millisecond and microsecond buckets only. It does not
claim nanosecond timestamp support, culture-specific date semantics, or an
unbounded dynamic datepart expression.
