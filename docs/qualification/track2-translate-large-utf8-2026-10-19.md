# Track 2 — `TRANSLATE` LargeUtf8 compatibility

`TRANSLATE(value, from, to)` now accepts `LargeUtf8` operands and preserves the
source width in its result. Mapping follows Trino's first-occurrence rule for
duplicate source characters; characters without a corresponding replacement
are removed, and NULL operands produce NULL.

Focused evidence from `engine/`:

```text
cargo fmt --all -- --check
cargo test -p kaveon-exec evaluates_translate_for_large_utf8 -- --nocapture
test expr_eval::tests::evaluates_translate_for_large_utf8 ... ok
test result: ok; 1 passed; 0 failed; 560 filtered out
```

This is local evaluator coverage. Storage-backed LargeUtf8 Parquet and external
pinned-Trino differential qualification remain open.
