# `REGEXP_LIKE` column-predicate qualification — 2026-11-03

The Engine qualifies the two-argument Trino `REGEXP_LIKE(value, pattern)`
predicate when the value is a text column and the pattern is a literal. It
returns SQL NULL when either input is NULL and otherwise returns the exact
regular-expression match result. The case is evaluated against plain,
dictionary-encoded, multi-file, partitioned, clustered, and three-worker
fragment layouts.

Evidence:

```text
$env:KAVEON_DIFF_ONLY='regexp_like_column_predicate'
cargo test -p kaveon-server --bin kaveon-server the_differential_sweep_matches_across_parquet_encodings_and_execution_paths -- --nocapture
```

The focused differential passed with exact local/distributed results and zero
retained query-memory reservations. This does not claim regex flags or broader
pattern-dialect compatibility beyond the Engine's compiled regex contract.
