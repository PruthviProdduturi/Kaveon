# Track 2 — UTF8View `REVERSE` text compatibility

## Boundary

`REVERSE(value)` now accepts Arrow `Utf8View` values in addition to `Utf8` and
`LargeUtf8`. It reverses Unicode scalar values, preserves NULL rows, and
returns ordinary `VARCHAR` (`Utf8`) output. The result type is intentional:
`Utf8View` is an input storage representation, not a client-visible debug
format or a promise that the view backing buffer survives the operator.

This closes the same logical-text boundary that previously caused view-backed
values to fall through to unsupported/debug paths. It does not change binary,
locale, or collation semantics.

## Evidence

The focused executor regression
`reverse_preserves_utf8_view_values_and_nulls` passes with:

- ordinary ASCII and non-ASCII Unicode scalars;
- a NULL input row;
- exact `VARCHAR` output type and values.

The existing local/three-worker text differential continues to cover the same
`REVERSE` semantics for storage-backed text columns. Distributed chart/result
serialization remains separately qualified by
`track5-utf8view-result-serialization-2026-11-02.md`.

## Qualification limit

This is an executor input-width qualification, not a claim of collation-aware
or SQL Server UTF-16 behavior. Locale-specific reversal and grapheme-cluster
reordering remain outside the Engine contract.
