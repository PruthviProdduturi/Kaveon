# Dictionary floating-point join keys

## Scope

`row_key` now unwraps `Dictionary<Int32, Float32>` and
`Dictionary<Int32, Float64>` values before constructing the exact floating
join key. The normalization matches the plain path: `-0.0` and `+0.0` share
one key, NaN values use the canonical NaN bits, and null dictionary values do
not join.

## Evidence

- `kaveon-exec` test `dictionary_float_join_keys_preserve_zero_nan_and_null_semantics`
  covers Float32 and Float64 dictionary values, finite values, signed zero,
  NaN, and nulls.
- `kaveon-server` test `distributed_float_join_keys_match_local` compares the
  equivalent Float32 and Float64 plain Parquet layouts through the local and
  three-worker fragment paths.

The Arrow Parquet writer currently rejects primitive dictionary arrays (it
panics while trying to write `Dictionary<Int32, Float32/64>` as a primitive
leaf). Therefore the distributed Parquet check uses the equivalent plain
floating layout; dictionary unboxing itself is covered by the execution-level
regression above. A native dictionary-encoded Parquet fixture remains a
separate storage qualification item.
