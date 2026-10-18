# Track 1 — exact decimal runtime-filter scale conversion

Runtime-filter literals may arrive with a different decimal scale than the
probe column. The storage predicate compiler now rescales Decimal128 and
Decimal256 literals exactly: increasing scale uses checked multiplication;
reducing scale is accepted only when discarded digits are zero. Lossy or
overflowing conversions return no storage predicate, leaving the exact
executor filter in control. The public Parquet reader validation path uses the
same exact conversion before compiling its `IN` predicate, so a scale mismatch
does not reject a value that is exactly representable at the probe scale.

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

cargo test -p kaveon-storage --lib exact_scale_conversion --quiet
running 2 tests
..
test result: ok. 2 passed; 0 failed
```

The focused compiler tests verify both directions, rejection of non-zero
discarded digits, and typed Arrow literal metadata after conversion. The two
Parquet reader tests additionally exercise Decimal128 and Decimal256 runtime
filters arriving at scale 3 for scale-2 columns and verify exact rows after
decoding. This is local storage evidence; cloud row-group pruning and
deployed-image qualification remain open.
