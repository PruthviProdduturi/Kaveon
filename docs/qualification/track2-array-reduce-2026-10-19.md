# Track 2 qualification: `REDUCE(array, initial, input_lambda, output_lambda)`

Kaveon supports the bounded scalar form of Trino's `REDUCE` higher-order array
function. The input lambda receives `(state, element)` for each array element,
and the output lambda receives the final state. Outer-row columns remain
available to both lambdas, while lambda parameters shadow captured columns.

The implementation preserves typed NULL elements, returns NULL for a NULL input
array, and requires one state/result value per evaluated row. It intentionally
does not claim arbitrary lambda arities, complex state serialization, or a
distributed stateful reduction protocol.

## Evidence

- SQL lowering test: `lowers_reduce_input_and_output_lambdas` in
  `engine/crates/sql/src/logical_plan.rs`.
- Local evaluator test: `reduce_applies_input_and_output_lambdas` in
  `engine/crates/exec/src/expr_eval.rs`.
- Distributed differential corpus case: `array_reduce_lambda` in
  `engine/crates/server/src/differential_tests.rs`, compared across the local
  and three-worker execution paths and the pinned Trino reference.

## Qualified boundary

This qualifies scalar numeric state with a two-parameter input lambda and a
one-parameter output lambda. Broader Trino lambda shapes, typed ROW/ARRAY/MAP
state, state spilling, and external cloud qualification remain open.
