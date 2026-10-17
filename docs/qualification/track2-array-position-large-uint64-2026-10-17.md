# Track 2 qualification: `ARRAY_POSITION` over large `UInt64` lists

Date: 2026-10-17

## Contract

`ARRAY_POSITION` must accept a large Arrow list of primitive values, including
`UInt64`, and return the first one-based matching position. NULL list elements
remain searchable without changing the typed result.

## Focused evidence

Run from `engine/`:

```powershell
cargo test -p kaveon-exec array_position_accepts_large_uint64_lists --quiet
```

Observed result:

```text
running 1 test
.
test result: ok; 1 passed; 0 failed
```

The regression searches `[7, NULL, u64::MAX]` for `u64::MAX` and asserts
position `3`.

This closes large primitive-list dispatch only; broader Trino array-position
and complex-element parity remains Partial in the tracker.
