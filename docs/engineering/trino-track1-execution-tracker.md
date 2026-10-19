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
| 1 | Worker discovery and scheduling | **Complete (bounded)** | Deterministic split enumeration, task assignment, leases, retry/steal, heartbeat expiry, stale-attempt protection, and resource-group admission exist. A split retry replays the exact failed batch instead of consuming newly enumerated work, expired leases are requeued once in partition order, and a takeover is accepted only after expiry while incrementing the attempt fence. Coordinator assignments now carry exact local Parquet and pinned Delta split descriptors; the focused qualification covers scheduler lifecycle tests and the existing orchestrator tests (`docs/qualification/track1-scheduler-split-assignment-2026-10-18.md`, `docs/qualification/track1-scheduler-lease-lifecycle-2026-10-18.md`). | Reopen for additional topologies or endurance evidence; the declared three-worker topology is qualified. Remote/cloud split enumeration and direct worker consumption of assignment split metadata remain separate gates. |
| 2 | Hash exchange and repartition | **Qualified (bounded)** | Authenticated Arrow exchange, checksums, streamed output, bounded replayable spools, cleanup, deterministic multi-column partitioning, live quota rejection, cancellation cleanup, a 900-second ADLS soak, and current-image AKS object-store fault recovery exist. | Longer mixed-workload endurance, broader topologies, and cross-region/object-store qualification. |
| 3 | Distributed joins | **Active** | Partitioned and broadcast hash joins, residuals, outer joins, semi/anti joins, dictionary-text keys, composite `USING` keys, measured local join spill, fail-closed spill-device quota handling with cleanup coverage, and bounded AKS outer-join/skew evidence exist. The current image now also has exact-repeat forced join-spill, post-query cleanup, exact retry after forced worker loss during a distributed scan, a bounded high-cardinality 504M-row self-join, exact join results in the ten-round mixed workload, and a current-image three-worker join-gates run covering left outer, full outer cross-source, and skewed grouped joins. | bounded AKS spill-quota exhaustion, larger input volumes, physical disk exhaustion, and repeated/node-loss endurance. |
| 4 | Distributed aggregates | Open | Partial/final aggregation, exact distinct, AVG, grouped spill, adaptive partials, and hybrid final merge exist. The current AKS image also has exact-repeat 10,000-group final-merge spill and cleanup evidence under a 4 MiB budget, plus five exact-repeat full-table grouped aggregates over 504M rows across all three workers; the current-image sample observed p95 26.453s. The differential corpus now includes a deliberately skewed grouped aggregate with exact local/distributed results across all parquet layouts. | Skew behavior under sustained pressure, mixed-workload spill evidence, and a larger statistical sample for an SLO. |
| 5 | Windows | Open | Distributed row-number and compatible partitioned windows are covered; the current image has exact-repeat partitioned `ROW_NUMBER()`/`QUALIFY` evidence across three workers, alongside the existing mixed-key evidence. The planner now shares one canonical hash exchange for multiple ordered window expressions with the same partition keys, including a bounded `ROWS` frame; the differential corpus checks exact local/distributed results for that shape across parquet layouts. The differential corpus also covers an unpartitioned `ROW_NUMBER()` result against every parquet layout and the distributed fragment path. Several ranking and value-window functions are implemented. A fail-closed planner regression now keeps distinct relation-qualified keys with the same leaf name on the centralized path, preventing an exchange from silently dropping one key (`docs/qualification/track1-window-qualified-key-guard-2026-10-18.md`). Partitioned `SMALLINT` finite `RANGE` frames now use exact widened boundary arithmetic and have exact local/three-worker evidence across both directions, NULL orders, and preceding/following bounds (`docs/qualification/track1-window-smallint-range-distributed-2026-10-18.md`). Partitioned `TINYINT` finite `RANGE` frames now follow the same exact widened boundary contract, with local/three-worker counts covering type extremes, NULL peers, both directions, and preceding/following bounds (`docs/qualification/track1-window-tinyint-range-distributed-2026-10-18.md`). Existing Decimal128 finite `RANGE` frames now have exact local/three-worker counts at precisions/scales `(38,6)` and `(20,2)`, covering signed adjacent high-magnitude values, NULL peers, both directions/NULL orders, and preceding/following bounds (`docs/qualification/track1-window-decimal-range-distributed-2026-10-18.md`). | Qualify mixed-key, unpartitioned, modifier, and remaining parallel window shapes; larger frame and cloud qualification remain open. |
| 6 | Global ORDER BY and TopN | Open | Global merge, multi-column ordering, null ordering, ordinal keys, OFFSET-aware TopN, and ordered-limit correctness are implemented. Already-sorted worker candidates now use a bounded k-way merge for integer, non-integer, and multi-key orderings without concatenating payload columns; the unsorted path retains the full Arrow fallback. The typed selection and merge fast paths now cover narrow `INT`/`BIGINT` and `UINT`/`UBIGINT` keys, preserving Arrow ordering and null semantics while avoiding the general lexsort path for common aggregate keys (local parity evidence: `docs/qualification/track1-global-topn-narrow-integer-2026-10-18.md`). The multi-key non-null integer merge now selects worker heads with a bounded heap, preserving source-order ties and reducing head comparisons to O(K log P); Arrow parity and 3/17/40-worker fragment/exchange evidence are retained in `docs/qualification/track1-global-topn-multi-integer-heap-2026-10-18.md`. The same typed heap now accepts non-null `Decimal128` keys, including multiple scales in distinct key columns and mixed integer keys, while nullable and unsorted inputs retain Arrow semantics; exact Arrow and 3/17/40-worker evidence is retained in `docs/qualification/track1-global-topn-multi-decimal-heap-2026-10-18.md`. The typed multi-key heap also accepts UTF-8 keys with shared Arrow string buffers, binary ordering, mixed integer/Decimal128 keys, and source-order ties; nullable and unsorted candidates retain Arrow fallback. Exact Arrow and 3/17/40-worker fragment/exchange evidence is retained in `docs/qualification/track1-global-topn-multi-utf8-heap-2026-10-18.md`. The current image has exact-repeat three-worker evidence for multi-column ordering with `NULLS LAST`, OFFSET, and LIMIT. | Demonstrate a parallel ordered result stream and pressure behavior without a coordinator bottleneck. |
| 7 | Memory admission and spill | Open | Per-query reservations, FIFO admission, resource groups, bounded spill, cancellation cleanup, telemetry, periodic result-cache expiry cleanup, ten exact concurrent mixed-workload rounds on the current image with resource samples and zero retained spill files, and a forced-worker-loss pressure run with zero file growth exist. | Skew, physical disk exhaustion, and final-merge spill under sustained load. |
| 8 | Cancellation and retry | Open | Queued/running cancellation, retry rotation, timeouts, orphan-task cleanup, and idempotent exchange handling exist. | Long-running federated cancellation and concurrent cancellation stress on the release image. |
| 9 | Worker-loss recovery | **Qualified (bounded)** | Replacement-worker recovery preserves exact results for the declared bounded gate; the current image also passes a forced worker loss during a distributed scan with exact retry, replacement, cleanup, and post-recovery probe. A graceful node-drain run also returned the exact result and restored three catalog-compatible workers, but Kubernetes evicted the pod before a retry was observable. | Node-drain interruption/retry and repeated-loss endurance with retained exact-result evidence. |
| 10 | Coordinator restart | **Complete (bounded)** | Idle and active-query cleanup is fail-closed, exchange spools are reclaimed, workers reregister, and exact post-restart probes pass. The current AKS image also passed an observed active-query coordinator restart with fail-closed interruption, three-worker recovery, exact post-restart probe, and no retained-file growth. | Multi-coordinator/failover continuity remains a separate future qualification. |
| 11 | Snapshot consistency | Open | Catalog and Delta versions are pinned across statement fragments and retries. | Cross-coordinator recovery and concurrent source-version qualification. |
| 12 | Runtime dynamic filtering | Open | Build-side runtime filters reduce probe work for supported joins; object Delta sources retain pinned add-action statistics, Iceberg sources retain manifest facts, and ADLS single-object readers defer lane/decoder startup until the first batch so late predicates reach file, row-group, and lane construction. Focused exact-result regressions cover the storage paths. Timestamp keys reach local Parquet row-group and row-filter evaluation; single-key timestamp joins now use a native `i64` index and exact storage `IN` literals for all four units, with unzoned/UTC, duplicate, NULL, and integer-precision regressions in local and partitioned execution (`docs/qualification/track1-runtime-filter-timestamp-join-index-2026-10-18.md`). Decimal128 keys now accept precision-38 build literals against narrower same-scale probe columns without changing their exact unscaled values (`docs/qualification/track1-runtime-filter-decimal-2026-10-18.md`). Time32/Time64 keys now use the exact integer-to-column path, with local Time64 Parquet evidence in `docs/qualification/track1-runtime-filter-time64-2026-10-18.md`. Late runtime filters now mask unread batches of an already-running local Parquet decoder and retain row-group pruning for unopened directory files, with exact local/partitioned join and scan evidence (`docs/qualification/track1-runtime-filter-late-parquet-2026-10-18.md`). The deployed image has an exact-repeat three-worker runtime-filter-eligible Delta join probe. Query scan telemetry records three accepted filters and four values per run, with exact repeat. The probe dataset did not reduce files or row groups, so this remains implementation and acceptance evidence rather than a cloud pruning qualification. | Qualify a representative cloud dataset whose partition/file or row-group statistics demonstrate measurable pruning, then repeat the timestamp and Decimal128 edges on the deployed image and cover Decimal256/scale conversion, remaining type/exchange edges, and cloud snapshot qualification. |
| 13 | Join reordering and skew mitigation | Open | Statistics-aware inner-join reordering and bounded broadcast selection exist. The differential corpus now includes an exact three-way join that exercises the reorderable inner-join path across local and distributed layouts. Join spill telemetry also records total rows and a bounded max-to-mean skew ratio for future policy qualification (`docs/qualification/track1-join-skew-telemetry-2026-10-18.md`). Bounded skew splitting now selects the largest encoded-byte bucket independently of row count; exact local/partitioned inner/full join results, paired routing, and cleanup are qualified in `docs/qualification/track1-join-byte-skew-candidate-2026-10-18.md`. | Cloud/pressure qualification of skew splitting, multi-join cost policy, and a larger plan-regression corpus. |
| 14 | Connector federation | Open | Native Parquet, Delta, and Iceberg paths exist; registered SQL sources use passthrough API paths. | Authenticated pushdown and cross-source execution qualification. |
| 15 | Upgrade and rollback | Open | Deployment artifacts and restart cleanup paths exist. | Rehearse upgrade, rollback, and state compatibility on the release image. |
| 16 | Sustained distributed soak | Open | Focused correctness and failure tests plus a 604.001-second ten-round, 40-query concurrent mixed-workload run, a current-image 900-second three-client soak with 1,386/1,386 exact requests, a forced-worker-loss pressure run, and a graceful node-drain observation with exact results, resource samples, and zero retained spill exist. | Longer endurance, node-loss during the soak, broader topology, and retry/failure qualification. |

