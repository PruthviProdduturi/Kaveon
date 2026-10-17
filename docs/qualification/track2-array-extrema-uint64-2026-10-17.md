# Track 2 qualification: `ARRAY_MIN` and `ARRAY_MAX` with `UInt64` lists

Date: 2026-10-17

## Contract

`ARRAY_MIN` and `ARRAY_MAX` accept regular Arrow lists with `UInt64` elements,
ignore NULL elements when selecting an extremum, and return a typed NULL for an
empty or all-NULL list. Large-list extrema already use the generic comparator
path; this qualification covers the regular-list dispatch.

## Focused evidence

Run from `engine/`:

```powershell
cargo test -p kaveon-exec array_extrema_accept_uint64_lists --quiet
```

Observed result:

```text
running 1 test
.
test result: ok; 1 passed; 0 failed
```

The regression checks `ARRAY_MIN([9, NULL, 1]) = 1` and
`ARRAY_MAX([9, NULL, 1]) = 9` with `UInt64` output arrays.

This is bounded element-type coverage; full Trino array extrema and complex
element parity remains Partial in the tracker.
