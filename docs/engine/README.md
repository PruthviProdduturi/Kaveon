# Kaveon Engine manual

Kaveon Engine is Kaveon's standalone distributed, vectorized analytical SQL query engine. Studio and the deterministic DLM are separate product pillars that consume Engine through governed interfaces.

## Evidence levels

| Level | Meaning |
|---|---|
| Contract | A validated shared type or protocol exists; execution is not implied |
| Local | The operator executes in one process with focused tests |
| Distributed | Coordinator fragments and workers execute the path |
| Docker-verified | Multiple containers completed representative queries |
| Production-qualified | Scale, concurrency, failure, security, and operations meet release gates |

“Implemented” does not mean production-qualified unless evidence explicitly says so.

## Manual

- [Architecture and startup](architecture-and-startup.md)
- [SQL and execution](sql-and-execution.md)
- [Distributed runtime](distributed-runtime.md)
- [Storage and catalogs](storage-and-catalogs.md)
- [Memory management](../reference/engine-memory-management.md)
- [Operations and roadmap](operations-and-roadmap.md)

`HANDSHAKE.md` is the live coordination record, `STATUS.md` is the product ledger, and `engine/DISTRIBUTED_EXECUTION_STATUS.md` records distributed verification.

