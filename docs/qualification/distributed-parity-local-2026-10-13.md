# Distributed parity: local qualification

Status: **qualified locally; deployed qualification remains open**.

This record captures the executable evidence for the distributed execution
paths in the current `dev` branch. It is deliberately separate from AKS
qualification: local tests prove semantics and recovery control flow, but do
not prove cluster latency, node pressure, or long-running network behavior.

## Results

| Gate | Command | Result |
|---|---|---|
| SQL planning and compatibility | `cargo test -p kaveon-sql --quiet` | 134 passed |
| Executor semantics and spill paths | `cargo test -p kaveon-exec --quiet` | 294 passed, 4 ignored benchmark cases |
| Server regression suite | `cargo test -p kaveon-server --bin kaveon-server --quiet` | 364 passed |
| Worker loss after producer exchange | `cargo test -p kaveon-server --bin kaveon-server --quiet a_worker_lost_between_the_producers_and_the_consumers_is_recovered_exactly` | 1 passed |
| Documentation contract | `node scripts/validate-docs.mjs` | 146 Markdown files, 39 routes, 9 SVGs |

The full workspace regression (`cargo test --workspace --quiet`) also passed:
all workspace targets were green, including the server differential suite
(364 tests), SQL (134), executor (294 plus four ignored benchmarks), optimizer
(48), and the remaining catalog/core/CLI/storage targets. The workspace run is
the broad regression gate; the focused rows above make the distributed gates
easy to reproduce.

The worker-loss test asserts an exact grouped result, identifies the lost
worker, records the moved spool, records stage retries, and verifies that the
surviving worker completes the final stage without missing or double-counting
partitions.

## 2026-10-16 — final-merge pressure gate

The release-mode ignored executor gate
`cargo test --release -p kaveon-exec final_merge_under_pressure -- --ignored --nocapture`
passed (`1 passed, 0 failed`, 61.28 seconds). It builds 6,000,000 partial
rows (812 MiB encoded), exercises the final merge under a 640 MiB reservation,
and repeats partitioned, broadcast, and reserving modes with tight and roomy
budgets. The tight runs completed exactly with bounded spill (305–458 MiB),
bounded memory (523–639 MiB), and no retained spill tables or source-thread
refusals in the partitioned/broadcast variants; the roomy runs completed with
zero spill. This is local release pressure evidence only. It does not replace
the open AKS disk-exhaustion, sustained mixed-workload, or network-fault gates.

## Scope boundary

These results qualify local semantics and recovery orchestration. They do not
close the Track 1 AKS gates for sustained mixed workload, cross-node latency,
coordinator restart, exchange pressure, or network fault injection. Those
still require a deployed artifact and archived result hashes, resource
telemetry, and failure/recovery timings. Track 2 still has explicit open
correlated-subquery, recursive-CTE, broad DML, and complex-type combinations.
