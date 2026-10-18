# Dictionary decimal join keys

## Scope

`row_key` now unwraps `Dictionary<Int32, Decimal128>` and
`Dictionary<Int32, Decimal256>` values without narrowing. The existing exact
scaled key representations are reused: Decimal128 values use their `i128`
unscaled value and scale, while Decimal256 values use Arrow `i256` and scale.
Null dictionary values remain non-matching.

## Evidence

- `kaveon-exec` test
  `dictionary_decimal_join_keys_preserve_scale_extrema_and_nulls` covers
  Decimal128 precision 38/scale 2 and Decimal256 precision 76/scale 3,
  extrema, and NULL values.
- `kaveon-server` test `distributed_decimal_join_keys_match_local` compares
  equivalent Decimal128 and Decimal256 Parquet layouts through local execution
  and the in-process three-worker fragment path.

The current Arrow Parquet writer rejects primitive dictionary arrays, so the
distributed check uses equivalent plain decimal Parquet layouts. Dictionary
unboxing is directly covered by the execution regression; native
dictionary-encoded decimal Parquet remains a storage qualification boundary.
