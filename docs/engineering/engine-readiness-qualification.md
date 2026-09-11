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

The evidence-backed score for the current integrated direction is **80/100
(8.0/10)**. This is a readiness estimate for the declared Engine scope, not a
feature-parity score against Trino or PostgreSQL. The threshold is met narrowly;
the remaining boundaries still prevent a broad production-readiness claim.

| Area | Points | Evidence credited | Evidence still withheld |
|---|---:|---|---|
| SQL correctness and types | 22/25 | 37-case differential suite, 12-query extended same-file suite, typed aggregates/windows/subqueries/set operations, explicit unsupported errors, native `ANALYZE` | Re-run the complete differential corpus on the integrated release image; broaden decimal, timestamp, nested-type, DML and randomized coverage |
| Memory and spill | 12/15 | Admission, bounded result/history/exchange paths, aggregate spill and a clean-current-image two-worker pressure run covering 11 spill, rejection and cancellation cases | Complete retained-result/worker-response accounting, successful join spill under skew, current-image AKS disk exhaustion and sustained pressure |
| Distributed execution and recovery | 14/15 | Multi-worker operators, immutable query catalog pinning, authenticated worker recovery, clean local consumer-loss recovery, and current-image AKS evidence for 12/12 concurrent exact queries plus forced in-flight worker loss, alternate-worker attempt-1 retry, exact output and three-worker catalog-compatible recovery | Node drain, catalog mutation during a running query, and multi-coordinator consistency |
| Authentication and platform integration | 13/15 | Entra/TLS, owner-bound results and transactions, role checks, exchange authentication, fail-closed secrets/configuration | Live rotation/revocation, tenant isolation and adversarial authorization qualification on the release deployment |
| Storage correctness | 8/10 | Parquet/Delta/Iceberg readers, pinned source identities, authenticated ADLS reads, CAS-backed immutable product/statistics documents | Live ADLS conflict/fault/restart evidence, broader schema evolution and corruption/recovery qualification |
| Operability | 8/10 | Reproducible CLI/images, Helm/Bicep, health/readiness/metrics/history, immutable ACR build and current-digest AKS rollout, startup stale-exchange reconciliation, and a passing combined restart/fault/pressure gate | Backup/restore, upgrade/rollback, alerting and a sustained current-image soak |
| Performance | 3/10 | Resource-matched harness, exact-result hashes, fail-closed claim evaluators; best recorded six-query diagnostic ratio is 1.057× | Run the publication-scale extended corpus on immutable current images; the required 1.90× Trino throughput result and PostgreSQL transaction comparison do not exist |

Three material boundaries prevent expanding this narrow 8/10 rating:

1. The current image has passed bounded coordinator restart, catalog catch-up,
   concurrent correctness, pressure and worker-loss recovery, but not a sustained
   mixed-workload soak, backup/restore or upgrade/rollback qualification.
