# Qualification: view-backed text in array functions

Date: 2026-11-03  
Scope: vectorized `ARRAY_HISTOGRAM` and `ARRAYS_OVERLAP` over list elements
(`kaveon-exec`)

## Contract

`ARRAY_HISTOGRAM` accepts list elements represented as Arrow `Utf8View` and
counts their logical Unicode text values, preserving NULL elements. The
returned map uses the Engine's stable UTF-8 key representation.

`ARRAYS_OVERLAP` accepts regular and 64-bit-offset lists whose element type is
`Utf8View`, compares logical text rather than view buffers, and preserves
three-valued NULL semantics for NULL list elements and NULL child values.

## Evidence

- `cargo test -p kaveon-exec array_histogram_accepts_utf8_view_elements` —
  passes for duplicate text, ordering, and NULL elements.
- `cargo test -p kaveon-exec arrays_overlap_accepts_utf8_view_lists` — passes
  for duplicate text and NULL child values.
- `cargo clippy -p kaveon-exec --all-targets -- -D warnings` — passes.
- `cargo fmt --all -- --check` — passes.

## Boundary

This qualifies text-valued list elements for `ARRAY_HISTOGRAM`. It does not
claim unsupported nested map ordering or locale-specific collation.
