# Track 2 qualification: `FULL JOIN ... USING` with NULL keys

Date: 2026-10-18

## Contract

`FULL JOIN ... USING (key)` does not match SQL NULL keys. The merged key and
left/right payload columns must retain the correct unmatched rows on both
sides.

## Focused evidence

The regression `distributed_full_join_using_null_keys_matches_local` compares
the exact local and three-worker distributed results for matched, unmatched,
and NULL-key rows, and checks query-memory release.

Run from `engine/`:

```powershell
cargo test -p kaveon-server distributed_full_join_using_null_keys_matches_local --quiet
```