2. Kaveon's transactional surface is a durable, typed product-record protocol.
   Arbitrary relational row DML, parameter binding, constraints/indexes,
   multi-table SQL transactions and a qualified isolation level remain absent.
   The API now also has a PostgreSQL connection-pinned unit of work and durable
   source-outbox schema; dataset parent/child writes use it atomically in code.
   There is no deployed schema, replay execution, backfill, reconciliation,
   restore or cutover evidence, so it earns no PostgreSQL-replacement readiness
   credit yet.
   A bounded deterministic dataset snapshot/reconciliation implementation now
   exists, but it has no real-environment artifact and does not change this
   evidence assessment.
   The operator command defaults to dry-run and requires a separate enable
   variable for target writes; it has not been executed for readiness evidence.
   The credential-free retirement parity audit likewise only validates supplied
   evidence. It fails closed over all 16 maintained authority families but does
   not produce live reconciliation, schema-discovery, shadow-read, fencing,
   restart, backup/restore or rollback evidence, so it earns no score by itself.
   The disabled credential-free collector only verifies and assembles local
   family reports with explicit source/target provenance; it does not execute a
   PostgreSQL or KaveonDB comparison and likewise earns no readiness credit.
   The static cutover dependency check classifies current Python database call
   sites against all 16 authority families. It catches new unclassified direct
   references but does not prove deployed runtime or dynamic-SQL coverage, so it
   is review evidence and adds no replacement readiness score by itself.
   Dataset authenticated point reads and lists up to 25 records now have a
   disabled bounded shadow comparator that preserves PostgreSQL responses and
   emits hash-only aggregate parity telemetry. It has not run in a live
   environment, covers no internal read or write path, and earns no readiness
   score yet.
   Dataset mutations also have a disabled post-commit observer that separates
   pending outbox replay from applied target divergence without changing source
   responses. It has no live observations and does not execute replay or fence
   writes, so it earns no readiness score.
   Authenticated chart point reads now have the same default-off, bounded,
   response-preserving hash comparison. No chart migration pipeline or live
   evidence exists, so it earns no readiness score.
   A typed durable DLM definition now pins a dataset ID and revision with owner
   isolation and referential validation. No writer, backfill, shadow evidence or
   generated-run publication exists, so this foundation earns no readiness
   score by itself.
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

Engine commit `c88f7ed` is deployed from ACR build `ca1u` as immutable digest
`sha256:1b41e38c56cb4fff74f17aa3c67ea599d6c3a6adfa4df55dede984ebc1d8d50a`.
The coordinator restart removed 15 abandoned valid exchange directories and
eight chunk files while preserving byte-identical result-named, malformed-name
and unrelated PVC canaries. The same digest then passed the combined AKS
worker-loss, concurrency and pressure gate. The exact next gates are: (1) run
the extended matched Trino publication gate; (2) run a sustained mixed-workload
soak plus backup/upgrade recovery; (3) qualify the declared product-record
transaction scope separately. Re-score only from those artifacts.

#### Current-image AKS restart and fault evidence — September 10

The credential-free report
`engine-aks-fault-pressure-validation-2026-09-10-pass-c88f7ed.json` records all
six top-level checks passing. Twelve concurrent known-result queries and twelve
bounded-pressure grouped queries returned exact hashes. During a four-stage
3,412,043-row taxi join, the verifier force-deleted `kaveon-worker-2`; stage 2
partition 2 retried as attempt 1 on `kaveon-worker-0` and returned exact result
`(3412043, 6077935)`. The replacement restored three active, catalog-compatible
workers. Coordinator exchange and all worker spill file counts were zero before
and after, no unrelated pod restarted, and sampled Engine memory remained below
pod limits. The separate cleanup artifact records the coordinator PVC namespace
isolation checks. This is bounded failure evidence, not coordinator HA or a
sustained-load claim.

#### Current-image local fault evidence — September 10

Commit `819f9777c22131c2727ed33621f00d6757df7ff9` was built in release mode
from a clean Engine tree. Both runs used binary SHA-256
`626ace17d3c36c4a75c8f32598fddab482d333fb965ca1e4bf1ec280b072336b`:

- The two-worker 256 MiB pressure gate passed all 11 cases: grouped spill,
  join, sort spill, TopN, set operation, bounded window, skew/window/repeat/disk
  quota rejection, and active-window cancellation. The grouped case observed
  91 spill files at peak and cleaned them all.
- The deterministic exchange-loss gate killed the consumer after eight
  producer chunks, then returned the exact million-row result
  `(1000000, 499500000)` and left zero exchange files.

The reviewed machine-readable reports are checked in as
`engine-pressure-validation-2026-09-10.json` and
`engine-exchange-loss-validation-2026-09-10.json`. The attempted current-image
Trino differential run did not start because the pinned reference service was
offline and Docker Desktop's Linux Engine pipe was unavailable. It produced no
SQL result and earns no correctness credit.

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
