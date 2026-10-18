# Track 2 qualification: `ARRAY_FIRST` and `ARRAY_LAST` empty/null behavior

Date: 2026-10-18

## Contract

`ARRAY_FIRST` and `ARRAY_LAST` return the typed endpoint of a list. They return
NULL for an empty list, a NULL input list, or a list whose selected endpoint is
NULL, while preserving the element type in the result schema. `ARRAY[NULL]`
and mixed typed/NULL literals are accepted and preserve a `NULL` element type
for all-NULL arrays.

## Focused evidence

`distributed_array_first_last_match_local_for_empty_and_null_literals` compares regular,
empty, all-NULL, and mixed typed/NULL arrays through the local planner and a
three-worker fragment execution. The distributed result must match the local
result and release all query memory. The local evaluator regression
`array_first_and_last_return_typed_values_or_null_for_empty_rows` covers NULL
selected endpoints.

Run from `engine/`:

```powershell
cargo test -p kaveon-server distributed_array_first_last_match_local_for_empty_and_null_literals -- --nocapture
```
