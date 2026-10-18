# Track 2 qualification: `JSON_ARRAY_CONTAINS` non-array input

Date: 2026-10-18

## Contract

`JSON_ARRAY_CONTAINS(document, needle)` returns `false` when the valid JSON
document is an object rather than an array. An object must not be interpreted
as a one-element array during membership evaluation.

## Focused evidence

The distributed regression `distributed_json_array_contains_non_array_is_false`
evaluates an object document with a typed numeric needle across three workers
and checks the exact `false` result plus query-memory release.

Run from `engine/`:

```powershell
cargo test -p kaveon-server distributed_json_array_contains_non_array_is_false --quiet
```
