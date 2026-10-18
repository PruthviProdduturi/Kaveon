# Track 2 qualification: `UNNEST(MAP_VALUES(...))`

Date: 2026-10-18

## Contract

`MAP_VALUES(map)` returns the map values as an array suitable for row
expansion by `UNNEST`, preserving the values represented by the map entries.

## Focused evidence

The distributed regression `distributed_map_values_unnest_matches_trino_rows`
expands a two-entry map across three workers and checks the exact rows `10`
and `20`, plus query-memory release.

Run from `engine/`:

```powershell
cargo test -p kaveon-server distributed_map_values_unnest_matches_trino_rows --quiet
```
