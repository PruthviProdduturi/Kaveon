# Track 2 qualification: `ARRAY_FIRST` and `ARRAY_LAST` empty/null behavior

Date: 2026-10-18

## Contract

`ARRAY_FIRST` and `ARRAY_LAST` return the typed endpoint of a list. They return
NULL for an empty list and for a NULL input list, while preserving the element
type in the result schema. The local evaluator also covers a NULL selected
endpoint; the distributed SQL parser currently does not accept a NULL literal
inside an `ARRAY[...]` constructor.

## Focused evidence

`distributed_array_first_last_match_local_for_empty_arrays` compares regular
and empty arrays through the local planner and a three-worker fragment
execution. The distributed result must match the local result and release all
query memory. The local evaluator regression
`array_first_and_last_return_typed_values_or_null_for_empty_rows` covers NULL
selected endpoints.

Run from `engine/`:

```powershell
cargo test -p kaveon-server distributed_array_first_last_match_local_for_empty_arrays -- --nocapture
```
