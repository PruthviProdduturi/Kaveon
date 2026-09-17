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
- Memory admission (2026-09-17): a FIFO queue on both roles instead of an immediate refusal. A statement whose budget does not fit waits in the coordinator's queue (`KAVEON_MEMORY_ADMISSION_QUEUE`, default 64; `KAVEON_MEMORY_ADMISSION_WAIT_SECONDS`, default 60; per request `settings.admission_wait_seconds`), visible as `QUEUED` and cancellable by ID; the head is admitted first and only when its whole budget fits. Workers queue tasks the same way, bounded by cancellation and the task timeout. HTTP 429 `MEMORY_ADMISSION_REJECTED` remains for a full queue or an expired wait and carries `admission_wait_ms`; query records carry `admission_wait_ms`; `/v1/node` and `/v1/cluster` report the queue depth and the admitted/queued/rejected/withdrawn counters. Verified by 12 focused tests on a 684-test workspace; not yet measured on AKS

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

### Distributed changes on `dev`, 2026-09-16 (Claude, while Codex is away)

Every item is one commit with tests; the differential sweep (`scripts/differential-cases.py`, 28 shapes, dictionary versus plain objects) matched 28/28 on AKS after them.

- `eb13ee6` — UNION / INTERSECT / EXCEPT fragments: set-operation nodes were pushed with no inputs and every such query fell back to the coordinator; `FragmentNode::validate_shape` now requires ≥ 2 inputs for Union and exactly 2 for Intersect/Except, and INTERSECT/EXCEPT deduplicate each side on its workers before the single final task.
- `b1f190f` — semi and anti joins distribute: the subquery side broadcasts into the probe stage (`attach_broadcast_join`, `JoinSpec { join_type: Semi | Anti, broadcast: true }`; `fragment_exec` compiles them to `SemiJoinOperator`); `SELECT DISTINCT` deduplicates on the workers before the exchange; the coordinator logs the reason when the stage planner cannot express a shape (it fell back silently).
- `bb82ea9` — `DISTINCT` over named columns hash-partitions across the workers (`PartitionedDistinct`, task count = worker count) instead of one final task; the SQL layer rewrites a lone `COUNT(DISTINCT x) [GROUP BY k]` to `COUNT(x)` over those distinct rows.
- `e578db1` — a worker's `/v1/internal/query/{id}/finish` cancels the query's token before removing it (orphaned tasks kept running after the coordinator gave up), remote task timeouts are not retried, `REMOTE_TASK_TIMEOUT` is 600 s.
- `2608c42`, `c851f6c` — exchange payload ceiling 1 GiB (256 chunks of 4 MiB), receive limit matches it, process spool 4 GiB.
- `8a4ff16` — partial-state encoding and final-merge memory estimates sized to the structures (were 4 KiB and ~5 KiB per group).
- `b99aa22`, `cd2dabc` — partial states encode straight into the binary columns without per-group vectors or a key sort; the final merge indexes groups by encoded key bytes and merges in place. Canonical (sorted) key order in the grouped-state batch is gone: consumers merge by hash, and `decode_grouped_aggregate_states` checks uniqueness rather than order.
- Not changed: no aggregate or join spill; the ceilings above are what bound a 100 M-row high-cardinality GROUP BY. Worker budget on AKS is now 3 GiB per query (`KAVEON_QUERY_MEMORY_LIMIT_BYTES`), admission 4 GiB, first set with `kubectl set env` on the StatefulSet and since folded into `infra/helm/kaveon-test` (`workers.memory.*`, `workers.exchange.*`, `coordinator.memory.*`, `coordinator.exchange.*`) so `helm upgrade` reproduces the running StatefulSets.

Later the same day ("fix all gaps"), one commit each with tests, workspace green (612 tests):

