# Track 1 window exchange qualified-key guard — 2026-10-18

## Scope

The distributed window planner may hash only columns that are present at the
exchange boundary.  It normally removes relation qualifiers from a column
reference (`orders.customer_id` becomes `customer_id`).  A pair of distinct
qualified references with the same leaf name, such as `left.id` and
`right.id`, cannot safely be represented by that unqualified exchange key.

## Change

The planner now fails closed to the single-task `FinalWindow` path whenever a
window partition specification contains two different relation qualifiers for
the same leaf column.  Safe same-column windows continue to use the existing
hash exchange; mixed relation-qualified keys no longer risk silently dropping
one key during partitioning.

## Evidence

- Planner regression: `keeps_qualified_leaf_collisions_on_single_final_stage`.
- The test asserts `PARTITION BY left.id, right.id` is not eligible for a
  `PartitionedWindow` exchange.
- This is a correctness guard, not a claim of broader mixed-key parallel
  execution.  Qualified output identity must be carried through the exchange
  before that shape can be distributed safely.

## Remaining scope

Mixed partition specifications, unpartitioned windows, and qualified-key
exchange identity remain on the centralized correctness path.  A future
qualified exchange-key representation can remove this guard after local and
distributed differential qualification.
