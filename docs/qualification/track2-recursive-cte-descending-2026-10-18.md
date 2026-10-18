# Track 2 qualification: bounded descending recursive CTE

Date: 2026-10-18

## Contract

A bounded single-column integer recursive CTE using `UNION ALL`, subtraction,
and a strict lower bound produces the descending sequence through the existing
recursive lowering path.

## Focused evidence

The distributed regression
`distributed_recursive_cte_descending_union_all_matches_trino_rows` evaluates
`5` down to `1` across three workers and checks exact ordered rows, plus
query-memory release.

Run from `engine/`:

```powershell
cargo test -p kaveon-server distributed_recursive_cte_descending_union_all_matches_trino_rows --quiet
```
