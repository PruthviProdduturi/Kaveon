# Server and SQL compatibility suite — 2026-11-03

## Scope

This qualification records the local release-image test gates for the current
`dev` branch. It is correctness evidence for the Engine and API crates; it is
not an AKS, object-store fault-injection, or external-client qualification.

## Commands and results

From `engine/` at commit `b8d69cb4`:

```text
cargo test -p kaveon-sql --lib
370 passed; 0 failed; 0 ignored

cargo test -p kaveon-server --bin kaveon-server
696 passed; 0 failed; 0 ignored
finished in 685.40s
```

The server suite includes the versioned T-SQL corpus, distributed differential
execution across Parquet encodings, joins, windows, aggregates, JSON and
bounded product DML/MERGE, catalog access, cancellation, retry/recovery,
result-cache, and `Utf8View` result serialization. The SQL suite covers parser
and planner boundaries, including explicit rejection of unsupported shapes.

The focused `Utf8View` regression also covers nullable text values and map keys;
those values are emitted as logical JSON strings or `null`, never Arrow debug
representations.

## Boundary

This evidence does not close the remaining tracker items: broader MERGE and
conflict semantics, full per-operator runtime attribution, adaptive
high-cardinality exchange, cloud/object-store recovery, cross-source joins, or
JDBC/ODBC and external Trino-client qualification remain open.
