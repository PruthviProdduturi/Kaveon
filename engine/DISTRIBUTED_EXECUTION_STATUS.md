# Distributed Execution Status

This file is the durable handoff for Kaveon's path from the current alpha executor to a fault-tolerant distributed analytical engine. It records verified behavior, incomplete work, and the next safe continuation point. Update it with every distributed-execution commit.

## Current baseline

- Branch: `dev`
- Execution unit: Arrow `RecordBatch`
- Storage partitioning: deterministic Parquet row-group and Delta active-file partitions
- Coordinator/worker transport: HTTP task submission and Arrow IPC task results
- Shipped distributed query shapes: scan/filter/project, partial/final grouped and global aggregates, Sort/TopN/limit, repartitioned or broadcast hash joins, window functions, INTERSECT/EXCEPT set operations
- Shipped local query shapes: filters, projections, aggregates (including DISTINCT on SUM/AVG), sort/TopN, hash joins, window functions (ROW_NUMBER/RANK/DENSE_RANK/LAG/LEAD/SUM/AVG/COUNT/MIN/MAX OVER with ROWS/RANGE/GROUPS frame specs), INTERSECT/EXCEPT, EXTRACT, DATE_TRUNC/DATE_PART/TO_CHAR/NOW/CURRENT_DATE/CURRENT_TIMESTAMP, Decimal128 type and literals, IN/NOT IN/EXISTS/NOT EXISTS subqueries via semi/anti join
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
| 6 | Memory accounting and spill | In progress | Hard reservations; bounded exchange storage; fixed-fan-in multi-pass lazy Sort/TopN spill merge; per-task compute, exchange partition/copy, IPC, memory, and spill telemetry | Aggregate/join spill, admission/revocation, and full operator-level telemetry |
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
- `cargo test --workspace --no-fail-fast`: 228 passed, 0 failed
- `cargo clippy --workspace --all-targets -- -D warnings`: passed
- Focused coverage includes graph/fragment agreement, partition-correct scans, weighted AVG and exact distinct partial/final execution, empty Arrow schemas, exchange identity/corruption/byte bounds, broadcast routing, fixed-fan-in spill compaction, retry cleanup, deterministic split steal/requeue, window functions (ROW_NUMBER/RANK/DENSE_RANK/LAG/LEAD with PARTITION BY and ORDER BY), INTERSECT/EXCEPT set operations, EXTRACT over timestamps and dates, date/time scalar functions, and DISTINCT on SUM/AVG aggregates
- Two-worker Docker correctness on `F:\kaveon-data`: exact DISTINCT returned 100,000; grouped weighted AVG + TopN returned five rows through three stages; TopN returned five rows; repartitioned customer/order join returned 5,000,000 rows. Worker-loss, spill-pressure, concurrency, and comparative performance evidence remain open.

The second integrated gate passed 69 focused tests across core, exec, and server and 112 tests across the full workspace after distributed TopN, authenticated exchange endpoints, memory reservations, and split leasing were added. Strict Clippy passed; Windows emitted filesystem-only incremental-cache hard-link warnings and copied the files instead.

The third integrated gate passed 85 focused tests across core, exec, and server and 128 tests across the full workspace after aggregate-state wire encoding, the stage DAG builder, spill-run infrastructure, and task timeout/retry classification were added. Strict Clippy and formatting passed. The Windows incremental cache again used file copies when hard links were unavailable; this is an environment warning, not a Rust lint.

The fourth gate passed 96 focused tests across core, exec, and server and 139 tests across the full workspace after the stage runtime, idempotent task/cancellation lifecycle, and spill-aware Sort/TopN paths were added. Strict Clippy and formatting passed. Spill accumulation is bounded, but final run merging is still eager and is not yet a fully bounded external sort.

The fifth gate passed 106 focused tests across core, exec, and server and 149 tests across the full workspace after executable fragments, canonical grouped aggregate states, lazy spill merging, HTTP lifecycle integration, and terminal cleanup were added. Strict Clippy and formatting passed. Spill merge retains one cursor batch per run, so fixed fan-in/multi-pass compaction remains required for a strict constant ceiling.

