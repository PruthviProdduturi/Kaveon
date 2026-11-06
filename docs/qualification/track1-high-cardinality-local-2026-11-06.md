# High-cardinality distributed qualification — 2026-11-06

This is a local Docker diagnostic run for the adaptive join and aggregate
paths. It is correctness and bottleneck evidence, not a publication claim.

## Run contract

- Engine image: `kaveon-engine:qualification-current`
- Engine commit recorded by the harness: `9c1c5be97c19e0ffccf12af9f4a1d05349f265be`
- Topology: two Engine workers plus coordinator
- Fixture: five million event rows and 100,000 customers
- Warmups: one per query
- Measured repetitions: two per query
- Cache policy: qualification harness cache policy (no result-cache claim)
- Harness output: `tmp/qualification-adaptive-5m/report.json`

## Result

All 12 extended cases returned exact results (`same_file_correctness: true`,
`passed: true`). The adaptive cases exercised distributed stages and completed
without a reported failure or leaked spill reservation.

The run exposes the next optimization target:

| Case | Kaveon median | Trino median | Relative result |
|---|---:|---:|---|
| `high_groups` — 5M rows grouped by 100K customer IDs | 2,987.7 ms | 988.5 ms | Kaveon 3.02x slower |
| `grouped_join` — distributed join followed by grouped aggregate | 1,415.9 ms | 646.4 ms | Kaveon 2.19x slower |

Kaveon was faster for `filtered_sum` (377.9 ms vs 493.2 ms) and `topn`
(626.0 ms vs 721.3 ms), so this is not evidence for a general regression.
It isolates high-cardinality grouping and join-to-aggregate materialization as
the next profiling target. The run is too small and too short to qualify the
publication gate; it does not establish superiority over Trino.

## Follow-up gate

Before changing the algorithm, capture CPU, allocation, exchange bytes, and
result-serialization timings for `high_groups` and `grouped_join`. The next
implementation change must reduce the dominant measured component while
preserving the exact result hash and spill cleanup under the same fixture.
