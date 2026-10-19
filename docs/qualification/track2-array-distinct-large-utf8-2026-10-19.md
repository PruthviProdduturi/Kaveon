# Track 2 qualification: `ARRAY_DISTINCT` over `LargeUtf8`

`ARRAY_DISTINCT` now preserves first-occurrence order and emits only one NULL
for `LargeUtf8` list elements. It also retains the `LargeUtf8` child type when
the input uses large list offsets. The previous branch sorted values and
returned duplicates, which diverged from Trino's order-preserving set
semantics.

## Evidence

- Local evaluator test: `array_distinct_large_utf8_keeps_first_occurrence_and_one_null`
  in `engine/crates/exec/src/expr_eval.rs`.
- Local evaluator test: `array_distinct_large_list_large_utf8_preserves_child_type`
  verifies that a `LargeList<LargeUtf8>` input does not narrow its child type.
- The test verifies `LargeUtf8` output type and the exact result
  `["b", "a", NULL]` for `["b", "a", "b", NULL, "a", NULL]`.

The current differential fixture does not contain a `LargeUtf8` Parquet source,
so this change is locally qualified; a wide-string distributed source remains
an explicit follow-up qualification boundary.
