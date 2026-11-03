# Qualification: view-backed text in scalar expressions

Date: 2026-11-03  
Scope: vectorized expression text-value extraction (`kaveon-exec`)

## Contract

Scalar text functions and conditional result selection that consume a column
value must read Arrow `Utf8View` as logical UTF-8 text, preserving Unicode
values and SQL NULLs. The physical view representation must not be reported as
an unsupported type or leak into function output. When a scalar function runs
over an Arrow dictionary, transformed `Utf8View` values remain dictionary
encoded and retain the original row keys; NULL values stay in the key array.

## Evidence

- `cargo test -p kaveon-exec text_value_reads_utf8_view_values` — passes for
  Unicode, NULL, and ordinary text values.
- `cargo test -p kaveon-exec case_zip_preserves_utf8_view_text_and_nulls` —
  passes for conditional text selection and NULL mask behavior.
- `cargo test -p kaveon-exec rewrap_dictionary_preserves_utf8_view_values` —
  passes for dictionary-preserving text values and row-key reconstruction.
- `cargo clippy -p kaveon-exec --all-targets -- -D warnings` — passes.
- `cargo fmt --all -- --check` — passes.

## Boundary

This qualifies the shared scalar text extraction boundary used by the
expression evaluator. It does not claim full locale/collation semantics or
unsupported nested function shapes.
