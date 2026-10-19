# Track 2 — basic text mappers on LargeUtf8

`UPPER`, `LOWER`, `TRIM`, and `CODEPOINT` now accept Arrow `LargeUtf8` values and preserve
the source width in their output. `CODEPOINT` validates one Unicode code point per non-NULL value. NULL inputs remain NULL, and the operations
share one width-aware mapper so narrow and wide strings follow the same path.

Focused evidence from `engine/`:

```text
cargo fmt --all -- --check
cargo test -p kaveon-exec evaluates_basic_text_mappers_for_large_utf8 -- --nocapture
test expr_eval::tests::evaluates_basic_text_mappers_for_large_utf8 ... ok
test result: ok; 1 passed; 0 failed; 563 filtered out
```

This is local evaluator coverage. Storage-backed LargeUtf8 Parquet and external
pinned-Trino differential qualification remain open.
