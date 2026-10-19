# Track 2 — split-map functions with LargeUtf8 inputs

`SPLIT_TO_MAP` and `SPLIT_TO_MULTIMAP` now accept `LargeUtf8` source strings
and delimiters. Their map keys and values retain the existing VARCHAR output
contract, while NULL handling, duplicate-key rejection/grouping, and delimiter
validation remain unchanged.

Focused evidence from `engine/`:

```text
cargo fmt --all -- --check
cargo test -p kaveon-exec split_to_ -- --nocapture
test result: ok; 2 passed; 0 failed; 563 filtered out
```

This is local evaluator coverage. Storage-backed LargeUtf8 Parquet and external
pinned-Trino differential qualification remain open.
