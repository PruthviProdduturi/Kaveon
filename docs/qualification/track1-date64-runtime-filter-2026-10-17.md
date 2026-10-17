# Track 1 — Date64 runtime-filter qualification

## Scope

This qualification covers the local Parquet path for Arrow `Date64` columns.
It verifies that an integer millisecond literal is accepted as a runtime-filter
value, that the Parquet statistics/bloom-filter gate accepts the same type, and
that the row-level residual predicate compiler evaluates the comparison without
falling back or silently dropping the filter.

## Evidence

The focused storage tests passed from `engine/`:

```text
cargo test -p kaveon-storage runtime_filter_accepts_date64_columns --quiet
test result: ok. 1 passed; 0 failed

cargo test -p kaveon-storage compares_date64_columns_with_integer_day_millis_literals --quiet
test result: ok. 1 passed; 0 failed
```

The full workspace suite also passed, including the distributed differential
sweep:

```text
cargo test --workspace --no-fail-fast --quiet
427 server tests passed; 0 failed
1 differential sweep passed; 0 failed
```

## Boundary

This is a local Parquet qualification. It does not claim cloud snapshot
qualification, Iceberg date-statistics support, or a universal latency SLO.
Those remain open in the parity tracker.
