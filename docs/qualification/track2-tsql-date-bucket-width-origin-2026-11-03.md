# T-SQL `DATE_BUCKET` width and origin qualification — 2026-11-03

## Scope

This qualification records the corrected bounded `DATE_BUCKET` lowering for
calendar widths and custom origins. It covers the behavior that is easy to get
wrong when the requested bucket unit is not the engine's arithmetic unit:

- year and quarter widths normalize to 12 and 3 months respectively;
- week widths normalize to seven days;
- hour and minute widths use seconds for the typed add operator;
- a typed custom origin is advanced by the complete bucket offset, rather than
  being replaced by the input's truncated calendar boundary.

The supported contract remains bounded: positive literal or row-local BIGINT
widths, typed DATE/TIMESTAMP origins, and the dateparts listed in the engine
compatibility reference. Invalid dynamic widths return typed NULL; unsupported
origin shapes and nanosecond buckets remain explicit rejections.

## Evidence

Planner coverage is in
`engine/crates/sql/src/logical_plan.rs::bounded_date_bucket_lowers_literal_widths_and_row_local_widths`.
The test asserts the normalized fourteen-day width for a two-week bucket, the
3,600-second arithmetic for a two-hour timestamp bucket, and that a typed
custom origin contributes the whole `DATE_DIFF` offset.

The distributed differential suite covers the existing date-bucket cases,
including week, custom-origin, and subsecond timestamp forms:

```text
cargo test -p kaveon-server --bin kaveon-server distributed_tsql_date_bucket -- --nocapture
```

The local SQL qualification command is:

```text
cargo test -p kaveon-sql --lib bounded_date_bucket_lowers_literal_widths_and_row_local_widths -- --nocapture
```

Both gates must pass before the compatibility corpus pin is advanced. This is
read compatibility evidence, not a claim of general Fabric Warehouse parity.
