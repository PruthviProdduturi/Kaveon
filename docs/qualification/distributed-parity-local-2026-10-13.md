# Distributed parity: local qualification

Status: **qualified locally; deployed qualification remains open**.

This record captures the executable evidence for the distributed execution
paths in the current `dev` branch. It is deliberately separate from AKS
qualification: local tests prove semantics and recovery control flow, but do
not prove cluster latency, node pressure, or long-running network behavior.

## Results

| Gate | Command | Result |
|---|---|---|
| SQL planning and compatibility | `cargo test -p kaveon-sql --quiet` | 131 passed |
| Executor semantics and spill paths | `cargo test -p kaveon-exec --quiet` | 293 passed, 4 ignored benchmark cases |
| Server regression suite | `cargo test -p kaveon-server --bin kaveon-server --quiet` | 364 passed |
| Worker loss after producer exchange | `cargo test -p kaveon-server --bin kaveon-server --quiet a_worker_lost_between_the_producers_and_the_consumers_is_recovered_exactly` | 1 passed |
| Documentation contract | `node scripts/validate-docs.mjs` | 145 Markdown files, 39 routes, 9 SVGs |

The worker-loss test asserts an exact grouped result, identifies the lost
worker, records the moved spool, records stage retries, and verifies that the
surviving worker completes the final stage without missing or double-counting
partitions.

## Scope boundary

These results qualify local semantics and recovery orchestration. They do not
close the Track 1 AKS gates for sustained mixed workload, cross-node latency,
coordinator restart, exchange pressure, or network fault injection. Those
still require a deployed artifact and archived result hashes, resource
telemetry, and failure/recovery timings. Track 2 still has explicit open
correlated-subquery, recursive-CTE, broad DML, and complex-type combinations.
