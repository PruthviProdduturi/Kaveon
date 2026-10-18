# Track 2 ROW subscript qualification — 2026-10-18

## Boundary

Kaveon now supports Trino's one-based row subscript form (`row_expr[index]`)
through the existing `ELEMENT_AT` expression path. The index is required to be
constant for a batch, as a ROW expression has one fixed output type. Indexes
start at one; zero, negative, and out-of-range indexes fail closed. A NULL
parent ROW propagates NULL to the selected field, and an all-NULL index returns
an all-NULL result.

## Evidence

- `engine/crates/sql/src/logical_plan.rs`: `row_subscript_lowers_to_element_at`
  verifies parser/planner lowering.
- `engine/crates/exec/src/expr_eval.rs`:
  `row_subscript_selects_one_based_field_and_masks_null_parent_rows` verifies
  one-based selection and nullable parent semantics.
- `engine/crates/exec/src/expr_eval.rs`:
  `row_subscript_rejects_varying_and_out_of_range_indices` verifies fail-closed
  dynamic and invalid indexes.
- `engine/crates/server/src/differential_tests.rs`:
  `distributed_row_subscript_matches_local_and_rejects_invalid_constant_index`
  compares the typed ROW result and NULL-parent propagation between the local
  planner and the in-process three-worker fragment path, then checks that an
  invalid constant index fails on the distributed path and releases its query
  memory reservation.
- `engine/crates/exec/src/project.rs`: projection schema inference preserves
  the selected ROW field type while planning an empty-batch schema probe.
- Focused validation: `cargo test -p kaveon-exec row_subscript --quiet` (2
  passed), `cargo test -p kaveon-sql row_subscript --quiet` (1 passed), and
  `cargo test -p kaveon-server distributed_row_subscript_matches_local_and_rejects_invalid_constant_index -- --nocapture`
  (1 passed).

## Remaining scope

This does not complete general ROW compatibility. Named dereference,
deeply-nested complex values, ordered comparisons, arbitrary dynamic row
indexes, and external-client/cloud qualification remain open in the Maps and
rows tracker row.