The sixth gate passed 164 tests across the full workspace after general coordinator/worker fragment execution, exchange v2 routing, distributed partial/final aggregates and joins, fixed-fan-in spill compaction, and local split enumeration were added. Strict workspace Clippy and formatting passed. The Rust 1.88 release image built and a coordinator plus two workers completed representative Delta queries over the real local dataset. This run exposed and closed release-toolchain compatibility, aggregate projection, HTTP exchange-body, and grouped-state partition-key defects. Worker-loss, concurrency, pressure, and comparative performance evidence have not yet been run.

The seventh gate passed 216 tests across the full workspace after SQL coverage expansion: window functions (ROW_NUMBER, RANK, DENSE_RANK, LAG, LEAD, and aggregate windows with PARTITION BY/ORDER BY), INTERSECT/EXCEPT set operations, DISTINCT on SUM/AVG aggregates, EXTRACT over all date fields, and date/time scalar functions (NOW, CURRENT_DATE, CURRENT_TIMESTAMP, DATE_TRUNC, DATE_PART, TO_CHAR/DATE_FORMAT). All new plan types are wired through the optimizer, local CLI planner, distributed server planner, fragment executor, and server API. Strict workspace Clippy and formatting passed.

The eighth gate passed 228 tests across the full workspace after window frame specifications (ROWS/RANGE/GROUPS BETWEEN with all bound types), Decimal128 type support (literals, casting, aggregation), and IN/NOT IN/EXISTS/NOT EXISTS subquery rewriting to semi/anti joins were added. SemiJoinOperator uses hash-based build/probe. Distributed semi/anti joins return explicit unsupported errors; local execution is fully wired. Strict workspace Clippy and formatting passed.

The September 11 AKS profiling follow-up added bounded cumulative task counters at the remaining diagnostic boundaries. Hash repartition now reports row hashing/encoding time separately from Arrow `take` time, allocation count, and copied bytes. Worker execution reports blocking-pool queue delay and compute wall time alongside Linux thread CPU. Spill snapshots report cumulative Arrow IPC write/read time alongside existing bytes, runs, and compactions. Existing fetch, IPC decode, encode, upload, memory, and admission counters remain intact. The counters use two clocks per hash-partition batch or spill operation and atomic accumulation for shared spill state; query results and execution decisions are unchanged. Focused exchange, spill, fragment, and task-metric tests passed, as did strict workspace Clippy and formatting checks on all four touched Rust files. No AKS deployment or measurement was performed.

Delta join planning now carries the exact analyzed transaction-log version into executable fragment construction. The pin is keyed by the resolved source URI from the query's immutable catalog publication, so a later add/remove commit cannot make the physical scan diverge from the snapshot whose exact cardinality selected the join distribution, and a catalog source replacement cannot consume the old source's pin. This also avoids resolving the Delta head a second time during fragment construction. Parquet behavior is unchanged. All 151 server tests and the focused storage statistics tests pass; no deployment or performance measurement was performed.

Local partial aggregation now samples at most eight batches and 65,536 rows
under the query memory pool before choosing its worker dispatch. A balanced
sample with at least 4,096 distinct canonical group hashes uses key-affine
dispatch, so one local worker owns each group key; low-cardinality, skewed,
global and single-worker aggregates retain round-robin dispatch. Partition
copies and the sample are conservatively reserved, channels remain bounded,
and the existing partition/spill operator remains the hard fallback. Per-task
telemetry reports the selected mode, sample rows/distinct hashes, and affinity
rows/bytes. Local exactness, gating, cancellation and memory-release tests plus
all 120 execution and 151 server tests pass. AKS performance remains unproven
until an immutable image completes the exact profile and throughput guard.

The key-affine `ParallelPartials` experiment and its distributed-fragment wiring
were rejected after an exact AKS profile. Enabling three local workers activated
affinity, but copied 80.18 MB during routing, retained 428,832 created groups,
increased stage-0 spill from 82.24 MB to 85.41 MB, regressed high-cardinality
latency from 0.994 s to 1.127 s, and regressed DISTINCT from 0.365 s to 0.691 s.
The runtime path and AKS chart defaults were reverted. Accepted source `e56aece`
and digest `sha256:270d39a...` remain the performance checkpoint.

An exact-identity ADLS Parquet footer-cache experiment was also rejected. It
revalidated every logical open with a network HEAD, preserving replacement
correctness but regressing an exact 120-query probe to 2.765291 QPS from the
2.834559-QPS accepted mean. Source `e9d4b76` was reverted. Reusable footer or
decoded-batch state must consume an already-pinned identity rather than adding
per-query network validation.

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
