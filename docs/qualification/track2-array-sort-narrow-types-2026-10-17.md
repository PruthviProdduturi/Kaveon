# Track 2 qualification: typed numeric `ARRAY_SORT`

## Scope

Regular Arrow lists passed to `ARRAY_SORT` now cover narrow signed and
unsigned integer types plus `Float32`, with the same ascending order and
NULL-last behavior as the existing wide numeric paths. Large-list sorting
uses the same recursive comparator contract.

## Evidence

- Test: `array_sort_accepts_narrow_integer_and_float32_lists` in
  `engine/crates/exec/src/expr_eval.rs`.
- Inputs: `INTEGER [3, NULL, 1]` and `REAL [2.5, -1.0]`.
- Expected results: `[1, 3, NULL]` and `[-1.0, 2.5]`.
- Result: all four `ARRAY_SORT` evaluator tests passed; `kaveon-exec`
  Clippy passed.

This is focused evaluator evidence; the broader differential and AKS gates
remain the qualification boundary for deployment claims.