- `bf80ab7` — **columnar hash aggregate** (`exec/src/columnar_aggregate.rs`): typed key vectors (integers, dates, booleans, text through an arena, dictionary text by value), flat accumulator columns, one hashbrown table of slot ids; a batch is three passes (key words, slots, one loop per aggregate). The partial stage encodes the exchange batch straight from the columns (`HashAggregate::into_partial_batch`), the final stage parses each encoded key into words and folds states into columns (`ColumnarGroups::merge_encoded`, `IncrementalAggregateMerger` decides columnar from the first row), and the finalised batch comes from `key_arrays`/`output_arrays`. The three slot-indexed row paths are gone. Memory: a batch's worst case is ensured before it is applied, the actual cost charged after, index doublings reserved once per capacity.
- `885c197` — the partial stage takes `ParallelPartials` whenever the node runs more than one thread; each thread's operator is the spill-capable `PartitionedHashAggregate` when `KAVEON_HASH_SPILL_ROOT` is set, with `with_budget_share(threads)` so the threads together buffer what the serial operator did. Nested partitioners over the same keys are salted (`THREAD_PARTITION_SALT`, `SPILL_PARTITION_SALT` in `exchange.rs`): a plain `hash % 16` inside a plain `hash % 4` left three quarters of the spill partitions empty.
- `ec1deec` — `ParallelPartials` is one parallel operator over a thread factory; DISTINCT (both the pre-exchange and the partitioned stage) runs through it too (`ParallelPartials::distinct`, `fragment_exec::distinct_operator`). The source is pumped from `next_batch` rather than drained in `start`: a streaming operator filled the bounded output queue before its input was drained and the two sides waited on each other.
- `36ffc27` — REGEXP_REPLACE through a StringBuilder with a process-wide compiled-pattern cache.
- `2c1068c` — **process memory model** (`core/src/process_memory.rs`): a counting global allocator in `kaveon-server`, the cgroup limit (v2 then v1, `KAVEON_PROCESS_MEMORY_LIMIT_BYTES` to override), `ProcessMemory` with a headroom (max(256 MiB, 15 %)); every reservation on an admitted pool answers to it before the query budget. A limited node not told its admission limit takes limit − headroom; an admission limit above the process limit is refused at startup. `/v1/node` and heartbeats carry `memory_allocated_bytes` and `memory_limit_bytes`.
- `b464a4e` — query records carry `execution: {mode, detail}` (distributed path taken, or the reason the coordinator ran it); Studio shows it.
- `64ba6ce` — the differential sweep runs under `cargo test` (`server/src/differential_tests.rs`, two Parquet encodings of generated rows through the local planner). It found and the commit fixes: text literals against Date32 columns (`ScalarValue::coerced_for`, `StoragePredicate::coerced_for`, applied by all three readers and the executor), MIN/MAX over Date32 (fold as day numbers, come back as dates on every path), HAVING on the coordinator-local path.
- `08041c7` — per-lane scan metrics (lane count, lightest/heaviest lane by rows and time) from the ADLS decoder through task metrics to the query record and Studio.
- `c087ff9`, `dc3ca2f` — **grouped partials flush on memory pressure** (`FlushingPartialAggregate`): the 8×-reduction probe and the whole-input replay through sixteen spill partitions are gone for grouped partials; a partial aggregates until a sixth of its budget share is held, emits its groups to the exchange, resumes. Used by the spill-path `PartitionedHashAggregate`, the parallel threads and the serial fragment partial. The global COUNT(DISTINCT) partial keeps the bounded path.
- `aaaffdd` — **the exchange output streams while the task runs** (`execute_fragment_streaming`, `exchange::StreamingOutput`, `upload_chunk_stream`): one lane per (output partition, destination), 4 MiB chunks into a bounded channel, four uploads in flight; chunks of a streamed output carry an open count and the last the final count (both stores learn it from the chunk that carries it). Per-partition payloads up to 8 GiB, consumer spool 12 GiB, coordinator per-query disk share `KAVEON_EXCHANGE_QUERY_DISK_LIMIT_BYTES`. The columnar table's doubling reservation is one guard replaced at each doubling (the final merge reserved 3× its size); DISTINCT over dictionary-only columns slices round-robin with a serial DISTINCT over the union.
- `1cbbcce` — **the final aggregate merges on several threads, in memory first** (`compile_final_aggregate_replayable`): the exchange spool is reopenable, so the parallel in-memory merge runs first with its result held, and only a budget refusal replays through the partitioned disk path.
- Differential 28/28 on `bf80ab7` and `aaaffdd` (`scale-suite/differential-2026-09-16-{bf80ab7,aaaffdd}.json`).
- `427b166`, `c4d7750` — workers spool the exchanges addressed to them on their own disk (`KAVEON_WORKER_EXCHANGE_SPOOL`, `KAVEON_IPC_SPOOL_ROOT`) so the coordinator relays nothing; the disk store's insert runs on the blocking pool (under streamed uploads the coordinator's probe stopped answering and the kubelet killed it).
- `8d15fd3` — a TopN straight over the final aggregate (through at most a projection) runs inside every merge thread (`final_under_top_n`, `FinalTail`), and the columnar table is consumed column by column as it finalises: the merged groups of `GROUP BY URL` never exist as a whole. `ReplayableFinal` replays through the disk path only before its first row is out.
- `76f1a5b` — DISTINCT deduplicates through the columnar table (`keep_by_columnar`), reservations in 64 KiB slabs (`ReservationSlab`) rather than one per key.
- `17a33e6` — `ORDER BY … OFFSET n LIMIT m` is a top-N (`LogicalPlan::top_n`): each partition keeps n + m rows, the target merges them and drops n once; the merge-thread TopN applies to the offset form too (ClickBench q40 planned a full sort of eighteen million merged groups on each worker and failed its budget).
- First uninterrupted 43-statement ClickBench pass on `8d15fd3` (`docs/qualification/clickbench/runs/kaveon-8d15fd3-2026-09-17.json`): 42 ran, q40 rejected (fixed above); geometric mean Trino ÷ Kaveon 1.39× over the 42. Open: the final merge rate over near-unique keys (q19/q33/q34/q35 at 0.24–0.46× Trino), the DISTINCT stage of q14, exchange consumers decode after the whole download.
- `d8a71d2`, `836f673` (2026-09-17) — **the columnar final merges by the batch through a prefetched slot index**: `ColumnarGroups` owns an open-addressed index (tag byte + slot per bucket, ¾ load, buckets prefetched sixteen rows ahead, doublings rehash from the columns in slot order) in place of the hashbrown table, `merge_encoded_batch` parses a batch's keys to packed words, resolves slots, then folds the compact states straight from their bytes into the accumulator columns; the merger reserves per batch (worst case ensured, actual charged, scratch held, the largest doubling a batch can cause covered first). `merge_rate` benchmark (ignored test, 4 M q19-shaped rows): 417–496 → 134–146 ns/row locally. Not yet measured on AKS.

