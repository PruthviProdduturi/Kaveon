# Track 2 — `POSITION` scalar compatibility — 2026-10-29

## Boundary

`POSITION(needle IN haystack)` is parsed by sqlparser as a dedicated AST
expression. Kaveon now lowers it with the SQL-required needle/haystack order
to the vectorized `POSITION` evaluator, which returns one-based Unicode scalar
positions, `0` when the needle is absent, and NULL when either input is NULL.

This qualifies the scalar expression for bounded text inputs in Engine
execution. It does not claim locale-specific collation or external-client
coverage.

## Evidence

Planner lowering:

```text
cargo test -p kaveon-sql position -- --nocapture
```

Distributed differential:

```text
cargo test -p kaveon-server --bin kaveon-server differential_tests::the_differential_sweep_matches_across_parquet_encodings_and_execution_paths -- --nocapture
```

The full multi-layout and three-worker differential passed with exact local
versus distributed results on 2026-10-29 (629.42 seconds).
