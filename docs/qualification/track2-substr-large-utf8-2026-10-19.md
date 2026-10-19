# Track 2 — `SUBSTR`/`SUBSTRING` LargeUtf8 compatibility

`SUBSTR` and `SUBSTRING` now accept `LargeUtf8` values and preserve the source
width in their result. Character offsets are applied over Unicode scalar values
instead of byte slices, preventing invalid-boundary failures for multibyte text;
NULL inputs and the existing start/length behavior remain unchanged.

Focused evidence from `engine/`:

```text
cargo fmt --all -- --check
cargo test -p kaveon-exec evaluates_substr_for_large_utf8_without_splitting_unicode -- --nocapture
test expr_eval::tests::evaluates_substr_for_large_utf8_without_splitting_unicode ... ok
test result: ok; 1 passed; 0 failed; 564 filtered out
```

This is local evaluator coverage. Storage-backed LargeUtf8 Parquet and external
pinned-Trino differential qualification remain open.
