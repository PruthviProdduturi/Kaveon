# Track 2 qualification: bounded recursive CTE with `UNION`

Date: 2026-10-18

## Contract

A bounded single-column integer recursive CTE using deduplicating `UNION`
produces the same monotonic sequence as the equivalent Trino query.

## Focused evidence

The distributed regression `distributed_recursive_cte_union_matches_trino_rows`
executes `1` through `5` across three workers and checks exact ordered rows,
plus query-memory release.

Run from `engine/`:

```powershell
cargo test -p kaveon-server distributed_recursive_cte_union_matches_trino_rows --quiet
```