The full-table aggregate evidence is retained in `docs/qualification/track1-aks-scale-aggregate-2026-10-15.md` and its raw JSON: two identical 100-row result sets after grouping all 504,000,000 source rows, across three stages, seven tasks, and all three workers. It strengthens item 4 correctness evidence; p95, skew, and sustained mixed-workload gates remain open.

The latest five-run aggregate p95 sample is retained in `docs/qualification/track1-aks-scale-aggregate-5x-2026-10-15.md` and its raw JSON: five exact result hashes, an observed 22.578s p95, three stages, seven tasks, and all three workers. It is workload evidence, not a universal latency SLO.

The current-image five-run aggregate sample is retained in
`docs/qualification/track1-aks-scale-aggregate-5x-current-2026-10-16.md` and
its raw JSON. Five full-table grouped aggregates over 504M rows finished with
the same exact result across three workers; the observed five-sample p95 was
26.453s. It is workload evidence, not a universal latency SLO. Skew,
sustained mixed-workload pressure, physical disk exhaustion, and larger
topologies remain open.

The bounded mixed-workload report is retained in `docs/qualification/track1-aks-mixed-workload-2026-10-15.md` and its raw JSON. Three concurrent rounds combined aggregate, join, window, and global-order queries; all results matched sequential baselines, all workers participated, and no spill files remained. Longer endurance remains open.

