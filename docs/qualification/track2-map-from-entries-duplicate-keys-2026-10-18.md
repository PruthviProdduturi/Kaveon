# Track 2 qualification: duplicate keys in `MAP_FROM_ENTRIES`

Date: 2026-10-18

## Contract

`MAP_FROM_ENTRIES` fails closed when the input contains duplicate keys, matching
Trino's map-construction contract. The duplicate is rejected during distributed
execution and does not leave query-memory reservations behind.

## Focused evidence

The distributed three-worker regression
`distributed_map_from_entries_rejects_duplicate_keys` evaluates two entries
with key `a` and asserts the duplicate-key failure plus zero retained query
memory after the error.

Run from `engine/`:

```powershell
cargo test -p kaveon-server distributed_map_from_entries_rejects_duplicate_keys --quiet
```
