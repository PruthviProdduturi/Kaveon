# Engine readiness qualification

This is a proposed evidence rubric for the requested **8/10 readiness** target.
It is not an independently certified rating. The separate “90% better than
Trino” target requires a selected metric and a declared workload; it cannot mean
every SQL feature, deployment scenario, and performance dimension at once.

## Scoring contract

| Area | Points | Evidence required for full credit |
|---|---:|---|
| SQL correctness and types | 25 | Differential results against Trino and DuckDB, typed NULL/empty/overflow cases, joins/subqueries/windows/set operations, explicit rejection of unsupported syntax |
| Memory and spill | 15 | Query/process admission, bounded operators and exchanges, pressure/skew tests, disk quota and cancellation cleanup |
| Distributed execution and recovery | 15 | Multi-worker results, retry identity, snapshot consistency, mid-query worker loss, exchange-host loss, cancellation without leaked resources |
| Authentication and platform integration | 15 | Fail-closed identities/roles, query and result ownership, TLS, catalog bridge revision conflicts, encrypted credentials and migration |
| Storage correctness | 10 | Real Parquet/Delta/Iceberg files, immutable snapshots, supported schema evolution, explicit unsupported-feature errors, authenticated cloud reads |
| Operability | 10 | Reproducible builds, bounded history/results, health/metrics, migrations, restart recovery, sustained mixed-workload soak |
| Performance | 10 | Release-build, same-file, CPU/memory-matched comparison with published queries, checksums, warmup policy, repetitions, concurrency and full results |

An 8/10 claim requires at least 80 points **and** no known silent wrong-result,
authentication bypass, or unbounded common execution path. Passing a handful of
queries earns evidence for those cases only. Open critical gates cannot be hidden
by averaging stronger areas. The rubric and benchmark workload must remain
visible when reporting a rating.

## Current local evidence

### Current assessment — September 10

The evidence-backed score for the current integrated direction is **75/100
(7.5/10)**. This is a readiness estimate for the declared Engine scope, not a
feature-parity score against Trino or PostgreSQL. It remains below 8/10 because
release-critical gaps cannot be averaged away.

| Area | Points | Evidence credited | Evidence still withheld |
|---|---:|---|---|
| SQL correctness and types | 22/25 | 37-case differential suite, 12-query extended same-file suite, typed aggregates/windows/subqueries/set operations, explicit unsupported errors, native `ANALYZE` | Re-run the complete differential corpus on the integrated release image; broaden decimal, timestamp, nested-type, DML and randomized coverage |
| Memory and spill | 11/15 | Admission, bounded result/history/exchange paths, aggregate spill and pressure/cancellation fixtures, bounded coordinator aggregate merge | Complete retained-result/worker-response accounting, join spill under skew, disk exhaustion and current-image pressure evidence |
| Distributed execution and recovery | 11/15 | Multi-worker operators, immutable query catalog pinning, authenticated worker recovery, compatible-worker scheduling, prior worker-loss fixture | Current-image AKS worker loss/node drain, coordinator restart, catalog mutation during query, retry/exchange-loss and sustained concurrency |
| Authentication and platform integration | 13/15 | Entra/TLS, owner-bound results and transactions, role checks, exchange authentication, fail-closed secrets/configuration | Live rotation/revocation, tenant isolation and adversarial authorization qualification on the release deployment |
| Storage correctness | 8/10 | Parquet/Delta/Iceberg readers, pinned source identities, authenticated ADLS reads, CAS-backed immutable product/statistics documents | Live ADLS conflict/fault/restart evidence, broader schema evolution and corruption/recovery qualification |
| Operability | 7/10 | Reproducible CLI/images, Helm/Bicep, health/readiness/metrics/history, checked-in AKS verifiers | Deploy and qualify the current digests, backup/restore, upgrade/rollback, alerting and a sustained current-image soak |
| Performance | 3/10 | Resource-matched harness, exact-result hashes, fail-closed claim evaluators; best recorded six-query diagnostic ratio is 1.057× | Run the publication-scale extended corpus on immutable current images; the required 1.90× Trino throughput result and PostgreSQL transaction comparison do not exist |

Three release-critical conditions currently cap the score below 8/10:

1. No current integrated image has passed the complete AKS correctness, pressure,
   worker-loss, coordinator-restart and catalog-catch-up sequence.
