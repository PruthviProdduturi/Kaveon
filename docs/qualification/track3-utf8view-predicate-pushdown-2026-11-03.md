# Qualification: view-backed text predicate pushdown

Date: 2026-11-03  
Scope: storage row-filter compilation for Parquet/Delta scans

## Contract

Text predicates over Arrow `Utf8View` columns must compile to the same bounded
storage filter path as ordinary UTF-8 columns. A scalar text literal is
materialized as a view-backed literal of the matching logical type, preserving
NULL semantics and allowing the decoder to reject rows before later columns
are materialized.

## Evidence

- `cargo test -p kaveon-storage utf8_view_runtime_filter_compares_text_without_falling_back`
  — passes for matching Unicode text, a non-matching value, and NULL.
- `cargo clippy -p kaveon-storage --all-targets -- -D warnings` — passes.
- `cargo fmt --all -- --check` — passes.

## Boundary

This qualifies scalar text comparison literal compilation for `Utf8View`. It
does not claim broader locale-specific collation, unsupported predicate forms,
or external-client behavior.
