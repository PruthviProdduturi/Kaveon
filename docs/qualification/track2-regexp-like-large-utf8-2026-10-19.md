# Track 2 — `REGEXP_LIKE` LargeUtf8 compatibility

`REGEXP_LIKE(value, pattern)` now accepts Arrow `LargeUtf8` values and
patterns. It preserves Trino's NULL propagation and uses the existing compiled
pattern cache, so the wider string representation does not change matching
semantics or cancellation behavior.

Focused evidence from `engine/`:

```text
cargo fmt --all -- --check
cargo test -p kaveon-exec evaluates_regexp_like_for_large_utf8 -- --nocapture
test expr_eval::tests::evaluates_regexp_like_for_large_utf8 ... ok
test result: ok; 1 passed; 0 failed; 561 filtered out
```

This is local evaluator coverage. Storage-backed LargeUtf8 Parquet and external
pinned-Trino differential qualification remain open.
