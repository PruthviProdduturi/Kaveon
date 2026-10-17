# Track 2 qualification: `ARRAYS_OVERLAP` with `LargeList<Decimal128>`

## Scope

The large-offset list path now accepts Decimal128 elements for
`ARRAYS_OVERLAP`, matching the existing regular-list decimal behavior and
preserving exact decimal equality.

## Evidence

- Test: `arrays_overlap_accepts_large_decimal_lists` in
  `engine/crates/exec/src/expr_eval.rs`.
- Inputs: `LargeList<DECIMAL(10,2)>` containing `1.25` on both sides.
- Expected result: `TRUE`.
- Result: all seven overlap evaluator tests passed; clippy passed for all
  execution targets.

Distributed regression: `arrays_overlap_decimal` in
`engine/crates/server/src/differential_tests.rs` passed in the full
Parquet-encoding and local/distributed execution-path sweep (1 test, 0
failures, 170.45 seconds).
