# Track 2 qualification: `ARRAY_POSITION` over large `UInt8` lists

Date: 2026-10-18

## Contract

`ARRAY_POSITION` uses a native-width primitive comparison for large Arrow
`List<UInt8>` values. It returns the first one-based position and preserves
NULL list elements and the `UInt8` maximum value without converting the list to
JSON or a wider integer type.

## Focused evidence

Run from `engine/`:

```powershell
cargo test -p kaveon-exec array_position_accepts_large_uint8_lists --quiet
```

The regression searches `[7, NULL, 255]` for `255` and returns position `3`.
The existing distributed differential corpus covers the corresponding regular
primitive array path; this qualification closes the large-list native
dispatch edge. Broader external-client and cloud qualification remains open
in the parity matrix.
