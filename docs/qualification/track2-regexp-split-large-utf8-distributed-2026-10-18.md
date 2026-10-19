# Track 2: `REGEXP_SPLIT` large UTF-8 and distributed parity

Date: 2026-10-18  
Scope: bounded SQL scalar-function parity

Kaveon's `REGEXP_SPLIT` evaluator now accepts both Arrow `Utf8` and `LargeUtf8`
input values (and either string representation for the pattern). It preserves
null input rows and trailing empty fields, returning the existing list-of-UTF8
result shape.

Evidence:

- Local executor regression: `regexp_split_accepts_large_utf8_values_and_patterns`
  in `engine/crates/exec/src/expr_eval.rs`.
- Three-worker differential: `regexp_split_distributed` in
  `engine/crates/server/src/differential_tests.rs`, run with
  `KAVEON_DIFF_ONLY=regexp_split_distributed`.
- Result: local and distributed paths produced identical rows; the focused
  differential passed.

Boundary: this covers the supported regex split scalar surface and Arrow text
widths. Full external pinned-Trino differential coverage remains open.
