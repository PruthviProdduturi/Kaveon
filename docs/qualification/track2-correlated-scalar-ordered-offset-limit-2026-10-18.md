# Track 2 qualification: correlated scalar ordered offset and limit

Date: 2026-10-18

## Contract

For a correlated scalar subquery, `ORDER BY ... OFFSET 1 LIMIT 1` is applied
per correlated key before the outer comparison. The distributed result must
match the local result exactly.

## Focused evidence

The regression `distributed_correlated_scalar_ordered_offset_limit_matches_local`
compares the local and three-worker plans for the bounded scalar shape and
asserts complete query-memory release.

Run from `engine/`:

```powershell
cargo test -p kaveon-server distributed_correlated_scalar_ordered_offset_limit_matches_local --quiet
```
