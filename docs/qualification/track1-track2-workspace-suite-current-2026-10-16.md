# Track 1 and Track 2 workspace qualification

This file is the current qualification snapshot. Update the rows below when
the boundary or evidence changes; do not append session notes or duplicate
historical ledgers. Detailed implementation history remains in git history and
the parity tracker.

**Source:** current `dev` source after the cube ordering/limit qualification
and the timestamp map-key compatibility work.

## Current evidence

| Area | Command or scope | Result |
| --- | --- | --- |
| Server distributed execution | `cargo test -p kaveon-server --all-targets --no-fail-fast --quiet` | **423 passed, 0 failed, 0 ignored**; includes joins, aggregates, exchange, spill, retry, cancellation, worker-loss, set operations, lateral joins, and the differential sweep (151.33 s) |
| Execution operators | `cargo test -p kaveon-exec --all-targets --no-fail-fast --quiet` | **390 passed, 0 failed, 4 ignored** |
| SQL compatibility | Differential local/distributed corpus | Exact results for qualified array/map/row operations, set operations, typed histograms, map lookup, nested keys, and the current Trino compatibility cases; cube answers also preserve output-column `ORDER BY`, `NULLS FIRST/LAST`, `OFFSET`, and bounded `LIMIT` without a scan |
| Parser regression | `cargo test -p kaveon-sql bracketed_identifiers -- --nocapture` | **1 passed, 0 failed**; covers Studio-generated bracketed identifiers such as `SELECT DISTINCT [year] ...` |
| Formatting | `cargo fmt --all -- --check` | Passed |
| Lint gate | `cargo clippy --workspace --all-targets -- -D warnings` | Passed on Rust 1.98.1 |

## Implemented qualification boundaries

- Distributed DISTINCT, UNION, INTERSECT, and EXCEPT paths use stable
  expression keys where supported; unsupported shapes retain an explicit
  single-task fallback.
- Exchange partitioning has deterministic primitive and nested array/map/row
  keys, including reversed map-entry order.
- Typed `ARRAY_HISTOGRAM` covers integer, unsigned integer, floating-point,
  decimal, boolean, date, date64, UTF-8, large UTF-8, and timezone-less
  timestamp values with deterministic ordering and null handling.
- `MAP_ELEMENT_AT` and `MAP_CONTAINS_KEY` cover the supported primitive,
  decimal, date, UTF-8, and timezone-less timestamp key families.
- Correlated lateral and residual outer-join cases are checked against local
  results before distributed qualification.

## Remaining qualification gates

This repository evidence does not close the live Track 1 AKS gates or every
Track 2 compatibility boundary. The parity tracker remains authoritative for
those open boundaries, including live fault injection, cloud object-store
qualification, broader optimizer coverage, and unsupported SQL features.

## Reproduction

Run engine commands from `engine/` with the pinned toolchain:

```text
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test -p kaveon-exec --all-targets --no-fail-fast --quiet
cargo test -p kaveon-server --all-targets --no-fail-fast --quiet
```
