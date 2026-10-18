# Track 2 SQL compatibility qualification — nested `ARRAY_REPEAT`

**Scope:** bounded `ARRAY_REPEAT` support for `ARRAY<bigint>` values in the
local expression evaluator.

The evaluator now repeats each non-null inner list as an independent nested
array, preserves null inner lists as null elements, preserves a null repeat
count as a null outer result, and returns an empty array for a zero count. The
implementation is deliberately limited to regular Arrow `LIST<INT64>` inputs;
other nested element types remain fail-closed rather than being coerced.

## Validation

From `engine/`:

```powershell
cargo fmt --all -- --check
cargo test -p kaveon-exec --lib array_repeat --quiet
```

The focused test suite passed 3 tests, including primitive values, large UTF-8
values, and nested integer arrays with null and zero-count rows. The new test
checks repeated inner values, null inner-list preservation, and empty output.

This is a local semantic qualification only. Distributed and external-client
wire qualification for nested `ARRAY_REPEAT` remains open.
