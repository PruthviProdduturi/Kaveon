# Track 2 — timestamp `RANGE` interval seconds

Date: 2026-10-18

Timestamp ordering now accepts fixed-unit `INTERVAL '<n>' SECOND` offsets,
including fractional seconds and explicit `MILLISECOND`, `MICROSECOND`, and
`NANOSECOND` units in `RANGE` window frames. The parser records the interval as a temporal bound,
and the executor converts seconds to the physical Arrow timestamp precision
(`SECOND`, `MILLISECOND`, `MICROSECOND`, or `NANOSECOND`) with checked
arithmetic. This is fixed-duration arithmetic; it does not apply calendar
month, year, or daylight-saving rules.

The SQL planner regression covers both preceding and following bounds. The
executor regression covers `TIMESTAMP` ordering with gaps between rows and
checks the exact rolling `SUM` frame membership. Three-worker differentials
use a timestamp cast over the events fixture and compare local output with
the in-process three-worker fragment path, including the same preceding and
following 24-hour boundaries. An explicit `MILLISECOND` interval has the same
exact local/distributed result.

Validation passed:

```text
cargo test -p kaveon-sql parses_explicit_subsecond_temporal_range_units_as_nanos -- --nocapture
test result: ok. 1 passed; 0 failed

cargo test -p kaveon-exec timestamp_range_fractional_second_offsets_scale_to_milliseconds -- --nocapture
test result: ok. 1 passed; 0 failed

cargo test -p kaveon-server --bin kaveon-server distributed_timestamp_range_explicit_millisecond_matches_local -- --nocapture
test result: ok. 1 passed; 0 failed
```

The scope is intentionally bounded to nonnegative `SECOND` intervals with at most nine fractional digits over
`TIMESTAMP` ordering and integer explicit subsecond units. Calendar month/year
intervals, expression-valued bounds, and full distributed window exchange
remain open.
