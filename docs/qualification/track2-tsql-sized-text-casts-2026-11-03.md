# T-SQL sized text casts — 2026-11-03

## Boundary

Kaveon now qualifies bounded `CAST`/`TRY_CAST` and `CONVERT`/`TRY_CONVERT`
to `VARCHAR(n)`, `NVARCHAR(n)`, and `CHAR(n)` for Engine UTF-8 text. The
declared width counts Unicode scalar values. Variable-width casts truncate after
`n` scalars; fixed-width `CHAR(n)` casts truncate and then pad with U+0020
spaces. NULL input remains NULL.

This is an explicit Kaveon text contract, not SQL Server code-page or UTF-16
storage emulation. Binary input, locale/code-page conversion, and styled sized
text `CONVERT` are rejected. Lengths above 1,000,000 scalars fail closed to
bound allocation and output expansion.

## Evidence

- SQL planner lowering: `lowers_bounded_tsql_sized_text_casts_without_dropping_length`
- Executor Unicode/truncation/padding regression:
  `sized_text_casts_truncate_and_pad_unicode_scalars`
- Three-worker differential: `distributed_tsql_sized_text_cast_matches_local`
- Corpus case: `sized_text_cast`
- Full layout differential sweep: `the_differential_sweep_matches_across_parquet_encodings_and_execution_paths`
  passed with the sized-text case included and a complete deterministic
  `ORDER BY event_date, country, industry` tie-breaker.

Focused commands:

```text
cargo test -p kaveon-sql lowers_bounded_tsql_sized_text_casts_without_dropping_length -- --nocapture
cargo test -p kaveon-exec sized_text_casts_truncate_and_pad_unicode_scalars -- --nocapture
cargo test -p kaveon-server distributed_tsql_sized_text_cast_matches_local -- --nocapture
KAVEON_DIFF_ONLY=tsql_sized_text_cast cargo test -p kaveon-server \
  the_differential_sweep_matches_across_parquet_encodings_and_execution_paths -- --nocapture
```

The qualification is limited to bounded UTF-8 text and does not claim general
SQL Server `VARCHAR` code-page behavior.
