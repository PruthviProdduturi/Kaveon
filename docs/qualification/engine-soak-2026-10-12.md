# Five-worker native Engine soak — 2026-10-12

This run is a local operational qualification snapshot for the distributed
execution tracker. It is not a cloud or performance-comparison claim.

| Field | Result |
| --- | --- |
| Binary | `engine/target/debug/kaveon-server.exe` |
| Binary SHA-256 | `99284adea19ed201b7de52f1950c6b9be0c95f8f657f2a4cbd20a1c7ed1d2b6a` |
| Workers | 5 |
| Fixture | 100,000 rows |
| Requested duration | 120 s |
| Actual workload | 120.547 s |
| Queries | 696 |
| Control cycles | 5 |
| Worker loss | Exercised intentionally |
| Query-result failures | 0 |
| Median latency | 265 ms |
| P95 latency | 515 ms |

The harness compared each completed result with DuckDB. The following checks
passed: exact query results, cancellation and queue handling, intentional
worker-loss handling, duration, bounded history, retained-file cleanup,
monitor responsiveness, bounded RSS growth and peak RSS, and surviving-worker
health. The complete machine-readable report is retained at
`tmp/soak-120s-trino-parity/report.json` in the qualification workspace.

This evidence does not close the AKS gate. It does not exercise Azure storage,
TLS, network policy, node drain, image rollout, or cloud resource limits. The
deployed multi-worker qualification remains required before claiming Track 1
complete.
