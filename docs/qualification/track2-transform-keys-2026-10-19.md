# Track 2 qualification: `TRANSFORM_KEYS(map, lambda)`

Kaveon supports the bounded scalar Trino form
`TRANSFORM_KEYS(map, (key, value) -> new_key)`. Map values, row boundaries,
and NULL map rows are preserved while each key expression is evaluated with
typed key/value columns; captured outer-row columns remain available and
lambda parameters shadow same-named outer columns. Transformed keys must be
non-NULL and unique within each map row.

## Evidence

- SQL lowering test: `lowers_transform_keys_lambda` in
  `engine/crates/sql/src/logical_plan.rs`.
- Local evaluator test: `transform_keys_rewrites_keys_and_preserves_values`
  in `engine/crates/exec/src/expr_eval.rs`.
- Distributed differential corpus case: `transform_keys_lambda` in
  `engine/crates/server/src/differential_tests.rs`, compared across the local
  and three-worker execution paths and the pinned Trino reference.

## Qualified boundary

This qualifies two-parameter scalar key transformations over maps, including
duplicate and NULL-key rejection. Broader map higher-order functions, complex
lambda state, and external cloud qualification remain open.
