# Track 2 qualification: `JSON_ARRAY_LENGTH` non-array results

Date: 2026-10-18

## Contract

`JSON_ARRAY_LENGTH(document)` returns the number of elements for a JSON array
and `NULL` for a valid object or JSON null. This keeps non-array values from
being treated as one-element arrays.

## Focused evidence

The distributed regression
`distributed_json_array_length_non_arrays_are_null` executes the array,
object, and JSON-null cases across three workers and checks the exact result
`3, NULL, NULL` plus query-memory release.

Run from `engine/`:

```powershell
cargo test -p kaveon-server distributed_json_array_length_non_arrays_are_null --quiet
```
