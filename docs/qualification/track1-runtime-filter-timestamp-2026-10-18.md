# Runtime-filter timestamp qualification

This is a local implementation qualification for the Track 1 runtime dynamic
filtering edge added on 2026-10-18. It does not claim cloud pruning or release
image qualification.

## Covered behavior

- Parquet timestamp columns with any Arrow timestamp unit are now valid targets
  for an exact build-side `IN` runtime filter. The filter literal is carried as
  the timestamp's integer storage value, and the row-filter compiler casts it
  to the column's timestamp type before comparison.
- Local Parquet decoding accepts a timestamp filter before the first batch and
  returns exactly the matching row.
- The partitioned hash-join runtime-filter path exercises timestamp keys and
  removes non-matching and NULL probe keys while preserving the exact inner
  join result.

## Reproduction and evidence

From `engine/`:

```text
cargo test -p kaveon-storage runtime_filter --quiet
cargo test -p kaveon-exec runtime_join_filter_uses_typed_unsigned_and_date_indexes --quiet
```

The first command passed 10 tests, including the timestamp Parquet reader
regressions. The second passed 1 test; its typed-index matrix includes the
timestamp case alongside unsigned integer and Date32 keys.

The remaining Track 1 gate is unchanged: qualify measurable file or row-group
pruning on a representative cloud dataset, then repeat the edge on the
deployed image and cover remaining runtime-filter type/exchange boundaries.
