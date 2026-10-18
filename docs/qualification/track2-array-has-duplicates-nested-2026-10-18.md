# Track 2 qualification: nested `ARRAY_HAS_DUPLICATES`

Date: 2026-10-18

## Contract

`ARRAY_HAS_DUPLICATES` compares nested array elements structurally. Equal inner
arrays are duplicates; reordered inner arrays remain distinct. The result is a
boolean and the distributed path must match the local evaluator.

## Focused evidence

`distributed_array_has_duplicates_nested_matches_local` compares duplicate and
reordered nested arrays through the local planner and the three-worker fragment
path, and verifies the exact boolean result and released query memory.

Run from `engine/`:

```powershell
cargo test -p kaveon-server distributed_array_has_duplicates_nested_matches_local -- --nocapture
```
