# Track 2 qualification: `ARRAY_FIRST` and `ARRAY_LAST` empty/null behavior

Date: 2026-10-18

## Contract

`ARRAY_FIRST` and `ARRAY_LAST` return the typed endpoint of a list. They return
NULL for an empty list and for a list whose selected endpoint is NULL, while
preserving the element type in the result schema.

## Focused evidence

`distributed_array_first_last_match_local_for_empty_and_null_rows` compares
regular, empty, and NULL-valued arrays through the local planner and a
three-worker fragment execution. The distributed result must match the local
result and release all query memory.

Run from `engine/`:

```powershell
cargo test -p kaveon-server distributed_array_first_last_match_local_for_empty_and_null_rows -- --nocapture
```