- `1137791`, `eeffad6`, `c29b5fb`, `ba75197`, `3411cf7`, `dc4966a` (2026-09-17) — **the final aggregate is a hybrid hash merge that never starts over** (`exec/src/final_merge.rs`, `HybridFinalMerge`): the in-memory merge holds what the budget admits; a refusal spills the thread's own table as encoded partial rows into salted-hash sub-partitions (one run each, `SpillRunWriter`) and the merge continues from the row the refusal fell on; at the end each sub-partition comes back on its own as a unit of complete groups, emitted in 4096-row batches with the TopN tail still inside the thread. The replay through `partitioned_final_aggregate` is gone (same fail-closed bound, without writing the whole input first). Memory: `OperatorMemoryAccount::prepaid` (core), a spill reserve and the doubling guard released at finish, the pump's queues reserved up front and given back at drain, a `Rendezvous` before emission. The exchange payloads decode on threads of their own (`ExchangeInputProvider::open_each`, `ParallelPartials::broadcast`) and every merge thread keeps the rows whose key hashes to it (`ThreadSelector`), so the calling thread no longer decodes, hashes or copies anything. Local, release, six million q33-shaped partial rows, 640 MiB, three threads: 5.3–6.1 s → 1.32–1.57 s wall, 1019 MiB/496 runs/128 compactions → 376–565 MiB/64–112 runs/0 compactions; with a budget that holds the merge 1.43 → 0.72–0.79 s. Not yet measured on AKS.

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
