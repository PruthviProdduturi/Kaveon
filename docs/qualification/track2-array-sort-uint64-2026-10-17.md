# Track 2 qualification: `ARRAY_SORT` with `UInt64` lists

Date: 2026-10-17

## Contract

`ARRAY_SORT` accepts regular and large Arrow lists with `UInt64` elements,
orders values ascending, and places NULL elements last. A NULL outer list
remains NULL.

## Focused evidence

Run from `engine/`:

```powershell
cargo test -p kaveon-exec array_sort_accepts_uint64_lists_and_keeps_nulls_last --quiet
```

Observed result:

```text
running 1 test
.
test result: ok; 1 passed; 0 failed
```

The regression covers a regular list containing `1`, `u64::MAX`, and NULL,
plus a large list containing `1` and `u64::MAX`.

This is a bounded element-type qualification; full Trino array-sort comparator
and complex-element parity remains Partial in the tracker.
