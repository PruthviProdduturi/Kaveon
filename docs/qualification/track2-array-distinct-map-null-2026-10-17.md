# Track 2 qualification: NULL-safe `ARRAY_DISTINCT` over map elements

## Scope

This qualification covers nested map elements in `ARRAY_DISTINCT`. Equal maps
are removed in first-occurrence order, and two maps with the same key and NULL
value are also equal under the recursive NULL-safe comparison used by nested
array values.

## Evidence

- Commit: `b4c183a5` plus the follow-up regression commit recorded with this
  qualification.
- Test: `array_distinct_deduplicates_nested_array_elements_in_order` in
  `engine/crates/exec/src/expr_eval.rs`.
- Input: `[MAP(ARRAY['a'], ARRAY[1]), MAP(ARRAY['a'], ARRAY[1]),
  MAP(ARRAY['a'], ARRAY[NULL]), MAP(ARRAY['a'], ARRAY[NULL])]`.
- Expected result: two maps, preserving the first ordinary map and the first
  NULL-valued map.
- Result: focused test passed with no failures.

This is focused evaluator evidence; it does not claim full Trino map-function
or distributed complex-type parity.
