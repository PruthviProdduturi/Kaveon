# Track 2 qualification: Decimal128 `ARRAY_SUM`

Kaveon now supports `ARRAY_SUM` over Decimal128 list elements. The result preserves the input decimal precision and scale, ignores null elements, returns null for an all-null list, and reports a deterministic execution error on checked `i128` overflow. The same expression is exercised in the local and distributed differential corpus alongside decimal `ARRAY_AVERAGE` and `ARRAY_CUM_SUM`.

The focused execution test covers a scale-2 decimal list (`1.25 + 2.30 = 3.55`), verifies the stored decimal value, checks decimal overflow rejection, and checks that all-null and NULL-array inputs produce NULL. The distributed differential sweep compares the result across the supported Parquet encodings and execution paths.

Validation from the Engine worktree:

```text
cargo fmt --all
cargo test -p kaveon-exec array_average_and_sum_accept_decimal_elements --quiet
cargo clippy --workspace --all-targets -- -D warnings
cargo test -p kaveon-server differential_tests::the_differential_sweep_matches_across_parquet_encodings_and_execution_paths --quiet
```

This qualifies the supported Decimal128 `ARRAY_SUM` expression. It does not qualify arbitrary nested decimal casts, unbounded decimal precision, or general typed-table DML.

The adjacent `ARRAY_AVERAGE` path also accepts UInt64 list elements and returns a DOUBLE result; this is covered by `array_average_accepts_uint64_elements`. `ARRAY_SUM` now accepts UInt64 list elements when the signed BIGINT result is representable and rejects values outside that range with a checked `ARRAY_SUM bigint overflow` error; this is covered by `array_sum_accepts_uint64_elements_without_wrapping`.
