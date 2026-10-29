# T-SQL `DATE_BUCKET` qualification — 2026-10-29

## Scope

This qualification covers the bounded Fabric/Azure SQL `DATE_BUCKET` surface
implemented by Kaveon Engine:

- `DATE_BUCKET(<datepart>, <positive-literal-width>, <date-or-timestamp>)`;
- dateparts `year`, `quarter`, `month`, `day`, `hour`, `minute`, and `second`;
- literal or unquoted datepart identifiers;
- SQL Server's default origin (`1900-01-01`) when no origin is supplied;
- nullable date/timestamp values through the existing typed `DATE_TRUNC`,
  `DATE_DIFF`, normalized remainder, and typed date-add evaluators on the
  distributed fragment path.

The following remain explicitly unsupported and are rejected before execution:

- non-positive, dynamic, or otherwise non-literal widths;
- custom origin arguments;
- unsupported dateparts and dynamic width/datepart expressions.

This is intentionally not a claim of full SQL Server `DATE_BUCKET` parity.

## Evidence

- SQL planner regression:
  `logical_plan::tests::bounded_date_bucket_lowers_literal_widths_and_rejects_unqualified_forms`.
- Exact local/distributed differential cases:
  `date_bucket_width_one` and `date_bucket_wider_width` in
  `engine/crates/server/src/differential_tests.rs`, covering month and day
  buckets (including three-month and seven-day widths) across dictionary,
  plain, partitioned, clustered, and fragmented layouts.
- The complete differential sweep remains the release gate for the broader
  corpus; the focused case is the authoritative feature qualification.

## Contract

Width one lowers directly to `DATE_TRUNC`. Wider positive literal widths lower
to `DATE_TRUNC` plus `DATE_DIFF`, a normalized modulo remainder, and the
typed calendar or fixed-duration date-add operator. No unqualified form
reaches a worker, and no custom-origin semantics are silently approximated.
