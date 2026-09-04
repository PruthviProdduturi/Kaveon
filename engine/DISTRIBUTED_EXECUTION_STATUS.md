# Distributed Execution Status

This file is the durable handoff for Kaveon's path from the current alpha executor to a fault-tolerant distributed analytical engine. It records verified behavior, incomplete work, and the next safe continuation point. Update it with every distributed-execution commit.

## Current baseline

- Branch: `dev`
- Execution unit: Arrow `RecordBatch`
- Storage partitioning: deterministic Parquet row-group and Delta active-file partitions
- Coordinator/worker transport: HTTP task submission and Arrow IPC task results
- Shipped distributed query shape: mergeable `COUNT`, `SUM`, `MIN`, and `MAX` grouped aggregates over a single partitioned scan
- Shipped local query shapes: filters, projections, aggregates, sort/TopN, and hash joins
- Existing exchange primitives: deterministic multi-column hash partitioning and a byte-bounded in-process exchange buffer

This is a functioning distributed slice, not yet a Trino-class distributed runtime. The missing capabilities below remain explicit release gates.

## Eight-workstream program

| # | Workstream | State | Verified scope | Remaining release gate |
|---|---|---|---|---|
| 1 | Network hash exchange | In progress | Arrow IPC transport; hash partitioner; bounded buffer; authenticated idempotent POST/GET/DELETE exchange endpoints | Wire fragment producers/consumers to the endpoints, streaming flow control, and end-to-end repartition tests |
| 2 | Stage and fragment planner | In progress | Validated DAG builder plus dependency-gated runtime state machine, retries, terminal cascade, and exchange-cleanup intents | Connect task assignments to worker transport and execute fragments instead of query-shape branches |
| 3 | Multi-stage aggregation | In progress | Distributed count/sum/min/max; versioned Arrow state encoding covers weighted AVG and typed exact distinct | Send encoded states through exchange and execute repartitioned final aggregation with equivalence tests |
| 4 | Distributed TopN | Implemented | Local partial TopN per scan partition followed by an Arrow-native coordinator final merge; multi-column direction/null and schema validation tests | Docker correctness/performance evidence and migration into the general fragment scheduler |
| 5 | Distributed hash joins | Not started | Correct local inner/outer/cross hash joins; hash partitioner exists | Co-partition both inputs, exchange by join keys, build/probe tasks, broadcast threshold, skew handling, outer-join correctness |
| 6 | Memory accounting and spill | In progress | Thread-safe memory limits; bounded Arrow spill runs; opt-in spill-aware Sort/TopN accumulation | Add streaming k-way run merge, then wire aggregate/join/exchange, admission/revocation, and telemetry |
| 7 | Failure, retry, cancellation | In progress | Attempt identity, retry/timeouts, bounded idempotent task registry, owner/waiter replay, cancellation tokens, and cleanup contracts | Wire lifecycle into HTTP execution, propagate cancellation across workers, and run worker-loss tests |
| 8 | Scheduler maturity | In progress | Active-worker discovery, retry rotation, dynamic split leases independent of worker count, failed-lease requeue | Wire storage split enumeration, worker pull/steal, skew mitigation, queues, admission, and resource groups |

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
- `cargo test --workspace --no-fail-fast`: 100 passed, 0 failed
- Focused coverage includes stage-graph validation, task/split identity, weighted AVG state merge, exact distinct set union, exchange chunk corruption and bounds, strict bearer validation, and alternate-worker retry selection
- Docker worker-loss, end-to-end exchange, distributed AVG/distinct, TopN, join, spill, and scheduler stress evidence: not yet complete

The second integrated gate passed 69 focused tests across core, exec, and server and 112 tests across the full workspace after distributed TopN, authenticated exchange endpoints, memory reservations, and split leasing were added. Strict Clippy passed; Windows emitted filesystem-only incremental-cache hard-link warnings and copied the files instead.

The third integrated gate passed 85 focused tests across core, exec, and server and 128 tests across the full workspace after aggregate-state wire encoding, the stage DAG builder, spill-run infrastructure, and task timeout/retry classification were added. Strict Clippy and formatting passed. The Windows incremental cache again used file copies when hard links were unavailable; this is an environment warning, not a Rust lint.

The fourth gate passed 96 focused tests across core, exec, and server and 139 tests across the full workspace after the stage runtime, idempotent task/cancellation lifecycle, and spill-aware Sort/TopN paths were added. Strict Clippy and formatting passed. Spill accumulation is bounded, but final run merging is still eager and is not yet a fully bounded external sort.

## Continuation point

Complete the current round in this order:

1. Build the stage executor and connect graph outputs/inputs through authenticated exchange endpoints.
2. Replace aggregate-specific fan-out and send AVG/exact-distinct states through the exchange.
3. Execute repartitioned and broadcast hash-join graphs with local/distributed equivalence tests.
4. Wire memory reservations and spill runs into aggregate, join, sort, TopN, and exchange.
5. Add the idempotent worker task registry, query cancellation propagation, and exchange cleanup.
6. Connect storage split enumeration to worker pull/steal, skew mitigation, and admission control.
7. Exercise correctness, worker loss, memory pressure, concurrency, and performance under local Docker.
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