The bounded AKS spill-quota exhaustion report is retained in `docs/qualification/track1-aks-join-spill-exhaustion-2026-10-15.md` and its raw JSON. Two low-quota runs failed closed with `spill limit exceeded`; restoring the 4 GiB quota yielded three Ready workers, zero retained spill files, and an exact post-restore join. Physical node-disk-full behavior remains open.

The five-minute endurance report is retained in `docs/qualification/track1-aks-mixed-workload-6round-2026-10-15.md` and its raw JSON. Six concurrent rounds ran for 350.584 seconds with 24/24 exact requests, six resource samples, and zero retained worker spill files. Node-loss during the soak and longer endurance remain open.

The current-image ten-round report is retained in
`docs/qualification/track1-aks-mixed-workload-10round-2026-10-16.md` and its
raw JSON. Ten concurrent rounds ran for 604.001 seconds with 40/40 exact
requests, ten resource samples, and zero retained worker spill files. It
supersedes the six-round timing for current-image evidence; node-loss during
the soak, longer endurance, and broader topology qualification remain open.

The current-image worker-loss mixed-pressure report is retained in
`docs/qualification/track1-aks-worker-loss-mixed-2026-10-16.md` and its raw
JSON. One worker was force-terminated during a distributed scan; the retry
returned the exact baseline, the replacement rejoined with the pinned catalog
snapshot, eight pressure requests were exact, and no exchange or spill files
remained. Repeated loss, node drain during a long soak, and physical disk
exhaustion remain open.

The graceful node-drain observation is retained in
`docs/qualification/track1-aks-node-drain-graceful-2026-10-16.md` and its raw
JSON. The active scan returned the exact baseline and the replacement worker
rejoined with the pinned snapshot, but eviction completed before a retry could
be observed; it is intentionally not counted as a retry qualification.

