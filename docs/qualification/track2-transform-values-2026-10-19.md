# Track 2 qualification: `TRANSFORM_VALUES(map, lambda)`

Kaveon supports the bounded scalar Trino form
`TRANSFORM_VALUES(map, (key, value) -> new_value)`. Keys and map row
boundaries are preserved while each value expression is evaluated with typed
key/value columns; captured outer-row columns remain available and lambda
parameters shadow same-named outer columns. NULL map rows remain NULL.

## Evidence

- SQL lowering test: `lowers_transform_values_lambda` in
  `engine/crates/sql/src/logical_plan.rs`.
- Local evaluator test: `transform_values_rewrites_values_and_preserves_keys`
  in `engine/crates/exec/src/expr_eval.rs`.
- Distributed differential corpus case: `transform_values_lambda` in
  `engine/crates/server/src/differential_tests.rs`, compared across the local
  and three-worker execution paths and the pinned Trino reference.

## Qualified boundary

This qualifies two-parameter scalar value transformations over maps. Broader
map higher-order functions, complex lambda state, and external cloud
qualification remain open.
