# Track 2 — concatenation with LargeUtf8 operands

`CONCAT`, `CONCAT_WS`, and the SQL string-concatenation operator (`||`) now
choose a width from their operands. When any operand is `LargeUtf8`, results
remain `LargeUtf8` instead of being narrowed through an intermediate `Utf8`
cast; numeric coercion, NULL separator behavior, and NULL operand semantics are
preserved.

Focused evidence from `engine/`:

```text
cargo fmt --all -- --check
cargo test -p kaveon-exec concat_ws_skips_null_values_and_propagates_null_separator -- --nocapture
test ... concat_ws_skips_null_values_and_propagates_null_separator ... ok
test result: ok; 1 passed; 0 failed; 564 filtered out
```

This is local evaluator coverage. Storage-backed LargeUtf8 Parquet and external
pinned-Trino differential qualification remain open.
