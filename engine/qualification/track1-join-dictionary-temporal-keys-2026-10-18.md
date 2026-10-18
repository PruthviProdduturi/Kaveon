# Dictionary temporal join keys

## Scope

`row_key` now unwraps `Dictionary<Int32, T>` temporal values for the
supported Arrow representations: `Date32`, `Date64`, `Time32` seconds and
milliseconds, `Time64` microseconds and nanoseconds, and all timestamp units
with optional timezone metadata. The values use the same exact `Int32` or
`Int64` key representation as the corresponding plain column path. Null
dictionary values remain non-matching.

## Evidence

- `kaveon-exec` test
  `dictionary_temporal_join_keys_preserve_units_extrema_and_nulls` covers
  Date32, Date64, Time32 milliseconds, Time64 nanoseconds, and timezone-bearing
  Timestamp seconds with extrema and NULL values.
- `kaveon-server` test `distributed_temporal_join_keys_match_local` compares
  Date32, Date64, Time32, Time64, and timezone-bearing Timestamp layouts across
  local execution and the in-process three-worker fragment path.

As with dictionary floating keys, the current Arrow Parquet writer rejects
primitive dictionary arrays. The distributed check therefore uses equivalent
plain temporal Parquet layouts; dictionary unboxing is directly covered by
the execution regression. Native dictionary-encoded temporal Parquet remains
a storage qualification boundary.
