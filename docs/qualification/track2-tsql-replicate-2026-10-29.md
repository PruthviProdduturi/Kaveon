# Track 2 — T-SQL `REPLICATE` qualification — 2026-10-29

## Scope

Kaveon accepts SQL Server's `REPLICATE(text, count)` spelling through the
existing bounded Unicode `REPEAT` evaluator. The qualified contract covers
UTF-8 text, a non-negative integer-literal or bounded row-local BIGINT
expression (including arithmetic and casts), NULL propagation, SQL Server's
NULL result for a negative count, and the shared bounded string-expansion
memory guard. Binary-type and 8,000-byte truncation rules are not claimed.

## Evidence

The planner canonicalizes `REPLICATE` to `REPEAT`, and the focused distributed
differential covers the row-local arithmetic count `actions - 20`, including
negative values that must become NULL:

```text
cargo test -p kaveon-server --bin kaveon-server \
  distributed_tsql_replicate_matches_local -- --nocapture
```

The full local execution suite and strict workspace Clippy gate also cover the
shared `REPEAT` evaluator and memory accounting.

## Boundary

Only bounded text with a non-negative literal or bounded row-local BIGINT
expression is qualified. Negative counts return NULL. Binary inputs, SQL
Server's legacy byte truncation behavior, and unbounded expansion remain
outside the contract.
