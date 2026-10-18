# Track 1 — fail-closed narrow integer runtime-filter coercion

Runtime-filter literals arrive at the storage boundary as `Int64`, while
narrow signed, unsigned, date, time, and timestamp columns use their own Arrow
types. The literal compiler now checks the target range before constructing the
typed Arrow literal. Out-of-range and negative-to-unsigned values decline
pushdown instead of allowing an unchecked cast to wrap and change the filter's
meaning; the exact executor path remains responsible for the original value.

## Evidence

```text
cargo test -p kaveon-storage --lib scan_predicate --quiet
running 8 tests
test result: ok. 8 passed; 0 failed

cargo test -p kaveon-storage --lib --quiet
test result: ok. 140 passed; 0 failed; 1 ignored

cargo clippy -p kaveon-storage --all-targets -- -D warnings
Finished successfully

cargo fmt --all -- --check
passed
```

The focused regression covers UInt32 overflow, negative UInt32, and Date32
overflow. Cloud row-group pruning and deployed-image qualification remain open.
