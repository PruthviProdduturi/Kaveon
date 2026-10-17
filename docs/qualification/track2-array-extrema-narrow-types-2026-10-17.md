# Track 2 qualification: typed numeric `ARRAY_MIN` and `ARRAY_MAX`

## Scope

Regular Arrow lists passed to `ARRAY_MIN` and `ARRAY_MAX` now cover narrow
signed and unsigned integer types plus `Float32`, preserving the element type
and returning NULL for a NULL or all-NULL input list.

## Evidence

- Test: `array_extrema_accept_narrow_integer_and_float32_lists` in
  `engine/crates/exec/src/expr_eval.rs`.
- Inputs: `INTEGER [9, NULL, 1]` and `REAL [2.5, -1.0]`.
- Expected results: `(MIN, MAX) = (1, 9)` and `(-1.0, 2.5)`.
- Result: all three `ARRAY_MIN/MAX` evaluator tests passed; `kaveon-exec`
  Clippy passed.

The `array_narrow_extrema` case is also in the server differential corpus. The
full local/distributed sweep passed with 0 failures in 165.87 s, including the
pinned reference comparison.

This is focused evaluator evidence; broader differential and deployment gates
remain required for full Trino compatibility claims.
