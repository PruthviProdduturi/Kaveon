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
| 1 | Worker discovery and scheduling | **Complete (bounded)** | Deterministic split enumeration, task assignment, leases, retry/steal, stale-attempt protection, and resource-group admission exist. | Reopen for additional topologies or endurance evidence; the declared three-worker topology is qualified. |
| 2 | Hash exchange and repartition | **Qualified (bounded)** | Authenticated Arrow exchange, checksums, streamed output, bounded replayable spools, cleanup, deterministic multi-column partitioning, live quota rejection, cancellation cleanup, a 900-second ADLS soak, and AKS object-store fault recovery exist. | Longer mixed-workload endurance, broader topologies, and cross-region/object-store qualification. |
| 3 | Distributed joins | **Active** | Partitioned and broadcast hash joins, residuals, outer joins, semi/anti joins, dictionary-text keys, composite `USING` keys, measured local join spill, and bounded AKS outer-join/skew evidence exist. The current AKS image now also has exact-repeat forced join-spill, post-query cleanup, and one-worker-loss retry evidence under a 1 MiB budget. | AKS disk exhaustion, larger high-cardinality joins, and sustained mixed-workload endurance. |
| 4 | Distributed aggregates | Open | Partial/final aggregation, exact distinct, AVG, grouped spill, adaptive partials, and hybrid final merge exist. The current AKS image also has exact-repeat 10,000-group final-merge spill and cleanup evidence under a 4 MiB budget. | Larger-scale cluster run with p95 latency, skew behavior, and final-merge spill evidence. |
| 5 | Windows | Open | Distributed row-number and compatible partitioned windows are covered; several ranking and value-window functions are implemented. | Qualify mixed-key, unpartitioned, frame, modifier, and remaining parallel window shapes. |
| 6 | Global ORDER BY and TopN | Open | Global merge, multi-column ordering, null ordering, ordinal keys, OFFSET-aware TopN, and ordered-limit correctness are implemented. | Demonstrate a parallel ordered result stream and pressure behavior without a coordinator bottleneck. |
| 7 | Memory admission and spill | Open | Per-query reservations, FIFO admission, resource groups, bounded spill, cancellation cleanup, and telemetry exist. | Five-minute mixed workload, skew, disk exhaustion, and final-merge spill evidence. |
| 8 | Cancellation and retry | Open | Queued/running cancellation, retry rotation, timeouts, orphan-task cleanup, and idempotent exchange handling exist. | Long-running federated cancellation and concurrent cancellation stress on the release image. |
| 9 | Worker-loss recovery | Open | Replacement-worker recovery preserves exact results for the declared bounded gate. | Node-drain and repeated-loss endurance with retained exact-result evidence. |
| 10 | Coordinator restart | **Complete (bounded)** | Idle and active-query cleanup is fail-closed, exchange spools are reclaimed, workers reregister, and exact post-restart probes pass. | Multi-coordinator/failover continuity remains a separate future qualification. |
| 11 | Snapshot consistency | Open | Catalog and Delta versions are pinned across statement fragments and retries. | Cross-coordinator recovery and concurrent source-version qualification. |
| 12 | Runtime dynamic filtering | Open | Build-side runtime filters reduce probe work for supported joins. | Inject filters into Parquet/Delta scan pruning with exact-result and exchange tests. |
| 13 | Join reordering and skew mitigation | Open | Statistics-aware inner-join reordering and bounded broadcast selection exist. | Skew splitting, multi-join cost policy, and plan-regression corpus. |
| 14 | Connector federation | Open | Native Parquet, Delta, and Iceberg paths exist; registered SQL sources use passthrough API paths. | Authenticated pushdown and cross-source execution qualification. |
| 15 | Upgrade and rollback | Open | Deployment artifacts and restart cleanup paths exist. | Rehearse upgrade, rollback, and state compatibility on the release image. |
| 16 | Sustained distributed soak | Open | Focused correctness and failure tests exist. | Mixed workload soak with throughput, p95 latency, memory, spill, retry, and failure counts retained. |

