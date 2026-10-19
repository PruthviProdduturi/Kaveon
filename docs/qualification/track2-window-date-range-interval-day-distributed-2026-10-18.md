# Track 2 window DATE RANGE interval qualification — 2026-10-18

Kaveon now lowers Trino-style `INTERVAL '<n>' DAY PRECEDING/FOLLOWING`
bounds in a `RANGE` window frame and evaluates them against Arrow `DATE`
(`Date32`) ordering values. The interval form is kept distinct from numeric
row offsets, so a temporal frame cannot silently be interpreted as a row count.

The local executor regression covers a two-day preceding and one-day following
frame across non-contiguous dates. The server differential regression executes
the same temporal frame locally and through the three-worker fragment path and
requires byte-equivalent canonical rows with no retained query-pool bytes.

Supported in this qualification: positive literal `INTERVAL ... DAY` bounds,
`DATE` ordering, one ordering expression, and `ROWS`/`GROUPS` behavior
unchanged. Month/year intervals, timestamp intervals, expression-valued
bounds, and distributed global window exchange remain outside this bounded
qualification and stay tracked as open parity work.

Evidence:

- `kaveon-sql`: `parses_temporal_range_interval_day_bounds`
- `kaveon-exec`: `date_range_interval_day_offsets_use_calendar_day_values`
- `kaveon-server`: `distributed_date_range_interval_day_matches_local`
