# Track 2 qualification: `ARRAY_POSITION` over `UInt8` lists

Date: 2026-10-18

## Contract

`ARRAY_POSITION` accepts a regular Arrow list of `UInt8` values and returns the
first matching position using Trino's one-based indexing. The implementation
compares in the native eight-bit width, preserving the maximum value (`255`)
and NULL element behavior.

## Focused evidence

Run from `engine/`:

```powershell
cargo test -p kaveon-exec evaluates_array_position_for_uint8_lists --quiet
```

The evaluator regression searches `[7, 255]` for `255`, and `[7, NULL]` for
`7`, returning positions `2` and `1` respectively.

The distributed differential corpus also includes the
`uint8_array_position` case across its storage layouts and worker fragment
path. The case is bounded to primitive regular lists; broader complex element
and external-client qualification remains tracked in the parity matrix.
