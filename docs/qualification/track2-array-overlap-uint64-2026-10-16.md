# Track 2 qualification: `ARRAYS_OVERLAP` with `UInt64` lists

Date: 2026-10-16  
Commit: `af502ea2` (working tree change: `ARRAYS_OVERLAP` `UInt64` dispatch and regression)

## Contract

`ARRAYS_OVERLAP` must accept regular and large Arrow lists whose element type is
`UInt64`. It returns `TRUE` for a shared non-null value, `FALSE` for disjoint
non-null lists, and `NULL` when no shared value exists but either list contains a
NULL element. A NULL list remains NULL.

## Focused evidence

Run from `engine/`:

```powershell
cargo test -p kaveon-exec arrays_overlap_accepts_uint64_lists_with_null_semantics --quiet
```

Observed result:

```text
running 1 test
.
test result: ok. 1 passed; 0 failed
```

The regression covers a shared `u64::MAX` value in regular lists, a disjoint
list with a NULL element, and a shared `u64::MAX` value in large lists.

## Differential regression

```powershell
cargo test -p kaveon-server differential_tests --quiet
```

Observed result: `41 passed; 0 failed` in 163.89 seconds.

This is a focused type-coverage qualification. Full Trino array-function and
complex-element parity remains Partial in the tracker.
