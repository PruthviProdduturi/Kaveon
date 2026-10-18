# Track 2 qualification: missing map-key lookup

Date: 2026-10-18

## Contract

`MAP_CONTAINS_KEY` returns `false` and `MAP_ELEMENT_AT` returns `NULL` when
the requested key is absent. Missing keys do not raise an execution error.

## Focused evidence

The distributed regression `distributed_map_lookup_missing_key_matches_trino_rows`
checks the exact `false, NULL` result across three workers and verifies query
memory is released.

Run from `engine/`:

```powershell
cargo test -p kaveon-server distributed_map_lookup_missing_key_matches_trino_rows --quiet
```
