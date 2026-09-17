# Engine memory management

**Maturity:** Alpha runtime control; aggregate/join spill remains a release gate.

Kaveon treats memory limits as execution correctness. An operator must reserve memory before retaining state. When a reservation cannot be satisfied, it must spill through a supported spill path or return an explicit execution error. Silent overcommit is not an accepted behavior.

## Budget hierarchy

1. `MemoryAdmissionController` reserves a complete per-query budget before execution. The sum of admitted budgets cannot exceed the process admission limit.
2. `QueryMemoryPool` atomically enforces the hard limit shared by every operator in one query.
3. `OperatorMemoryAccount` attributes current and peak reservations to a named operator.
4. `MemoryReservation` returns its bytes through RAII on success, error, cancellation, or operator destruction.

Admission and reservation counters are thread-safe. A rejected admission or operator reservation leaves accounting unchanged.

## Operator behavior

| Operator | Current behavior |
|---|---|
| Sort | Opt-in memory accounting and bounded Arrow IPC spill runs; fixed-fan-in multi-pass merge |
| TopN | Opt-in memory accounting and bounded spill runs; fixed-fan-in merge |
| Hash aggregate | Opt-in accounting for group state and exact-distinct values; fails closed when its query budget is exhausted |
| Hash join | Opt-in accounting for retained inputs, build index, match bitmap, and output-index growth; fails closed when its query budget is exhausted |
| Exchange | Independent byte and exchange-count ceilings with accounting released on cleanup |

Hash aggregate and hash join do not yet spill. Their bounded mode protects a process by rejecting work that exceeds its assigned query budget. Production readiness requires partitioned aggregate/join spill before large or skewed workloads can rely on those operators.

## Admission lifecycle

An admitted query owns its budget until its admission guard is dropped. The query pool subdivides that budget among operator accounts; operator reservations do not change the admitted budget. Cancellation and error paths must destroy operators and their guards so both retained memory and admission capacity are released.

The coordinator admits each submitted query against `KAVEON_MEMORY_ADMISSION_LIMIT_BYTES` and assigns `KAVEON_QUERY_MEMORY_LIMIT_BYTES`. A query whose budget does not fit on arrival waits in a FIFO admission queue (`KAVEON_MEMORY_ADMISSION_QUEUE`, default 64) for at most `KAVEON_MEMORY_ADMISSION_WAIT_SECONDS` (default 60), and is refused with HTTP 429 `MEMORY_ADMISSION_REJECTED` only when the queue is full or the wait expires; workers queue tasks the same way, bounded by cancellation and the coordinator's task timeout. The head of the queue is served first and only when its whole budget fits, so a large budget is never starved by smaller arrivals and the order is the arrival order. Local plans and worker fragments propagate query pools to hash aggregate and hash join. Compatibility constructors remain available for embedded callers, so this is server-runtime enforcement—not a claim that every library embedding is bounded.

Every node also answers to its process limit: the container's cgroup limit, read by the Engine itself, or `KAVEON_PROCESS_MEMORY_LIMIT_BYTES` when set (0 disables). A headroom of the larger of 256 MiB and 15 % is kept free. A node not told its admission limit takes the process limit less the headroom; an admission limit above the process limit is refused at startup. Deployments should let the Engine read the cgroup limit rather than set the override.

On the AKS qualification cluster (`infra/helm/kaveon-test`, values `<role>.memory.*`) workers run 3 GiB per query and 4 GiB admission inside a 6 GiB container, and the coordinator 512 MiB per query and 2 GiB admission inside 4 GiB.

## Required production evidence

- concurrent admission never exceeds the configured process ceiling, and a queued arrival is admitted in order once budget is released;
- aggregate and join state remain within their query budget under high cardinality and skew;
- cancellation, retry, worker loss, and operator errors return every reservation;
- spill disk limits and cleanup hold during partial failures;
- telemetry reports measured current, peak, and spilled bytes without deriving values;
- performance tests cover both in-memory and forced-spill execution.