2. Kaveon's transactional surface is a durable, typed product-record protocol.
   Arbitrary relational row DML, parameter binding, constraints/indexes,
   multi-table SQL transactions and a qualified isolation level remain absent.
3. The checked-in comparison gates fail closed because no publication-scale
   1.90× Trino result or completed PostgreSQL transaction report exists.

Claims must therefore stay scoped as follows:

- **Supported:** Kaveon is a distributed lakehouse analytics engine with a
  durable product-record transaction substrate, demonstrated by the named local
  and AKS fixtures.
- **Unsupported:** full Trino feature parity, general PostgreSQL replacement,
  broad production readiness, or performance superiority beyond a declared
  workload whose checked-in gate passes.
- **Measured diagnostic:** the best recorded matched six-query result is 1.057×
  Trino throughput. It is below the 1.90× target and is not the publication
  workload.

Immutable Engine/API/Studio images through `0fe58a9` are deployed, and the
catalog-recovery Engine digest passed a three-worker identity check plus an exact
34-row distributed smoke query. That closes deployment and basic catalog
catch-up; it does not close fault or sustained-load qualification. The exact
next gates are: (1) run the current AKS SQL, worker-loss, coordinator-restart,
pressure and soak suite while retaining machine-readable reports; (2) run the
extended matched Trino publication gate; (3) qualify the declared product-record
transaction scope separately. Re-score only from those artifacts.

### Provisional assessment — September 8

The evidence-based engineering estimate is **74/100 (7.4/10)**, not independent
certification. It does not qualify the subsequent streaming join/TopN changes.

| Area | Provisional points | Main withheld evidence |
|---|---:|---|
| SQL correctness and types | 21/25 | Broader coverage and final combined release qualification |
| Memory and spill | 11/15 | Complete common-path allocation review and renewed pressure evidence |
| Distributed execution and recovery | 11/15 | Broader concurrent failures and coordinator restart limitations |
| Authentication and platform integration | 12/15 | Production migration/configuration verification and immutable identity/policy gaps |
| Storage correctness | 7/10 | Authenticated live cloud reads |
| Operability | 7/10 | Renewed combined-build soak and deployment recovery evidence |
| Performance | 5/10 | Publication workload/scale, warmups, reporting and repeated-run uncertainty |

The six-query million-row results below are optimization diagnostics. The checked-in
benchmark protocol requires at least five warmups and thirty measured executions,
five million fact rows and 100,000 customers, and a broader query set. The harness
now exposes an extended suite and prevents diagnostic runs from setting the
performance target flag. Withheld points remain withheld until evidence closes
the gaps; the separate 1.9× throughput target is still unmet.

Working-tree qualification on 2026-09-05, before final combined release build:

- `tmp/qualification-37-combined/report.json`: 37 semantic cases pass against Trino483
  and DuckDB1.5.5; five native workers; authenticated result paging and replay;
  concurrent requests and a worker-loss query pass.
- `tmp/same-files-initial/report.json`: six queries pass on the same Parquet files
  with 100,000 fact rows. Native debug Kaveon and containerized Trino have unequal
  resources; these timings do not establish relative performance.
- `tmp/same-files-million-combined/report.json`: all six one-million-row queries
  pass with five workers after fixing exchange placement and projection ordering.
  Earlier failing reports remain available for inspection.
- `tmp/exchange-loss-lazy-input/report.json`: killing a worker at join-consumer
  startup after upstream data exists still returns the exact million-row result;
  coordinator exchange files are cleaned up.
- `tmp/pressure-local-blocking-fixed/report.json`: all 11 local pressure cases
  pass, including cancellation after about 297 ms of active CPU work; admission
  recovers in about 93 ms. Logical budgets and sampled process RSS are separate.
- `tmp/pressure-two-workers-incremental/report.json`: all 10 distributed pressure
  cases pass, including 100,000 distinct groups under 256 MiB query pools with
  spill and cleanup. This is a fixture test, not a universal RSS guarantee.
- Native TLS has six passing checks; the native platform/Engine bridge has eight.
- `tmp/soak-two-workers-600s/report.json`: the September 5 binary passed a
  ten-minute mixed workload with 4,231 exact-result queries, 23 cancellation/queue
  cycles, a worker failure, bounded history and zero retained exchange files.
  This does not qualify later execution changes.