## Active item: 3 — distributed joins

The next evidence package must include distributed outer-join equivalence,
skew behavior, disk-exhaustion handling, measured join-spill behavior,
commands, raw results, and a short qualification report under
`docs/qualification/`.

The worker-discovery gate is now complete for the declared three-worker
topology. The qualification package includes Docker and AKS scheduling runs,
forced pod-loss retry, grouped skew/failure injection, repeated pressure, and
graceful node-drain retry. Item 2 is now qualified for the declared
three-worker bounded gate; item 3 is the active work item.

The bounded AKS join qualification is retained in
`docs/qualification/track1-aks-join-gates-2026-10-15.md` and its raw JSON
report. Left outer, full outer cross-source, and skewed grouped joins all
finished twice with identical results across the three workers. Disk
exhaustion and forced join-spill recovery remain the next item-3 gate.

Current exchange evidence includes the scheduler and stage-runtime suites,
three-node Docker and AKS runs, exact multi-worker exchange, cancellation
cleanup, worker replacement, grouped skew/failure injection, repeated pressure,
and graceful node-drain retry. These reports qualify worker discovery and
scheduling for the declared three-worker topology. The broader topology and
endurance work remains open.

The bounded ADLS-backed soak
`docs/qualification/track1-aks-exchange-soak-2026-10-15.md` completed 416/416
exact requests over 300 seconds with four-way concurrency, zero unexpected
restarts, and zero retained exchange/spill files. The current immutable image
also passed a 900-second ADLS-backed soak with two-way concurrency: 1,180/1,180
exact requests, zero failures, zero unexpected restarts, and zero retained
exchange/spill files. Evidence is retained in
`docs/qualification/track1-aks-current-image-soak-900-2026-10-15.md` and its
raw JSON report. Together with the fault, quota, and object-store reports
below, this closes the declared bounded exchange pressure gate.
The current-image fault/pressure qualification
`docs/qualification/track1-aks-current-image-fault-pressure-2026-10-15.md`
passed concurrent exact results, active-query cancellation, forced worker-loss
retry, exact recovery, bounded grouped pressure, memory limits, and zero
retained exchange/spill growth on the deployed immutable image. A graceful
node-drain attempt restored the worker and exact result but did not expose a
retry attempt, so it is retained as negative evidence rather than counted as a
retry gate. The live quota probe
`docs/qualification/track1-aks-exchange-quota-2026-10-15.md` rejected a
high-cardinality exchange with HTTP 507 and retained zero exchange files after
restoration of the production limits.

The complete workspace suite is recorded in
`docs/qualification/track1-track2-workspace-suite-2026-10-15.md`: 1,457 tests
passed with zero failures on the release source. This strengthens the local
implementation evidence but does not substitute for the remaining distributed
join, aggregate, window, ordering, connector, upgrade, and full mixed-workload
endurance gates on AKS.

The explicit object-store fault and orphan-spool qualification is recorded in
`docs/qualification/track1-aks-storage-fault-2026-10-15.md` and its raw JSON
report. A temporary policy blocked public object-store traffic for one worker;
the baseline and fault runs returned the same exact result, a retry attempt was
observed, and no exchange or spill files remained afterward. The policy was
removed before the qualification completed.

The positive graceful node-drain result is retained in
`docs/qualification/track1-aks-node-drain-skew-2026-10-15.md`; repeated pod-loss
evidence is retained in
`docs/qualification/track1-aks-repeat-fault-2026-10-15.md`.
Coordinator interruption, startup cleanup, and exact recovery are retained in
`docs/qualification/track1-aks-coordinator-restart-2026-10-15.md`.

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
The local differential spill gate is also covered by
`a_join_under_a_refusing_budget_spills_and_matches_the_in_memory_join` and
the partitioned-join spill tests in the workspace suite. Those tests prove
exact local spill/replay and cleanup; they do not substitute for the AKS
high-cardinality and disk-pressure gate.





