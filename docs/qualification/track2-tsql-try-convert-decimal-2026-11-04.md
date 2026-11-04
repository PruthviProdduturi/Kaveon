# T-SQL `TRY_CONVERT(DECIMAL(p,s), ...)` qualification — 2026-11-04

## Boundary

Kaveon qualifies the bounded SQL Server/Fabric form
`TRY_CONVERT(DECIMAL(p,s), expression)` for the existing typed decimal safe-cast
path. Valid numeric values preserve the requested precision and scale. A
malformed value, NULL input, or value that overflows the requested decimal
range returns a typed NULL rather than failing the statement. This qualification
is intentionally bounded to decimal targets and does not claim general
SQL Server style, locale, or arbitrary target-type parity.

## Evidence

The versioned read corpus adds `try_convert_decimal` and `convert_decimal` with planner status
`planner_qualified` and distributed status
`qualified_for_bounded_decimal_casts_and_null_on_error` or
`qualified_for_bounded_decimal_casts_and_strict_errors`, respectively.

Focused local-versus-three-worker differential:

```text
$env:KAVEON_DIFF_ONLY='tsql_try_convert_decimal'
cargo test -p kaveon-server --bin kaveon-server \
  differential_tests::the_differential_sweep_matches_across_parquet_encodings_and_execution_paths \
  -- --exact --nocapture
test result: ok. 1 passed; 0 failed
```

The case mixes valid numeric input with non-numeric text, orders by a complete
source key, and compares canonical local and distributed rows. It passed across
the differential Parquet layouts and the three-worker execution path.

The strict companion uses only valid row-local numeric text and has the same
exact local/distributed result gate. A separate planner regression confirms
that malformed strict decimal input remains an execution error rather than
silently becoming NULL.

The authoritative T-SQL corpus now contains 189 cases. Cloud deployment,
locale-sensitive conversion, and unbounded arbitrary-precision decimal
behavior remain outside this evidence.

## Explicit boundary

- Decimal precision/scale must be supplied as bounded literal `p,s` values
  supported by the Engine's typed decimal cast path.
- Conversion errors, overflow, and NULL input return typed NULL.
- Styled, locale-aware, CLR-backed, and non-decimal `TRY_CONVERT` targets are
  not implied by this qualification.