- September 8 verification: 32 storage, 14 optimizer, 89 server and 21 API tests
  pass; fresh native TLS (6) and actual HTTP platform bridge (8) checks pass.
- Subsequent September 8 combined workspace tests and strict all-target Clippy
  pass. Adaptive spill has 15 focused cases within 98 executor tests. API tests
  increased to 26 after fixing async-job owner isolation and deletion races.
- The integrated local Compose stack passes its health checks. A request through
  Studio's trusted proxy, API and two-worker Engine returns COUNT=3 and SUM=6
  from the generated local fixture (`tmp/local-stack-qualified/query-result.json`).
- `tmp/same-files-matched-million-r3/report.json`: the first opt-in parallel run
  fails five aggregate queries because final aggregate column names differ from
  the local planner's expected names. TopN passes. This run cannot support a
  speedup claim; an end-to-end regression and corrected run are required.
- `tmp/same-files-matched-million-r4-released/report.json`: after fixing aggregate
  output names/nullability and releasing consumed replayable results in the
  benchmark client, all six query types and 360 measured queries per engine pass.
  Both containers have 4 CPUs/8 GiB, concurrency four, and identical million-row
  files. Observed mixed-workload throughput is **23.43 queries/s for Kaveon** and
  **22.17 for Trino**, a **1.057×** ratio. The provisional **1.9×** target remains
  unmet. Join latency is still substantially higher; a point estimate from this
  workload is not proof of broad engine superiority.
- `tmp/same-files-matched-million-r5/report.json`: streaming joins and typed TopN
  pass the six-query diagnostic plus 360 measured requests per engine. Observed
  throughput ratio is 1.050×; this small difference does not demonstrate a
  meaningful aggregate gain over r4.
- `tmp/extended-five-million-r6-correctness/report.json`: all 12 extended queries
  pass on five million fact rows and 100,000 customers. Exact metadata COUNT and
  Delta row-group predicate propagation address two exposed costs. One measured
  sample after one warmup is diagnostic evidence only, not publication evidence.
  High-cardinality grouping and joins remain significant bottlenecks.
- `tmp/same-files-matched-million-r2/report.json`: six queries pass with both
  single-node release services limited to 4 CPUs and 8 GiB. Five measured runs
  follow one warmup. The geometric mean of Trino median latency divided by Kaveon
  median latency is **0.83**, improved from **0.70** in the first matched run but
  still below parity. No requested performance-superiority target is achieved.

Reports include executable/file hashes. The source tree remains under concurrent
development, so earlier executable results do not certify later changes. Temporary
reports are local artifacts; copy a reviewed, secret-free final report into durable
release evidence after combined validation.

## Active implementation

- Typed aggregate state, SQL window/subquery semantics, additional memory and
  spill accounting, admission lifetimes, bounded expression expansion.
- Coordinator-pinned Delta versions across splits/retries; classic and multipart
  checkpoint replay; S3/ADLS Parquet/Delta reads; Iceberg reader implementation.
- Authenticated result pages, bounded network receive, resource-group queues,
  native TLS, platform query/catalog bridge, credential-storage hardening.
- Statistics-based inner-join build-side selection with output-order preservation.
- Compact grouped-state transport and opt-in local parallel aggregation.
- Sustained mixed-workload soak and complete local Compose integration.

### Engine-backed virtual chart datasets

The Studio chart generator uses a server-resolved Engine flag only for virtual
datasets. Engine's distributed planner currently exposes derived-table output
columns without the derived-table relation alias, so generated outer projections
and groupings are unqualified for that path. Physical and general PostgreSQL
datasets retain normal relation aliases. This is a scoped compatibility path,
not a claim of general PostgreSQL or Trino derived-table alias parity; the
showcase wrappers require live Engine validation.

## External and performance gates

AKS and authenticated ADLS evidence now exists for earlier deployed images, but
the newest catalog recovery, native statistics and product-transaction changes
still require immutable-image rollout and repeat qualification. Do not infer
current deployment readiness from earlier digests or from in-memory object-store
tests.

Pending a user preference, the working metric is **1.9× throughput** on the
published workload. This differs from 90% lower latency or compute cost. A
matched comparison must keep the same data,
query semantics, topology/resource totals, result-consumption policy, and cache
policy. Publish failures and per-query measurements as well as aggregate results.

See [qualification commands](../../engine/qualification/README.md) and the
[benchmark protocol](../../engine/benches/README.md).
