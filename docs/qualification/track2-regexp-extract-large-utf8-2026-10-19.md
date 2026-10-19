# Track 2 — `REGEXP_EXTRACT` LargeUtf8 compatibility

`REGEXP_EXTRACT(value, pattern[, group])` now accepts `LargeUtf8` values and
patterns, and preserves the source width in its VARCHAR result. Group validation,
compiled-pattern reuse, and NULL propagation remain unchanged.

Focused evidence from `engine/`:

```text
cargo fmt --all -- --check
cargo test -p kaveon-exec evaluates_regexp_extract_for_large_utf8 -- --nocapture
test expr_eval::tests::evaluates_regexp_extract_for_large_utf8 ... ok
test result: ok; 1 passed; 0 failed; 562 filtered out
```

This is local evaluator coverage. Storage-backed LargeUtf8 Parquet and external
pinned-Trino differential qualification remain open.
