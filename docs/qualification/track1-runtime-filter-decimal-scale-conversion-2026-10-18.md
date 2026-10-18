# Track 1 — exact decimal runtime-filter scale conversion

Runtime-filter literals may arrive with a different decimal scale than the
probe column. The storage predicate compiler now rescales Decimal128 and
Decimal256 literals exactly: increasing scale uses checked multiplication;
reducing scale is accepted only when discarded digits are zero. Lossy or
overflowing conversions return no storage predicate, leaving the exact
executor filter in control.

## Evidence

```text
cargo test -p kaveon-storage decimal_runtime_filter_rescales_only_exact_values --quiet
running 1 test
.
test result: ok. 1 passed; 0 failed

cargo test -p kaveon-storage decimal256_runtime_filter_rescales_without_narrowing --quiet
running 1 test
.
test result: ok. 1 passed; 0 failed
```

The tests verify both directions, rejection of non-zero discarded digits, and
typed Arrow literal metadata after conversion. This is local storage evidence;
cloud row-group pruning and deployed-image qualification remain open.
