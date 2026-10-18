# Track 2 qualification: `ARRAY_POSITION` over large `UInt16` lists

Date: 2026-10-18

## Contract

`ARRAY_POSITION` uses a native-width primitive comparison for large Arrow
`List<UInt16>` values. It returns the first one-based position and preserves
NULL list elements and the `UInt16` maximum value without widening the values.

## Focused evidence

Run from `engine/`:

```powershell
cargo test -p kaveon-exec array_position_accepts_large_uint16_lists --quiet
```

The regression searches `[7, NULL, 65535]` for `65535` and returns position
`3`. The regular-list UInt16 differential case remains covered separately;
this page records the large-list native dispatch edge. Broader external-client
and cloud qualification remains open in the parity matrix.
