# Track 2 qualification: `ARRAY_SORT` over nested ROW elements

## Scope

`ARRAY_SORT` now accepts orderable nested list and ROW elements. It uses the
same recursive lexicographic comparison as scalar expression ordering and
places NULL elements last. MAP elements remain rejected because Trino does not
define an ordered MAP type.

## Evidence

- Commit: the follow-up commit recorded with this qualification.
- Test: `array_sort_orders_nested_rows_and_places_nulls_last` in
  `engine/crates/exec/src/expr_eval.rs`.
- Input: `[ROW(2), NULL, ROW(1)]`.
- Expected result: `[ROW(1), ROW(2), NULL]`.
- Result: focused test passed with no failures.

This is focused evaluator evidence; it does not claim full nested-function or
distributed window/order qualification.
