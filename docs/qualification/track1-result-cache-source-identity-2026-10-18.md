# Track 1 result-cache source identity qualification — 2026-10-18

## Boundary

The result cache must not serve rows produced from an older source when a
Parquet file or directory listing changes. Delta version keys already covered
Delta tables, but a cache key containing only `delta_versions` left ordinary
Parquet sources dependent on the catalog snapshot and TTL.

## Change

Planning now records the `analyze_source` identity for every resolved source in
`SourcePins::source_versions`. The result-cache key includes that map in
addition to the existing Delta version map. The identity covers Delta, Iceberg,
directory Parquet, and single-file Parquet source changes without exposing
source contents in telemetry or query records.

## Evidence

The focused server result-cache suite passes:

```text
cargo test -p kaveon-server result_cache::tests -- --nocapture
13 passed, 0 failed
```

`the_key_carries_everything_that_decides_a_result` now asserts that a changed
source identity produces a cache miss key, independently of the Delta version
field. `cargo check -p kaveon-server --all-targets` and the workspace Rust
format check also pass for this change.

This is local source-identity and cache-key evidence. Deployed hit/miss latency
and live object-store replacement qualification remain separate gates.
