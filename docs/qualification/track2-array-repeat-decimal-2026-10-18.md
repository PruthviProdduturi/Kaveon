# `ARRAY_REPEAT` decimal scalar differential

Date: 2026-10-18

`ARRAY_REPEAT(CAST(actions AS DECIMAL(8, 2)), 2)` now preserves the decimal
precision and scale while producing the repeated list. The focused server
differential compares node-local evaluation with the three-worker fragment
path over four rows and verifies exact canonical results and zero retained
query memory.

```text
cargo test -p kaveon-server differential_tests::distributed_array_repeat_decimal_matches_local -- --nocapture
PASS (1 test)
```

The implementation is limited to scalar Decimal128 repeat values; broader
complex-element and external-client qualification remain separate work.
