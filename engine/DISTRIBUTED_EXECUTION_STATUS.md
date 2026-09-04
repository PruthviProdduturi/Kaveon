# Distributed Execution Status

This file is the durable handoff for Kaveon's path from the current alpha executor to a fault-tolerant distributed analytical engine. It records verified behavior, incomplete work, and the next safe continuation point. Update it with every distributed-execution commit.

## Current baseline

- Branch: `dev`
- Execution unit: Arrow `RecordBatch`
- Storage partitioning: deterministic Parquet row-group and Delta active-file partitions
- Coordinator/worker transport: HTTP task submission and Arrow IPC task results
- Shipped distributed query shapes: scan/filter/project, partial/final grouped and global aggregates, Sort/TopN/limit, and repartitioned or broadcast hash joins
- Shipped local query shapes: filters, projections, aggregates, sort/TopN, and hash joins
- Existing exchange primitives: deterministic multi-column hash partitioning and a byte-bounded in-process exchange buffer

This is a functioning distributed slice, not yet a Trino-class distributed runtime. The missing capabilities below remain explicit release gates.

## Eight-workstream program

| # | Workstream | State | Verified scope | Remaining release gate |
|---|---|---|---|---|
| 1 | Network hash exchange | Implemented locally | Exchange-ID-safe Arrow IPC v2; authenticated idempotent endpoints; fragment producer/consumer wiring; 512 MiB process payload bound; cleanup | Streaming flow control and Docker/AKS end-to-end pressure evidence |
| 2 | Stage and fragment planner | Implemented locally | Validated DAG/runtime; deterministic executable fragments; authenticated coordinator/worker dispatch | Docker worker-loss and multi-query stress evidence |
| 3 | Multi-stage aggregation | Implemented locally | Partial/final COUNT/SUM/MIN/MAX/weighted AVG/exact DISTINCT; typed/null keys; empty-global semantics | Docker equivalence and performance evidence; aggregate spill |
| 4 | Distributed TopN | Implemented locally | General fragment scheduler; multi-column direction/null ordering; partial/final TopN; fixed-fan-in spill merge | Docker correctness/performance evidence |
| 5 | Distributed hash joins | Implemented locally | Hash repartition for equi-joins; broadcast cross-join build; local inner/outer/cross semantics | Distributed outer-join equivalence, broadcast threshold, skew handling, and join spill |
| 6 | Memory accounting and spill | In progress | Hard reservations; bounded exchange storage; fixed-fan-in multi-pass lazy Sort/TopN spill merge | Aggregate/join spill, admission/revocation, and operator telemetry |
| 7 | Failure, retry, cancellation | Implemented locally | Idempotent replay, authenticated dispatch/control, retry rotation, cancellation, failed-attempt and consumed-exchange cleanup | Docker worker-loss and concurrent cancellation stress evidence |
| 8 | Scheduler maturity | In progress | Deterministic Parquet row-group and Delta-file splits; exact-attempt lease/requeue/steal; stale-attempt protection | Connect enumerated splits to coordinator task assignments, admission/resource groups, and broader skew mitigation |

## Correctness and performance gates

Every workstream must land with:

1. Unit tests for contracts and failure boundaries.
2. Cross-crate integration tests for each supported distributed query shape.
3. `cargo fmt --check`.
4. `cargo clippy --workspace --all-targets -- -D warnings`.
5. `cargo test --workspace`.
6. A clean two-worker Docker run over mounted local Parquet/Delta data.
7. Correctness comparison against the local executor for identical SQL and data.
8. Criterion or repeatable release-build measurements that report data volume, rows, worker count, warmup, sample count, and memory. Single runs are never published as benchmark claims.

### Latest verification — 2026-09-04

- `cargo fmt --check`: passed
- Strict Clippy for `kaveon-core`, `kaveon-exec`, and `kaveon-server`, including all targets: passed
- `cargo test --workspace --no-fail-fast`: 164 passed, 0 failed
- `cargo clippy --workspace --all-targets -- -D warnings`: passed
- Focused coverage includes graph/fragment agreement, partition-correct scans, weighted AVG and exact distinct partial/final execution, empty Arrow schemas, exchange identity/corruption/byte bounds, broadcast routing, fixed-fan-in spill compaction, retry cleanup, and deterministic split steal/requeue
- Docker worker-loss, end-to-end exchange, distributed AVG/distinct, TopN, join, spill, and scheduler stress evidence: not yet complete

The second integrated gate passed 69 focused tests across core, exec, and server and 112 tests across the full workspace after distributed TopN, authenticated exchange endpoints, memory reservations, and split leasing were added. Strict Clippy passed; Windows emitted filesystem-only incremental-cache hard-link warnings and copied the files instead.

The third integrated gate passed 85 focused tests across core, exec, and server and 128 tests across the full workspace after aggregate-state wire encoding, the stage DAG builder, spill-run infrastructure, and task timeout/retry classification were added. Strict Clippy and formatting passed. The Windows incremental cache again used file copies when hard links were unavailable; this is an environment warning, not a Rust lint.

The fourth gate passed 96 focused tests across core, exec, and server and 139 tests across the full workspace after the stage runtime, idempotent task/cancellation lifecycle, and spill-aware Sort/TopN paths were added. Strict Clippy and formatting passed. Spill accumulation is bounded, but final run merging is still eager and is not yet a fully bounded external sort.

The fifth gate passed 106 focused tests across core, exec, and server and 149 tests across the full workspace after executable fragments, canonical grouped aggregate states, lazy spill merging, HTTP lifecycle integration, and terminal cleanup were added. Strict Clippy and formatting passed. Spill merge retains one cursor batch per run, so fixed fan-in/multi-pass compaction remains required for a strict constant ceiling.

The sixth gate passed 164 tests across the full workspace after general coordinator/worker fragment execution, exchange v2 routing, distributed partial/final aggregates and joins, fixed-fan-in spill compaction, and local split enumeration were added. Strict workspace Clippy and formatting passed. Docker Compose configuration validates, but container startup, worker-loss, concurrency, and comparative performance evidence have not yet been run for this gate.

## Continuation point

Complete the current round in this order:

1. Run local-vs-distributed correctness for aggregate, TopN, equi-join, and cross-join through the two-worker Docker stack.
2. Exercise worker loss, retry, cancellation, stale attempts, and exchange cleanup under Docker.
3. Connect deterministic storage split enumeration to fragment task assignments instead of worker-count partitions.
4. Add aggregate and join memory reservations, spill, and revocation.
5. Add exchange streaming flow control and bounded consumer-side fetching.
6. Add admission queues/resource groups and concurrency/skew stress tests.
7. Record repeatable release-build performance and memory evidence; do not publish single-run claims.
8. Add ADLS Gen2 range reads, then repeat the suite on a minimum five-worker AKS cluster.

## Machine-to-machine handoff

On the next machine:

```powershell
git switch dev
git pull origin dev
Get-Content HANDSHAKE.md
Get-Content engine/DISTRIBUTED_EXECUTION_STATUS.md
cargo test --workspace --manifest-path engine/Cargo.toml
```

Set `KAVEON_DATA_PATH` to that machine's local data directory before Docker validation. Local drive letters are deployment configuration and must never be embedded in Engine plans, catalog contracts, or tests.
