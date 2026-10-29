# T-SQL `DATE_BUCKET` qualification — 2026-10-29

## Scope

This qualification covers the bounded Fabric/Azure SQL `DATE_BUCKET` surface
implemented by Kaveon Engine:

- `DATE_BUCKET(<datepart>, 1, <date-or-timestamp>)`;
- dateparts `year`, `quarter`, `month`, `day`, `hour`, `minute`, and `second`;
- literal or unquoted datepart identifiers;
- nullable date/timestamp values through the existing typed `DATE_TRUNC`
  evaluator and distributed fragment path.

The following remain explicitly unsupported and are rejected before execution:

- widths other than the literal `1`;
- custom origin arguments;
- unsupported dateparts and dynamic width/datepart expressions.

This is intentionally not a claim of full SQL Server `DATE_BUCKET` parity.

## Evidence

- SQL planner regression:
  `logical_plan::tests::bounded_date_bucket_lowers_to_date_trunc_and_rejects_unqualified_forms`.
- Exact local/distributed differential case:
  `date_bucket_width_one` in
  `engine/crates/server/src/differential_tests.rs`, covering month and day
  buckets across dictionary, plain, partitioned, clustered, and fragmented
  layouts.
- The complete differential sweep remains the release gate for the broader
  corpus; the focused case is the authoritative feature qualification.

## Contract

The bounded form lowers to `DATE_TRUNC` only after validating all arguments.
No unqualified form reaches a worker, and no custom-origin semantics are
silently approximated.
