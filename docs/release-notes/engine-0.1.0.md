# Kaveon Engine 0.1.0

The first numbered release of the Kaveon Engine. Until now the only way to run
it was a rolling `engine-dev` prerelease whose contents changed on every push,
so there was no way to say which build you had. This names one.

## What it is

A distributed, columnar query engine written in Rust on Arrow 54 and Parquet
54, with its own SQL parser and logical plan. It reads Parquet, Delta and
Iceberg from local disk or Azure Data Lake Storage Gen2, and runs as a
coordinator with any number of workers, or as a single process.

## The `kaveon` CLI

Download the binary for your platform, put it on your PATH, and point it at
data or at a server.

```
kaveon --local --data-dir /path/to/parquet/files
kaveon --server https://your-coordinator:8080 --ca-cert /path/to/ca.crt
```

The CLI is remote-first; `--local` is what runs an embedded engine.

## Distributed execution

Scan, filter and projection; partial and final aggregation, grouped and
global, over `COUNT`, `SUM`, `MIN`, `MAX`, weighted `AVG` and exact
`COUNT(DISTINCT)`; Sort and Top-N; repartitioned and broadcast hash joins;
Arrow IPC exchange with bounded, replayable flow control; task retry and
cancellation; and spill for aggregates, joins, Sort and Top-N.

An admission queue and resource groups bound what a query may take. The HTTP
statement API is authenticated — bridge tokens, principals, Entra delegated
tokens and roles — and serves TLS through rustls.

## What is qualified, and where

Correctness is qualified by differential testing: every distributed result is
compared against the same statement run by the local executor, over 2,404
workspace tests. Outer-join null-extension, mixed and empty window scopes,
concurrent cancellation and scheduler lease fencing under repeated takeover
are each covered.

Throughput and reliability are qualified on a three-worker AKS cluster.
Several workstreams are measured locally and await cluster qualification:
skew handling, join spill, residual-join pressure, multi-query stress, a
sustained soak, rolling-restart recovery, and remote object-store split
assignment. `engine/DISTRIBUTED_EXECUTION_STATUS.md` carries the remaining
gate per workstream and is the honest reference.

## Known limits

- **No S3 reader.** S3 can be registered as a catalog storage type, but there
  is no reader behind it. ADLS Gen2 and local disk are the working paths.
- **Delta replay has no checkpoints.** Commits replay from version 0, so a
  table with a long history costs more to open than it should.
- **High-cardinality aggregation is slower than the best columnar engines.**
  A grouped aggregate over millions of groups is the shape to watch; the
  columnar aggregate landed for it and the gap has closed substantially, but
  it has not closed.
- **`Utf8View` columns serialise incorrectly** through `/v1/statement`,
  returning debug text instead of the value. `CAST(col AS VARCHAR)` is the
  workaround until the fix lands.

## Compatibility

A bounded T-SQL read surface, tracked against Fabric Warehouse, Azure SQL,
PostgreSQL and Trino in `docs/engineering/sql-compatibility-gap-plan.md`. A
capability is listed as qualified there only after parser, planner, local
execution, distributed execution and result-correctness tests pass; parser
acceptance alone is not a parity claim.

Registered Fabric SQL and Azure SQL sources are API passthrough. They are not
executed by the Engine and cannot be joined with an Engine catalog in one
statement.

MIT licensed.
