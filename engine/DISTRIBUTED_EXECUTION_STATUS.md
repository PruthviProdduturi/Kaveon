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
| 1 | Network hash exchange | In progress | Arrow IPC result transport; hash partitioner; bounded buffer; exchange envelope implementation under validation | Wire producer/consumer endpoints, streaming flow control, authentication, and end-to-end repartition tests |
| 2 | Stage and fragment planner | In progress | Shared stage/task/partition identities; general fragment contracts under validation | Translate physical plans into a dependency graph and schedule dependencies instead of query-shape branches |
| 3 | Multi-stage aggregation | In progress | Distributed merge for count/sum/min/max | Mergeable AVG and exact distinct state, repartitioned final aggregation, empty/null semantics, multi-stage integration |
| 4 | Distributed TopN | Not started | Correct vectorized local TopN | Local partial TopN per scan partition followed by a coordinator final TopN, with ordering/null equivalence tests |
| 5 | Distributed hash joins | Not started | Correct local inner/outer/cross hash joins; hash partitioner exists | Co-partition both inputs, exchange by join keys, build/probe tasks, broadcast threshold, skew handling, outer-join correctness |
| 6 | Memory accounting and spill | Not started | Byte-bounded in-process exchange buffer; process RSS telemetry | Per-query/operator reservations, admission, revocation, aggregate/join/sort spill, disk limits, spill telemetry |
| 7 | Failure, retry, cancellation | In progress | Task attempt is part of task identity; alternate-worker retry scheduler under validation | Idempotent task registry, query cancellation propagation, timeouts, retry classification, exchange cleanup, worker-loss tests |
| 8 | Scheduler maturity | Not started | Active-worker discovery and deterministic scan partition fan-out | Split enumeration independent of worker count, dynamic assignment, work stealing, skew mitigation, queues, resource groups |

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

### Latest verification — 2026-09-03

- `cargo fmt --check`: passed
- Strict Clippy for `kaveon-core`, `kaveon-exec`, and `kaveon-server`, including all targets: passed
- `cargo test --workspace --no-fail-fast`: 100 passed, 0 failed
- Focused coverage includes stage-graph validation, task/split identity, weighted AVG state merge, exact distinct set union, exchange chunk corruption and bounds, strict bearer validation, and alternate-worker retry selection
- Docker worker-loss, end-to-end exchange, distributed AVG/distinct, TopN, join, spill, and scheduler stress evidence: not yet complete

## Continuation point

Complete the current round in this order:

1. Validate and integrate the shared stage/fragment contracts.
2. Validate the exchange envelope and expose authenticated worker exchange endpoints.
3. Replace aggregate-specific task fan-out with the fragment scheduler.
4. Finish mergeable AVG and exact distinct state and run local/distributed equivalence tests.
5. Add distributed partial/final TopN.
6. Add repartitioned hash joins only after exchange backpressure is proven end to end.
7. Add memory reservations and spill before broad concurrency benchmarks.
8. Exercise worker loss, retry, cancellation, skew, and admission under Docker before making a Trino-class claim.

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
