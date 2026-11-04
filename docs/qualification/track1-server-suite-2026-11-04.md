# Server compatibility suite — 2026-11-04

## Scope

This receipt records the current `dev` release-image server gate after the
`Utf8View` result-serialization fix. It is local correctness evidence for the
Engine server binary; it is not AKS, object-store fault-injection, or external
client qualification.

## Command and result

Run from `engine/` at commit `6e18d05c`:

```text
cargo test -p kaveon-server --bin kaveon-server --no-fail-fast --quiet
717 passed; 0 failed; 0 ignored; 0 filtered out
finished in 708.57s

cargo fmt --all -- --check
passed

cargo clippy -p kaveon-server --bin kaveon-server --tests -- -D warnings
passed
```

The suite includes the `Utf8View` result regression, distributed differential
execution, window and aggregate paths, JSON, bounded product DML/MERGE,
catalog access, cancellation/retry behavior, result-cache behavior, and
server API serialization.

## Boundary

This gate does not close the remaining qualification boundaries: broader
MERGE/conflict semantics, full per-operator runtime attribution, high-
cardinality adaptive exchange, cloud/object-store recovery, cross-source
joins, or JDBC/ODBC and external Trino-client qualification.
