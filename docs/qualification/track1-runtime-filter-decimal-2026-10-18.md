# Track 1 — Decimal runtime-filter qualification

This qualification covers the bounded Parquet runtime-filter path for exact
`DECIMAL(10,2)` equality keys. A build-side decimal key is represented by the
join index at precision 38; the storage predicate retags the one-value Arrow
literal to the probe column precision without converting its unscaled value.
The scales must match, and values that do not fit the probe precision are
rejected before pushdown.

## Evidence

The focused regression writes four decimal values to a two-row-group Parquet
file, applies a two-value runtime `IN` filter before the first decode, and
returns exactly the two matching unscaled values:

```text
cargo test -p kaveon-storage applies_decimal_runtime_filter_with_wider_build_precision --quiet
running 1 test
.
test result: ok. 1 passed; 0 failed
```

The complete storage runtime-filter group also passes:

```text
cargo test -p kaveon-storage runtime_filter --quiet
running 11 tests
test result: ok. 11 passed; 0 failed
```

This is local Parquet acceptance evidence. It does not claim deployed-image
qualification, measurable cloud row-group reduction, Decimal256 support, or
decimal scale conversion; those remain explicit boundaries in the tracker.
