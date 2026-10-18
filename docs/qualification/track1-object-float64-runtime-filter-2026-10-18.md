# Track 1 — Object-store Float64 runtime-filter qualification

## Scope

The generic object-store Parquet reader, used by ADLS-backed reads, must accept a late Float64 equality filter before decoding and preserve the exact `DOUBLE` value. This closes the object-reader half of the local Float64 type edge without claiming a live cloud run.

## Evidence

The regression writes four Float64 values to an in-memory object store, enables late materialisation, applies `2.25` before the first batch, and observes exactly one decoded row:

```text
cargo test -p kaveon-storage --lib object_reader::tests::object_runtime_filter_preserves_float64_values_before_decode --quiet
cargo fmt --all -- --check
cargo clippy -p kaveon-storage --all-targets -- -D warnings
```

The same object-reader path is used for ADLS object reads; credentials and network access are not exercised by this test.

## Boundary

Live ADLS qualification with representative row groups, pruning counters, and the deployed image remains open.
