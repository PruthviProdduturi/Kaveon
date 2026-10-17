# Track 2 qualification: typed numeric `ARRAYS_OVERLAP`

## Scope

`ARRAYS_OVERLAP` now accepts regular and large lists with narrow signed and
unsigned integer elements and `Float32`, while preserving Trino's three-valued
NULL result when a NULL element is present but no definite match exists.

## Evidence

- Test: `arrays_overlap_accepts_narrow_integer_and_float32_lists` in
  `engine/crates/exec/src/expr_eval.rs`.
- Inputs: `INTEGER [1, 3]` vs `[2, 3]`, and `REAL [1.5]` vs `[2.5]`.
- Expected results: `TRUE` and `FALSE`.
- Result: all six overlap evaluator tests passed.
- Distributed regression: `arrays_overlap_narrow_numeric` in
  `engine/crates/server/src/differential_tests.rs` passed in the full
  Parquet-encoding and local/distributed execution-path sweep (1 test, 0
  failures, 166.44 seconds).

The broader deployment gates and full complex-type function matrix remain
required for full Trino parity claims.