The current-image join-gates report is retained in
`docs/qualification/track1-aks-join-gates-current-2026-10-16.md` and its raw
JSON. Left outer, full outer cross-source, and skewed grouped joins each
finished twice with identical results across three catalog-compatible workers.
This strengthens the bounded join evidence; disk exhaustion, larger inputs,
and repeated node-loss endurance remain open.

The current-image object-store fault report is retained in
`docs/qualification/track1-aks-storage-fault-current-2026-10-16.md` and its raw
JSON. A scoped ADLS egress fault produced an observed stage retry, the exact
504M-row result, and zero retained exchange or spill files after cleanup.
Cross-region storage and physical disk exhaustion remain open.

The current-image coordinator-loss report is retained in
`docs/qualification/track1-aks-coordinator-loss-current-2026-10-16.md` and its
raw JSON. An active query was interrupted during coordinator restart, cleanup
was fail-closed, all three workers returned, the exact probe passed, and no
retained exchange or spill growth was observed. Multi-coordinator continuity
remains outside this bounded gate.

The current-image three-client soak is retained in
`docs/qualification/track1-aks-current-soak-900-c3-2026-10-16.md` and its raw
JSON. It ran for 900 seconds with 1,386/1,386 exact requests, zero failures,
zero unexpected restarts, all three workers Ready, and no retained exchange or
spill growth. Node loss during the soak, physical disk exhaustion, and broader
topologies remain open.

The current-image partitioned window report is retained in
`docs/qualification/track1-aks-window-current-2026-10-16.md` and its raw JSON.
The bounded `ROW_NUMBER()` plus `QUALIFY` query finished twice with the exact
same 12-row result across three workers. Unpartitioned windows, broader
modifiers, and full parallel-window qualification remain open.

The current-image global-order report is retained in
`docs/qualification/track1-aks-global-order-current-2026-10-16.md` and its
raw JSON. A multi-column `NULLS LAST` query with OFFSET and LIMIT finished
twice with the exact same 20-row result across three workers. Fully parallel
ordered streaming and pressure qualification remain open.

The current-image runtime-filter-eligible join probe is retained in
`docs/qualification/track1-aks-runtime-filter-join-current-2026-10-16.md` and
its raw JSON. It finished twice with identical results across three workers.
The deployed scan telemetry records three accepted runtime filters carrying
four values per run. The probe did not reduce files or row groups, so storage-
pruning and cloud snapshot gates remain open.

## Active item: 3 — distributed joins

The next evidence package must include distributed outer-join equivalence,
skew behavior, disk-exhaustion handling, measured join-spill behavior,
commands, raw results, and a short qualification report under
`docs/qualification/`.

The worker-discovery gate is now complete for the declared three-worker
topology. The qualification package includes Docker and AKS scheduling runs,
forced pod-loss retry, grouped skew/failure injection, repeated pressure, and
graceful node-drain observation. Item 2 is now qualified for the declared
three-worker bounded gate; item 3 is the active work item.

The bounded AKS join qualification is retained in
`docs/qualification/track1-aks-join-gates-2026-10-15.md` and its raw JSON
report. Left outer, full outer cross-source, and skewed grouped joins all
finished twice with identical results across the three workers. Disk
bounded forced join spill, one-worker-loss recovery, and a bounded high-cardinality 504M-row self-join are now qualified; physical node-disk-full behavior, larger input volumes, and sustained mixed-workload endurance remain the next item-3 gates.

Current exchange evidence includes the scheduler and stage-runtime suites,
three-node Docker and AKS runs, exact multi-worker exchange, cancellation
cleanup, worker replacement, grouped skew/failure injection, repeated pressure,
and graceful node-drain observation. These reports qualify worker discovery and
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
The bounded high-cardinality self-join is retained in `docs/qualification/track1-aks-high-cardinality-join-2026-10-15.md` and its raw JSON; it finished twice with identical six-row results across five stages, 13 tasks, and all three workers. Larger input volumes, disk exhaustion, and sustained mixed-workload endurance remain open.

The clean-baseline rerun is retained in `docs/qualification/track1-aks-current-image-fault-pressure-2026-10-15-rerun.md` and its raw JSON. All seven checks passed on Engine image `sha256:0ea4244c392267f5d821935cb63f4c2a251c30be2f44a33554b340e1eb63a1f6`, including zero retained files before and after the run. This closes the earlier pre-existing-file precondition gap; larger high-cardinality joins, disk exhaustion, and sustained mixed-workload endurance remain open.
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

The current complete workspace suite is recorded in
`docs/qualification/track1-track2-workspace-suite-current-2026-10-16.md`: 1,539
tests listed, zero failures, and six intentional/benchmark ignores on the
release source (including the JSON object-constructor regression). The
earlier run remains in `track1-track2-workspace-suite-2026-10-15.md`. This strengthens the local
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











