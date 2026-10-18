# Track 1 dictionary numeric and boolean join keys — 2026-10-18

## Boundary

Dictionary encoded join keys previously supported only `Dictionary<Int32,
Utf8/LargeUtf8>`. Numeric and boolean dictionary columns failed in hash-key
construction even though their logical values are orderable join keys.

## Change

`row_key` now unwraps `Dictionary<Int32, Int32>`,
`Dictionary<Int32, UInt32>`, and `Dictionary<Int32, Boolean>` values before
hashing. Dictionary value NULLs remain non-matching. Numeric extrema are kept
lossless through the existing `Int32`/`UInt64` key representations.

## Evidence

The executor test covers all three value types, NULL values, signed extrema,
unsigned extrema, and boolean values. The distributed differential writes a
dictionary-encoded Int32 Parquet pair, compares local and three-worker results,
and verifies both extrema matches while excluding NULL:

```text
cargo test -p kaveon-exec dictionary_numeric_and_boolean_join_keys_unbox_losslessly -- --nocapture
cargo test -p kaveon-server distributed_dictionary_numeric_join_key_matches_local -- --nocapture
```

Both passed locally. Dictionary temporal/decimal and broader external/cloud
qualification remain open.
