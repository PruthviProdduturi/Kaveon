# Track 2 qualification: `MAP_FILTER(map, lambda)`

Kaveon supports the bounded scalar Trino form `MAP_FILTER(map, (key, value) ->
predicate)`. The predicate is evaluated once per map entry with typed key and
value columns; captured outer-row columns remain available and lambda
parameters shadow same-named outer columns. NULL predicates are treated as
false, while NULL map rows remain NULL with their offsets preserved.

## Evidence

- SQL lowering test: `lowers_map_filter_lambda` in
  `engine/crates/sql/src/logical_plan.rs`.
- Local evaluator test: `map_filter_applies_key_value_lambda` in
  `engine/crates/exec/src/expr_eval.rs`.
- Distributed differential corpus case: `map_filter_lambda` in
  `engine/crates/server/src/differential_tests.rs`, compared across the local
  and three-worker execution paths and the pinned Trino reference.

## Qualified boundary

This qualifies two-parameter boolean predicates over scalar map keys and
values. Broader map higher-order functions, complex lambda state, and external
cloud qualification remain open.
