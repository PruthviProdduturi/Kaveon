# Track 2 SQL compatibility qualification — nested `ARRAY_REPEAT`

**Scope:** bounded `ARRAY_REPEAT` support for `ARRAY<bigint>` values in the
local evaluator and distributed fragment path, including regular and large
Arrow list offsets.

The evaluator now repeats each non-null inner list as an independent nested
array, preserves null inner lists as null elements, preserves a null repeat
count as a null outer result, and returns an empty array for a zero count. The
implementation supports regular `LIST<INT64>` and large `LARGE_LIST<INT64>`
inputs; other nested element types remain fail-closed rather than being
coerced.

## Validation

From `engine/`:

```powershell
cargo fmt --all -- --check
cargo test -p kaveon-exec --lib array_repeat --quiet
cargo test -p kaveon-server --bin kaveon-server distributed_nested_array_repeat_preserves_inner_arrays -- --nocapture
```

The focused evaluator suite passed 4 tests, including primitive values, large
UTF-8 values, and nested regular and large integer arrays with null and
zero-count rows. The
server differential test passed through the three-worker fragment path and
verified that `UNNEST(ARRAY_REPEAT(ARRAY[1, 2], 2))` returns two independent
`ARRAY<bigint>` rows (`[1, 2]`, `[1, 2]`) without flattening or changing the
nested type.

This qualifies the bounded local and in-process distributed fragment paths for
both Arrow list offset widths. External-client wire qualification, arbitrary
nested element types, and larger cloud/AKS qualification remain open.
