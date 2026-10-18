# Dictionary integer join keys

## Scope

`row_key` now unwraps dictionary integer values for the remaining supported
widths: Int8, Int16, Int64, UInt8, UInt16, and UInt64. Narrow signed values
reuse the lossless Int32 key representation; narrow unsigned values reuse the
UInt64 representation. Int64 and UInt64 retain their full-width values. Null
dictionary values remain non-matching.

## Evidence

- `kaveon-exec` test `dictionary_integer_join_keys_widen_losslessly` covers
  signed and unsigned extrema, reordered dictionary keys, and NULL values for
  all six widths.
- `kaveon-server` test `distributed_dictionary_integer_join_keys_match_local`
  compares equivalent six-width Parquet layouts through local execution and
  the in-process three-worker fragment path.

The current Arrow Parquet writer rejects primitive dictionary arrays, so the
distributed check uses equivalent plain integer Parquet layouts. Dictionary
unboxing is directly covered by the execution regression; native
dictionary-encoded integer Parquet remains a storage qualification boundary.
