# Track 2: scalar `CONTAINS` LargeUtf8 compatibility

Date: 2026-10-18  
Scope: bounded SQL scalar-function parity

The scalar string form of `CONTAINS(value, substring)` now accepts both Arrow
`Utf8` and `LargeUtf8` inputs, including mixed-width values and needles. NULL
inputs retain Trino's NULL result semantics. Array `CONTAINS` dispatch remains
unchanged.

Evidence:

- Local executor regression: `contains_accepts_large_utf8_values_and_needles`
  in `engine/crates/exec/src/expr_eval.rs`.
- Three-worker differential: `contains_large_utf8_compatibility` in
  `engine/crates/server/src/differential_tests.rs`.
- Focused local and distributed results match exactly.

Boundary: this qualifies the scalar string overload and Arrow text widths. It
does not claim complete string-function or external pinned-Trino coverage.
