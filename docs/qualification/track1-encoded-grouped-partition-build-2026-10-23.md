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
- The server's full differential sweep passed in isolation: 611 tests, 0
  failures, including `the_differential_sweep_matches_across_parquet_encodings_and_execution_paths` (580.09 s).

This is an allocation reduction with exact routing coverage. The canonical
binary route is now the default; set `KAVEON_DIRECT_GROUPED_EXCHANGE=off` for
an explicit rollback/control. It is not yet a ClickHouse or Trino performance
claim; a rebuilt ClickBench run is required to measure wall time and exchange
bytes before scoring it.
