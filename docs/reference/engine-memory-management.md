# Engine memory management

**Maturity:** Alpha runtime control. Every path below is implemented and
tested; qualification under skew, disk exhaustion and sustained load remains
a release gate. The variables are listed with their defaults and AKS values
in the [settings reference](../engine/settings.md).

Kaveon treats memory limits as execution correctness. An operator must
reserve memory before retaining state. When a reservation cannot be
satisfied, it spills through a supported spill path or returns an explicit
execution error. Silent overcommit is not an accepted behavior.

## Budget hierarchy

1. `ProcessMemory` guards the whole process: a counting global allocator in
   `kaveon-server` and the container's cgroup limit (v2, then v1;
   `KAVEON_PROCESS_MEMORY_LIMIT_BYTES` overrides, `0` disables) with a
   headroom of the larger of 256 MiB and 15 %. Every reservation on an
   admitted pool answers to live allocated bytes before its query budget.
   A node not told its admission limit takes the process limit less the
   headroom; an admission limit above the process limit is refused at
   startup. `/v1/node` and worker heartbeats carry `memory_allocated_bytes`
   and `memory_limit_bytes`.
2. `MemoryAdmissionController` admits a complete per-query budget before
   execution; the sum of admitted budgets cannot exceed the node's admission
   limit. Arrivals that do not fit wait in a FIFO queue (below).
3. `QueryMemoryPool` atomically enforces the hard limit shared by every
   operator in one query; it also carries the query's shared resources (the
   spill manager, the per-request parallelism ceiling).
4. `OperatorMemoryAccount` attributes current and peak reservations to a
   named operator; `prepaid` balances are taken once and served first so
   threads on one budget do not reach their refusals together.
5. `MemoryReservation` returns its bytes through RAII on success, error,
   cancellation, or operator destruction.

Admission and reservation counters are thread-safe. A rejected admission or
operator reservation leaves accounting unchanged.

## Admission queue

