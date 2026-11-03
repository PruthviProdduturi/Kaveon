# Server and SQL compatibility suite — 2026-11-03

## Scope

This qualification records the local release-image test gates for the current
`dev` branch. It is correctness evidence for the Engine and API crates; it is
not an AKS, object-store fault-injection, or external-client qualification.

## Commands and results

From `engine/` at commit `4991739f`:

```text
cargo test -p kaveon-sql --lib
370 passed; 0 failed; 0 ignored

cargo test -p kaveon-server --bin kaveon-server
696 passed; 0 failed; 0 ignored
finished in 685.40s

cargo test -p kaveon-exec --lib
626 passed; 0 failed; 4 ignored
finished in 37.85s

cargo test -p kaveon-optim --lib
86 passed; 0 failed; 0 ignored

cargo test -p kaveon-storage --lib
158 passed; 0 failed; 3 ignored

cargo test -p kaveon-catalog --lib
93 passed; 0 failed; 4 ignored

cargo clippy -p kaveon-optim --all-targets -- -D warnings
passed
```

Together these suites cover the versioned T-SQL corpus, distributed
differential execution across Parquet encodings, joins, windows, aggregates,
JSON, bounded product DML/MERGE, catalog access, cancellation, retry/recovery,
result-cache, storage/catalog behavior, optimizer planning, and `Utf8View`
result serialization. The SQL suite covers parser and planner boundaries,
including explicit rejection of unsupported shapes.

The focused `Utf8View` regression also covers nullable text values and map keys;
those values are emitted as logical JSON strings or `null`, never Arrow debug
representations.

## Boundary

This evidence does not close the remaining tracker items: broader MERGE and
conflict semantics, full per-operator runtime attribution, adaptive
high-cardinality exchange, cloud/object-store recovery, cross-source joins, or
JDBC/ODBC and external Trino-client qualification remain open.
