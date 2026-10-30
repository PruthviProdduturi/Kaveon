# Track 2 — T-SQL `REPLICATE` qualification — 2026-10-29

## Scope

Kaveon accepts SQL Server's `REPLICATE(text, count)` spelling through the
existing bounded Unicode `REPEAT` evaluator. The qualified contract covers
UTF-8 text, a non-negative integer-literal count, NULL propagation, and the
shared bounded string-expansion memory guard. Dynamic counts and negative
counts fail closed until SQL Server's exact row-local semantics have a
dedicated evaluator. Binary-type and 8,000-byte truncation rules are not
claimed.

## Evidence

The planner canonicalizes `REPLICATE` to `REPEAT`, and the corpus includes the
distributed case `replicate`:

```text
cargo test -p kaveon-server --bin kaveon-server \
  the_differential_sweep_matches_across_parquet_encodings_and_execution_paths
```

The full local execution suite and strict workspace Clippy gate also cover the
shared `REPEAT` evaluator and memory accounting.

## Boundary

Only the bounded text form with a non-negative integer-literal count is
qualified. Dynamic or negative counts, binary inputs, SQL Server's legacy byte
truncation behavior, and unbounded expansion remain outside the contract.
