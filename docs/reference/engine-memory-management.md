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

The coordinator admits each submitted query against `KAVEON_MEMORY_ADMISSION_LIMIT_BYTES` and assigns `KAVEON_QUERY_MEMORY_LIMIT_BYTES`. Local plans and worker fragments propagate query pools to hash aggregate and hash join. Compatibility constructors remain available for embedded callers, so this is server-runtime enforcement—not a claim that every library embedding is bounded.

## Required production evidence

- concurrent admission never exceeds the configured process ceiling;
- aggregate and join state remain within their query budget under high cardinality and skew;
- cancellation, retry, worker loss, and operator errors return every reservation;
- spill disk limits and cleanup hold during partial failures;
- telemetry reports measured current, peak, and spilled bytes without deriving values;
- performance tests cover both in-memory and forced-spill execution.
