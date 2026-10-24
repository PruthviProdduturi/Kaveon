# Direct encoded grouped partition build — 2026-10-23

The encoded grouped-state partitioner now builds each worker's canonical
`Binary` columns directly. The previous path allocated a row-index builder per
worker and then invoked Arrow `take` for every column. The new path preserves
the same hash routing and NULL behavior while removing those intermediate
indices and generic `take` dispatch from the high-cardinality grouped-state
route. Non-binary layouts retain the existing fallback.

Validation:

- `cargo fmt --all -- --check` passed.
- `cargo clippy -p kaveon-exec --all-targets -- -D warnings` passed.
- `cargo test -p kaveon-exec encoded_source_partitioner_routes_each_state_key_once --quiet` passed with binary key/state columns and NULL payload rows.
- `cargo test -p kaveon-exec local_parallel --quiet` passed: 14 tests.

This is an allocation reduction with exact routing coverage. It is not yet a
ClickHouse or Trino performance claim; a rebuilt ClickBench run is required to
measure wall time and exchange bytes before enabling or scoring it.
