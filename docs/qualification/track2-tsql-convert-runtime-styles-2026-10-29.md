# T-SQL runtime `CONVERT` styles — 2026-10-29

## Scope

This record qualifies the bounded SQL Server/Fabric date conversion surface
for text columns. It does not claim general T-SQL `CONVERT` style coverage.

## Evidence

The focused distributed differential was run from `engine/` with:

```text
$env:KAVEON_DIFF_ONLY='tsql_runtime_date_style'
cargo test -p kaveon-server --bin kaveon-server \
  differential_tests::the_differential_sweep_matches_across_parquet_encodings_and_execution_paths \
  -- --nocapture
```

Result: **1 passed, 0 failed** (1.28 seconds).

The case compares local and three-worker execution over the supported Parquet
layouts for:

```sql
SELECT
  TRY_CONVERT(DATE, value, 0) AS named_date,
  TRY_CONVERT(DATE, value, 1) AS us_short_date,
  TRY_CONVERT(DATE, value, 101) AS us_date,
  TRY_CONVERT(DATE, value, 103) AS eu_date,
  TRY_CONVERT(DATE, value, 23) AS iso_dashed_date,
  TRY_CONVERT(DATE, value, 112) AS iso_date
FROM (VALUES
  ('Dec 31 2024'), ('12/31/2024'), ('31/12/2024'), ('2024-12-30'),
  ('20241230'), ('bad'), (NULL)
) AS values_table(value)
```

Invalid and NULL inputs remain NULL under `TRY_CONVERT`; valid values use the
style-specific parser. The current bounded runtime styles are `0`, `1`, `23`,
`101`, `103`, and `112`; unsupported style codes remain explicit errors. The
differential completed with **1 passed, 0 failed** after this six-style expansion.

## Boundary

This is a read-only, text-to-DATE contract. It does not add style-aware
conversion for timestamp/time targets, character formatting, locale-sensitive
styles, or arbitrary runtime style expressions.
