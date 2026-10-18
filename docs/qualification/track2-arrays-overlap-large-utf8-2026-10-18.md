# Track 2 qualification: `ARRAYS_OVERLAP` over large UTF-8 lists

Date: 2026-10-18

## Contract

`ARRAYS_OVERLAP` compares `LargeList<LargeUtf8>` values directly with their
64-bit string offsets. It preserves exact string equality and Trino's
three-valued NULL behavior for NULL outer arrays and NULL elements.

## Focused evidence

Run from `engine/`:

```powershell
cargo test -p kaveon-exec arrays_overlap_accepts_large_utf8_lists --quiet
```

The regression compares `["alpha", NULL]` with `["omega", "alpha"]` and
returns `TRUE`. Broader external-client and cloud qualification remains open
in the parity matrix.
