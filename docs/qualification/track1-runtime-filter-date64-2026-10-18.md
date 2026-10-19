# Date64 runtime-filter qualification

## Result

Single-key `Date64` build-side runtime filters now use a typed `i64` hash index
instead of the generic composite row-key path. The filter preserves NULL
exclusion, duplicate build values, exact millisecond values, and emits a
lossless storage `IN` predicate for Parquet/Delta readers.

## Evidence

`join::tests::date64_runtime_filter_uses_typed_index_and_storage_predicate`
checks a duplicate-bearing build side, a nonmatching probe value, matching
values, and a NULL probe. It verifies both the storage predicate values and
the batch-level mask.

## Boundary

This qualification covers local single-key Date64 filters. Composite Date64
keys, timestamp/timezone coercions, Iceberg/ADLS deployed scans, and full
cross-worker late-filter timing remain governed by the broader runtime
filter qualification row.