# Track 2 — `REGEXP_REPLACE` LargeUtf8 compatibility

`REGEXP_REPLACE(value, pattern, replacement)` now accepts Arrow `LargeUtf8`
values, patterns, and replacements. The evaluator preserves the source's
`LargeUtf8` output type, retains Trino NULL propagation when any operand is
NULL, and keeps the existing compiled-pattern cache and expression cancellation
checks. Replacement growth remains charged against the expression memory
budget.

Focused evidence from `engine/`:

```text
cargo fmt --all -- --check
cargo test -p kaveon-exec evaluates_regexp_replace_for_large_utf8 -- --nocapture
test expr_eval::tests::evaluates_regexp_replace_for_large_utf8 ... ok
test result: ok; 1 passed; 0 failed; 559 filtered out
```

This is local evaluator coverage. A storage-backed LargeUtf8 Parquet and
external pinned-Trino differential run remain open.
