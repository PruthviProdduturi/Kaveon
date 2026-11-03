# Qualification: view-backed text in regex functions

Date: 2026-11-03  
Scope: vectorized `REGEXP_REPLACE` dictionary/unique-value fast path

## Contract

`REGEXP_REPLACE` over an Arrow `Utf8View` column must preserve logical UTF-8
text and NULLs while retaining the evaluator's distinct-value optimization.
The physical view representation must not force an unsupported-type fallback.

## Evidence

- `cargo test -p kaveon-exec regexp_replace_accepts_utf8_view_columns` —
  passes for Unicode text, NULL, and a second text value.
- `cargo clippy -p kaveon-exec --all-targets -- -D warnings` — passes.
- `cargo fmt --all -- --check` — passes.

## Boundary

This qualifies `REGEXP_REPLACE`'s view-backed text path. It does not claim
locale-specific regex behavior or unsupported regex syntax.
