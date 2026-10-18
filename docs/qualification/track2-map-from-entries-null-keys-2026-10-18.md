# Track 2 qualification: NULL keys in `MAP_FROM_ENTRIES`

Date: 2026-10-18

## Contract

`MAP_FROM_ENTRIES` rejects NULL map keys during distributed execution, matching
Trino's requirement that map keys are non-null. The failed query releases its
memory reservation before returning the error.

## Focused evidence

The three-worker regression `distributed_map_from_entries_rejects_null_keys`
uses a typed NULL VARCHAR key and asserts the fail-closed error plus zero
retained query memory.

Run from `engine/`:

```powershell
cargo test -p kaveon-server distributed_map_from_entries_rejects_null_keys --quiet
```
