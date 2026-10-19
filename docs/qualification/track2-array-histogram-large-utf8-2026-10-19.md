# Track 2 qualification: `ARRAY_HISTOGRAM` over `LargeUtf8`

`ARRAY_HISTOGRAM` now preserves the input string width for map keys. A
`LargeUtf8` array produces a `MAP(LargeUtf8, BIGINT)` result instead of
narrowing its keys to `Utf8`; counts remain deterministic and NULL elements
are ignored.

## Evidence

- Local evaluator test: `array_histogram_preserves_large_utf8_key_type` in
  `engine/crates/exec/src/expr_eval.rs`.
- The test verifies the exact sorted keys `["alpha", "beta"]`, counts
  `[1, 2]`, and the `LargeUtf8` key type.

The current differential fixture does not contain a `LargeUtf8` Parquet source,
so this change is locally qualified; a wide-string distributed source remains
an explicit follow-up qualification boundary.
