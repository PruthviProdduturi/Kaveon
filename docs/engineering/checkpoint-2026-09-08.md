# Kaveon validation checkpoint — September 8, 2026

The user requested a proper test of the current build, followed by a pause and
review. Feature development and performance tuning stop at this checkpoint.
The previous 8/10 readiness and 1.9× Trino throughput objective is **not achieved**.
The last documented engineering estimate is 7.4/10; it has not been independently
certified or increased merely because more tests passed.

## Validation

| Check | Result | Local evidence |
|---|---|---|
| Rust workspace | 328 tests pass | `tmp/checkpoint-workspace-tests.log` |
| Rust formatting and strict workspace/all-target Clippy | Pass after formatting cleanup | `tmp/checkpoint-clippy.log` |
| Five-worker SQL, paging, concurrency and worker loss | 37 SQL cases and operational checks pass | `tmp/smoke-sept8-five-workers/report.json` |
| Local memory pressure | 11/11 pass | `tmp/pressure-sept8-local/report.json` |
| Two-worker memory pressure | 10/10 pass | `tmp/pressure-sept8-two-workers-quota-fixed/report.json` |
| Current frozen-build ten-minute soak | All 10 gates pass; 4,171 exact queries, 23 cancellation cycles, worker recovery, zero retained files | `tmp/soak-sept8-two-workers-600s/report.json` |
| API security/integration | 26 tests passed on unchanged API source | Native test evidence summarized in the security guide |
| TLS and real HTTP platform bridge | 6 and 8 checks passed | Security qualification guide |
| Extended same-file SQL | All 12 queries pass at five million rows/100,000 customers | `tmp/extended-five-million-r6-correctness/report.json` |
| Local integrated stack | Healthy; Studio/API/two-worker query returns COUNT=3, SUM=6 | `tmp/local-stack-qualified/query-result-r6.json` |
| Claude's `01bdd86` data-source fix | Create, disable, re-enable and encrypted storage pass through Studio/API/PostgreSQL; fixture deleted | `tmp/local-stack-qualified/data-source-01bdd86.json` |

The first two-worker pressure run is retained. Its disk-quota case inherited a
large memory pool, allowing the adaptive aggregate to finish without spilling.
That case now uses at most 32 MiB to force spill and test the 1 KiB disk quota.
Engine limits were not relaxed to make the test pass.

The native test executable and its exact pre-format source snapshot are preserved
under `tmp/qualification-sept8-frozen`. Later changes to engine source were
formatting only. The Docker r6 image is separately identified by the extended
report. Native and Docker binaries are distinct builds; their evidence is not
interchangeable.

All qualification processes stopped after the final soak. Development is paused
at the user's request; only the authorized checkpoint commit/push follows. The
local Compose environment remains available. The [AKS deployment plan](aks-deployment-plan.md)
records infrastructure prerequisites and missing inputs; no AKS resources were
provisioned or deployed.

## What changed

- Corrected NULL/subquery/window/set and exact aggregate semantics, including
  parallel aggregate output names and schemas.
- Added memory accounting, bounded spill/exchange/result handling, cancellation
  cleanup, adaptive spill, compact aggregate state and streaming hash joins.
- Added fail-closed Engine identities, TLS, ownership checks, platform bridging
  and explicit credential encryption/migration. Async API results now enforce
  ownership and cannot reappear after deletion.
- Added pinned Delta/checkpoint and Iceberg reading, consistent projections,
  Delta row-group pruning and exact eligible metadata-only COUNT.
- Provisioned the local build/test environment and integrated Compose stack.

## Review before resuming

1. Review the substantial checkpoint changes by area before resuming development.
   The user authorized committing and pushing all changes after final validation.
   Existing `dev` pushes trigger the Container Apps workflow; AKS deployment is
   a separate planned activity, not performed during this checkpoint.
2. Confirm the intended readiness/deployment scope and the meaning of “90%
   better.” The working performance metric has been 1.9× throughput.
3. Complete the declared publication benchmark protocol: extended workload,
   five warmups, thirty measurements, equal resources and full result reporting.
   The six-query diagnostic is around 1.05× throughput, not 1.9×; joins and
   high-cardinality groups remain expensive.
4. Resolve live cloud qualification scope. The current Azure identity still
   does not list the recorded Kaveon subscription. Cloud tests, production
   migration/configuration, coordinator restart/HA and remaining memory-bound
   coverage must not be inferred from local fixtures.

See [readiness evidence](engine-readiness-qualification.md),
[memory and spill](engine-memory-and-spill.md), and
[security/integration](engine-security-integration.md).
