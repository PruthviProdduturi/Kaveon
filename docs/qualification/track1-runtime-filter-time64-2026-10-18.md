# Time64 runtime-filter qualification — 2026-10-18

## Scope

This qualification covers a bounded local Parquet edge in Track 1: build-side
runtime-filter values represented at the engine boundary as `Int64` are accepted
for Arrow `Time64` probe columns and reach both the row-level predicate and the
Parquet reader before batch decoding.

It does not claim cloud pruning or Decimal256/scale-conversion support.

## Evidence

- `kaveon-storage` scan-predicate tests compile an equality predicate against a
  `Time64(Microsecond)` column and preserve exact microsecond values.
- `kaveon-storage` Parquet-reader tests write a four-row `Time64(Microsecond)`
  fixture in two row groups, apply one runtime-filter value, and return exactly
  the matching row after the reader path.
- The accepted scalar representation is `ScalarValue::Int64`, matching the
  existing join runtime-filter conversion for `Time32`/`Time64` keys.

Commands:

```text
cargo test -p kaveon-storage scan_predicate --quiet
cargo test -p kaveon-storage parquet_reader --quiet
```

Both focused groups passed on the local workspace on 2026-10-18.

## Boundary

`Time32`/`Time64` runtime filters now share the existing exact integer-to-column
cast path. Remaining qualification includes deployed/cloud statistics pruning,
other exchange/type edges, Decimal256, and scale conversion.