# Track 2 — Current SQL compatibility and differential gate

## Scope

This gate checks the current Engine source against the Trino-focused planner
tests and the full local/distributed differential corpus. It is a compatibility
and execution regression gate, not a claim that every Trino feature is
implemented.

## Evidence

Run from `engine/` at source `63211a6f`:

```text
cargo test -p kaveon-server trino_ --quiet
29 passed; 0 failed; 401 filtered out; finished in 0.27s

cargo test -p kaveon-server --bin kaveon-server differential_tests --quiet
41 passed; 0 failed; 389 filtered out; finished in 376.21s
```

The differential sweep exercises the same corpus across the supported Parquet
encodings and coordinator/partitioned execution paths. It includes the
five-way join reorder cases, arrays/maps/rows/JSON, windows and `QUALIFY`,
set operations, exact and approximate aggregates, `UNNEST`, prepared
parameters, and the bracketed numeric identifier regression.

## Boundary

These results prove the current corpus is stable. The Track 2 rows that remain
Partial—recursive CTE graphs, deeper correlated/lateral forms, broader complex
type and JSON coverage, and general DML—still require their own implementation
and qualification evidence.
