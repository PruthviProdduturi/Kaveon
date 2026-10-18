# Decimal256 runtime-filter qualification — 2026-10-18

Decimal256 equality keys now use an exact typed 256-bit hash index in the join
build and probe paths. The runtime batch filter checks the original
`Decimal256Array` values directly and does not narrow them to Decimal128 or a
storage scalar. Duplicate keys and NULL probe values retain the existing join
semantics.

## Evidence

From `engine/`:

```text
cargo test -p kaveon-exec decimal256_ -- --nocapture
```

Result: **3 passed**. The tests cover exact values outside Decimal128's range,
duplicate build keys, NULL handling, and the typed runtime-filter path.

This is local executor evidence. Decimal256 scale conversion, cloud storage
predicate pushdown, and deployed-image qualification remain open.
# Track 1 — Decimal256 runtime-filter qualification

This qualification covers exact `DECIMAL(40,2)` equality runtime filters on
Parquet. Build-side 256-bit keys now cross the storage predicate ABI as
decimal text and are parsed directly into Arrow `Decimal256Array` values. No
conversion through `i128` or `f64` occurs. The probe column's precision and
scale remain authoritative; a mismatched scale is rejected, preserving exact
join semantics.

## Evidence

The focused regression writes four values in two Parquet row groups, applies
an exact two-value runtime filter before decoding, and returns only the two
matching values:

```text
cargo test -p kaveon-storage applies_decimal256_runtime_filter_without_narrowing --quiet
running 1 test
.
test result: ok. 1 passed; 0 failed

cargo test -p kaveon-exec decimal256_runtime_filter_uses_exact_typed_index_without_narrowing --quiet
running 1 test
.
test result: ok. 1 passed; 0 failed
```

This is local Parquet and executor acceptance evidence. It does not claim
deployed-image qualification, measurable cloud row-group reduction, or
cross-scale Decimal256 coercion; those remain explicit tracker boundaries.
