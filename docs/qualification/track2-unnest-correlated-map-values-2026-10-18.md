# Track 2 qualification: correlated `UNNEST(MAP_VALUES(...))`

Date: 2026-10-18

## Contract

In a lateral join, `MAP_VALUES` is evaluated for each left input row. Its
values expand into rows with one-based `WITH ORDINALITY`, retaining each
left-row correlation.

## Focused evidence

The distributed regression
`distributed_correlated_map_values_unnest_with_ordinality` evaluates two
left rows across three workers and checks exact value and ordinality results,
plus query-memory release.

Run from `engine/`:

```powershell
cargo test -p kaveon-server distributed_correlated_map_values_unnest_with_ordinality --quiet
```