The coordinator admits each statement against
`KAVEON_MEMORY_ADMISSION_LIMIT_BYTES` with its query pool
(`KAVEON_QUERY_MEMORY_LIMIT_BYTES`, or the request's lower
`settings.query_memory_limit_bytes`). A statement whose pool does not fit on
arrival waits in a FIFO queue (`KAVEON_MEMORY_ADMISSION_QUEUE`, default 64)
for at most `KAVEON_MEMORY_ADMISSION_WAIT_SECONDS` (default 60, or the
request's lower `settings.admission_wait_seconds`; `0` refuses at once). The
head of the queue is served first and only when its whole budget fits, so a
large budget is never starved by smaller arrivals and the order is the
arrival order. HTTP 429 `MEMORY_ADMISSION_REJECTED` is returned only when
the queue is full on arrival, the request asked not to wait, or the wait
expired, and carries `admission_wait_ms`. A waiting statement is `QUEUED` in
the history and `DELETE /v1/query/{id}` removes it at once. Workers admit
each task through the same queue in arrival order, bounded by cancellation
and the coordinator's 600 s task timeout rather than a wait of their own.
Counters (`queue_limit`, `queue_depth`, `admitted`, `queued`, `rejected`,
`withdrawn`) are on `/v1/node` and per node on `/v1/cluster`. A resource
group's queue, when the principal has one, is passed before memory
admission, so the two waits add. Queue setting `0` restores the pre-2026-09-17
behaviour of refusing on arrival.

## Operator behavior

| Operator | Current behavior |
|---|---|
| Hash aggregate | Columnar table (typed key vectors, flat accumulator columns, open-addressed slot index) with reservations per batch: the worst case ensured before a batch is applied, the actual cost charged after, each index doubling reserved before it happens. Grouped partials flush their groups to the exchange at a sixth of their budget share and continue (`FlushingPartialAggregate`), and judge each flush round: one that made a group for four in five of its rows stops aggregating and passes rows through as their own partial rows through a small table cleared per batch (`PartialBatchEncoder`), re-judging an aggregating round after every pass-through window (`KAVEON_ADAPTIVE_PARTIAL_AGGREGATION`); the final stage is a hybrid merge that, when a batch is refused, spills its own table as encoded partial rows into salted sub-partitions and continues from the row the refusal fell on, then returns each sub-partition as a unit of complete groups (`HybridFinalMerge`); nothing is read twice. The global `COUNT(DISTINCT)` partial and the spill-capable `PartitionedHashAggregate` keep the bounded partitioned path. |
| DISTINCT, set operations | Keys through the columnar table on several threads, reservations in 64 KiB slabs; fail closed at the budget |
| Hash join | Accounted inputs, build index, match bitmap and output growth. Under `KAVEON_HASH_SPILL_ROOT` the build side is spill-safe on every path the planner emits (broadcast and partitioned; inner, left, right, full): `PartitionedHashJoin` collects the build while the budget admits it and builds it in memory (`BuiltSide`: the concatenated rows, the index, the outer bitmap, each reserved before it is made); a refusal — of a batch's reservation, of the built form, or of a build that would take more than half of the budget free when it began — partitions both inputs by the join keys' hash (salted, as the aggregate's spill is) into `KAVEON_HASH_SPILL_PARTITIONS` partitions on disk, builds and probes each within the budget with its memory exact through the operator's account, and repartitions a partition whose build is refused in turn under the next level's salt, to `MAX_JOIN_SPILL_DEPTH` = 3 (65 536-way at the default sixteen). Past that depth — key skew no partitioning resolves — it fails closed with the budget's message. A cross join has no key to partition by and fails closed at the first refusal. A broadcast build is the same rows on every task, so each task spills its own. Task metrics: `join_spill_bytes_written`, `join_spill_partitions`, `join_spill_depth`. |
| Semi / anti join | The right input built as a key set, or as its rows by key under a residual (Q21's shape), the left probed batch by batch. Under `KAVEON_HASH_SPILL_ROOT` `PartitionedSemiJoin` gives the build the same spill: a refusal partitions both inputs by the hash of the key expression's value (the equality the build uses, so a dictionary and a plain column, or two integer widths, land together), builds and probes each partition on its own, repartitions to the same depth limit. `NOT IN`'s rules read the whole set, so the partitioning records whether any right key was NULL and whether any was not, and every partition is probed with those facts. |
| Sort / TopN | Accounted input and merge workspaces; bounded Arrow IPC spill runs with fixed-fan-in multi-pass merge |
| Exchange | Streamed output in 4 MiB chunks through a bounded channel; disk stores with node and per-query byte ceilings; received payloads spooled to disk (12 GiB per process) and decoded one producer per thread with the active batch charged to query memory |
| Scan | Decoder lanes read into bounded channels; full-object and decoded-batch caches are process-wide and explicit (256 MiB each) |

Spill for every operator is enabled by `KAVEON_HASH_SPILL_ROOT` (the AKS
chart sets `/tmp/spill` with a 4 GiB per-query budget); without it, an
accounted operator fails closed at its budget. The join spill is
verified in process (every join type, semi and anti with and without a
residual, `NOT IN` with a NULL in the set, the depth limit under total
skew, the differential of six join shapes and of `orders ⋈ lineitem` at
six thousand orders on both paths under a budget that refuses the
build); its qualification on the cluster under skew and disk exhaustion
is still open, as is the aggregate's.

## Admission lifecycle

An admitted query owns its budget until its admission guard is dropped. The
query pool subdivides that budget among operator accounts; operator
reservations do not change the admitted budget. Cancellation and error paths
destroy operators and their guards so both retained memory and admission
capacity are released; finishing a query cancels its orphaned worker tasks.
Compatibility constructors remain available for embedded callers, so this is
server-runtime enforcement, not a claim that every library embedding is
bounded.

On the AKS qualification cluster (`infra/helm/kaveon-test`, values
`<role>.memory.*`) workers run 3 GiB per query and 4 GiB admission inside a
6 GiB container, and the coordinator 512 MiB per query and 2 GiB admission
inside 4 GiB; `KAVEON_PROCESS_MEMORY_LIMIT_BYTES` is deliberately not set so
the Engine reads the cgroup limit.

## Required production evidence

- concurrent admission never exceeds the configured process ceiling, and a
  queued arrival is admitted in order once budget is released (unit-tested;
  the 8-client throughput tier has not been rerun on the cluster since the
  queue landed);
- aggregate and join state remain within their query budget under high
  cardinality and skew, with the hybrid final merge measured on the cluster;
- cancellation, retry, worker loss, and operator errors return every
  reservation;
- spill disk limits and cleanup hold during partial failures;
- telemetry reports measured current, peak, and spilled bytes without
  deriving values (the final merge's spilled tables and groups are not yet on
  the task metrics);
- performance tests cover both in-memory and forced-spill execution.
