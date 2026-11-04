# T-SQL `DATE_BUCKET(week, ...)` qualification — 2026-10-30

## Contract

Kaveon accepts the bounded SQL Server/Fabric form
`DATE_BUCKET(week, width, date[, origin])` for positive literal or bounded
row-local `BIGINT` widths. Week buckets are evaluated as seven-day calendar
units using the existing typed date arithmetic. Invalid widths and unsupported
origin expressions remain fail-closed.

This is a read-compatibility slice, not a claim of complete SQL Server date
semantics for every locale or calendar setting.

## Evidence

- Parser/planner regression:
  `cargo test -p kaveon-sql --lib bounded_date_bucket_lowers_literal_widths_and_row_local_widths`
- Exact local/three-worker differential across dictionary, plain, partitioned,
  clustered, and multi-file Parquet layouts:
  ```text
  KAVEON_DIFF_ONLY=date_bucket_week_datepart \
  cargo test -p kaveon-server --bin kaveon-server \
    the_differential_sweep_matches_across_parquet_encodings_and_execution_paths
  ```
  Result: 1/1 case passed with exact grouped rows and counts.

The case was recorded in the 112-case historical corpus revision. The current
authoritative corpus is maintained in
`docs/qualification/tsql-read-compatibility-corpus-2026-10-28.json` and
currently contains 182 cases.
