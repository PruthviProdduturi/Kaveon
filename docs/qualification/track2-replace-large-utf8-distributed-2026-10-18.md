# Track 2: scalar `REPLACE` LargeUtf8 compatibility

Date: 2026-10-18  
Scope: bounded SQL scalar-function parity

The scalar `REPLACE(value, search, replacement)` evaluator now accepts
`LargeUtf8` source values and mixed-width string operands while preserving the
source's LargeUtf8 output type. NULL source, search, or replacement values
remain NULL.

Evidence:

- Local executor regression: `replace_accepts_large_utf8_values_and_operands`
  in `engine/crates/exec/src/expr_eval.rs`.
- Three-worker differential: `replace_large_utf8_compatibility` in
  `engine/crates/server/src/differential_tests.rs`.
- Focused local and distributed results match exactly.

Boundary: this qualifies the scalar replacement function and Arrow text widths.
Full string-function and external pinned-Trino qualification remain open.
