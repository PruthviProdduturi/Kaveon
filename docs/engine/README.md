# Kaveon Engine manual

Kaveon Engine is Kaveon's standalone distributed, vectorized analytical SQL
query engine. Studio and the deterministic DLM are separate product pillars
that reach the Engine through the API's authenticated bridge.

## Evidence levels

| Level | Meaning |
|---|---|
| Contract | A validated shared type or protocol exists; execution is not implied |
| Local | The operator executes in one process with focused tests |
| Distributed | Coordinator fragments and workers execute the path |
| Cluster-verified | The three-worker AKS test cluster completed representative statements, with a record under `docs/qualification/` or a HANDSHAKE Log row naming the image digest |
| Production-qualified | Scale, concurrency, failure, security, and operations meet release gates |

"Implemented" does not mean production-qualified unless evidence explicitly
says so. The Engine is alpha throughout: every path below is implemented and
tested, most are cluster-verified, none is production-qualified.

## Manual

- [Architecture and startup](architecture-and-startup.md)
- [SQL and execution](sql-and-execution.md)
- [Distributed runtime](distributed-runtime.md)
- [Storage and catalogs](storage-and-catalogs.md)
- [The learning engine](learning-engine.md) — `ANALYZE`, the statistics object, what answers without a scan, what the readers skip
- [Memory management](../reference/engine-memory-management.md)
- [Settings reference](settings.md)
- [HTTP API](../reference/api.md)
- [SQL compatibility](../reference/engine-sql-compatibility.md)
- [Operations and roadmap](operations-and-roadmap.md)

`HANDSHAKE.md` is the live coordination record, `STATUS.md` is the product
ledger, `engine/DISTRIBUTED_EXECUTION_STATUS.md` records distributed
verification commit by commit, and `docs/qualification/` holds the measured
records.
