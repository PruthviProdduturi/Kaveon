# Track 1 execution tracker

This tracker breaks the distributed-runtime work in the Trino parity matrix
into one auditable item per row. Work proceeds in order: only one item is
`Active` at a time. A `Qualified` item needs code, focused tests, and the
deployment or workload evidence named in its gate. This is a capability
snapshot, not a session diary.

## Status

- **Complete** — implementation and the stated evidence are present.
- **Active** — the only item currently being worked.
- **Open** — implementation exists or is partially present, but its gate is open.
- **Not started** — the capability or its qualification work is still missing.

## Ordered work

| # | Capability | Status | What is already true | Completion gate |
|---:|---|---|---|---|
| 1 | Worker discovery and scheduling | **Active** | Deterministic split enumeration, task assignment, leases, retry/steal, stale-attempt protection, and resource-group admission exist. | Run the same correctness corpus on each supported multi-worker topology and retain scheduling, retry, skew, and failure evidence. |
| 2 | Hash exchange and repartition | Open | Authenticated Arrow exchange, checksums, streamed output, bounded replayable spools, cleanup, and deterministic multi-column partitioning exist. | Sustained network and cloud-storage pressure, cancellation, quota, and orphan-spool evidence on the release image. |
| 3 | Distributed joins | Open | Partitioned and broadcast hash joins, residuals, outer joins, semi/anti joins, dictionary-text keys, composite `USING` keys, and bounded spill exist. | Distributed outer-join equivalence, skew, disk exhaustion, and measured join-spill qualification. |
| 4 | Distributed aggregates | Open | Partial/final aggregation, exact distinct, AVG, grouped spill, adaptive partials, and hybrid final merge exist. | Larger-scale cluster run with p95 latency, skew behavior, and final-merge spill evidence. |
| 5 | Windows | Open | Distributed row-number and compatible partitioned windows are covered; several ranking and value-window functions are implemented. | Qualify mixed-key, unpartitioned, frame, modifier, and remaining parallel window shapes. |
| 6 | Global ORDER BY and TopN | Open | Global merge, multi-column ordering, null ordering, ordinal keys, OFFSET-aware TopN, and ordered-limit correctness are implemented. | Demonstrate a parallel ordered result stream and pressure behavior without a coordinator bottleneck. |
| 7 | Memory admission and spill | Open | Per-query reservations, FIFO admission, resource groups, bounded spill, cancellation cleanup, and telemetry exist. | Five-minute mixed workload, skew, disk exhaustion, and final-merge spill evidence. |
| 8 | Cancellation and retry | Open | Queued/running cancellation, retry rotation, timeouts, orphan-task cleanup, and idempotent exchange handling exist. | Long-running federated cancellation and concurrent cancellation stress on the release image. |
| 9 | Worker-loss recovery | Open | Replacement-worker recovery preserves exact results for the declared bounded gate. | Node-drain and repeated-loss endurance with retained exact-result evidence. |
| 10 | Coordinator restart | Open | Idle and active-query cleanup is fail-closed and exchange spools are reclaimed. | Multi-coordinator or failover evidence with in-flight query continuity. |
| 11 | Snapshot consistency | Open | Catalog and Delta versions are pinned across statement fragments and retries. | Cross-coordinator recovery and concurrent source-version qualification. |
| 12 | Runtime dynamic filtering | Open | Build-side runtime filters reduce probe work for supported joins. | Inject filters into Parquet/Delta scan pruning with exact-result and exchange tests. |
| 13 | Join reordering and skew mitigation | Open | Statistics-aware inner-join reordering and bounded broadcast selection exist. | Skew splitting, multi-join cost policy, and plan-regression corpus. |
| 14 | Connector federation | Open | Native Parquet, Delta, and Iceberg paths exist; registered SQL sources use passthrough API paths. | Authenticated pushdown and cross-source execution qualification. |
| 15 | Upgrade and rollback | Open | Deployment artifacts and restart cleanup paths exist. | Rehearse upgrade, rollback, and state compatibility on the release image. |
| 16 | Sustained distributed soak | Open | Focused correctness and failure tests exist. | Mixed workload soak with throughput, p95 latency, memory, spill, retry, and failure counts retained. |

## Active item: 1 — worker discovery and scheduling

The next evidence package must include:

1. The supported worker topologies and their exact image/commit identifiers.
2. A repeated local or containerized correctness corpus against the local executor.
3. Assignment, lease, retry, requeue, and stale-attempt results for each topology.
4. Skew and cancellation observations, including cleanup of abandoned tasks.
5. Commands, raw results, and a short qualification report under
   `docs/qualification/`.

After that evidence is reviewed, change item 1 to `Complete`, move item 2 to
`Active`, and update this matrix in place.

Current evidence: the scheduler and stage-runtime focused suites pass (16
tests total) on the current commit. The Docker report
`docs/qualification/track1-worker-scheduling-docker-2026-10-14.md` records a
three-node Compose run, and the AKS report
`docs/qualification/track1-worker-scheduling-aks-2026-10-14.md` records the
production topology: three workers discovered with one catalog snapshot, a
three-way 504-million-row scan and exchange, a final merge, and exact platform
counts. These runs close the basic multi-worker assignment and stage-dependency
evidence. The bounded AKS fault-pressure report
`docs/qualification/engine-aks-fault-pressure-2026-10-14.md` additionally
records worker loss, replacement retry, cancellation cleanup, concurrent exact
results, and bounded pressure on the same three-worker image. A dedicated skew
qualification and the remaining stale-attempt/lease and failure-path evidence
are still required before item 1 can move to Complete. The rerun summary is
`docs/qualification/track1-aks-fault-pressure-rerun-2026-10-14.md`; it records
the cancellation-harness correction and a passing live result. The focused
scheduler gate report
`docs/qualification/track1-scheduler-gates-2026-10-14.md` records skew-tail
steal, retry, stale-attempt, and lease-return unit evidence; the live rerun
records worker loss and replacement retry. The live skew observation
`docs/qualification/track1-aks-skew-observation-2026-10-14.md` records a 29.3x
data imbalance with bounded worker timing and exact results; it does not claim
a general skew-splitting optimizer. Item 1 remains Active until a controlled
live skew/failure injection and repeated lease evidence are retained together
on the release image; the current reports define that final gate but do not
close it.

The newer live rerun
`docs/qualification/track1-aks-fault-pressure-2026-10-15.md` independently
passes the three-worker loss, retry, cancellation, concurrent exact-result,
and bounded-pressure checks on the current cluster. It strengthens the failure
evidence but does not close the separate skew-splitting and repeated-lease gate.

## Validation commands

Run from `engine/`:

```powershell
cargo test -p kaveon-optim --quiet
cargo test -p kaveon-server --bin kaveon-server differential_tests --quiet
cargo test --workspace --no-fail-fast --quiet
```

Run from the repository root:

```powershell
node scripts/validate-docs.mjs
```
