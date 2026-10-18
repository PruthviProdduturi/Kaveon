# Track 1 — UInt32 RANGE window offsets

The window evaluator now accepts `RANGE` frames with numeric offsets over
`UInt32` ordering columns. Boundary arithmetic widens the unsigned values to
`i128`, matching the existing Int32/Int64/UInt64 path without precision loss.

## Evidence

```text
cargo test -p kaveon-exec --lib window --quiet
running 15 tests
test result: ok. 15 passed; 0 failed

cargo clippy -p kaveon-exec --all-targets -- -D warnings
Finished successfully

cargo fmt --all -- --check
passed
```

The focused regression covers `COUNT(*) OVER (ORDER BY UInt32 RANGE BETWEEN
1 PRECEDING AND CURRENT ROW)` with a NULL peer, verifying exact frame counts.
Broader distributed-window parallelism and cloud qualification remain open.
