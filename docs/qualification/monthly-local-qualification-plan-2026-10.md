# Kaveon one-month qualification plan

This plan covers work that can proceed without an AKS subscription or live
ADLS account. It is deliberately separate from the cloud gates: local evidence
can make the release safer and faster, but it cannot claim cloud qualification.

## Current baseline

- Formatting, strict Clippy, CI, catalog tests, storage tests, and the local
  distributed reliability subset are green. Focused local coverage also passes:
  587 execution tests, 89 catalog tests, 154 storage tests, 45 transaction/DML
  tests, 25 aggregate/planner tests, 24 window/planner tests, and 4 TopN
  differential tests, all with zero failures. Catalog access and revision tests
  add 11 passing cases covering restart reload, stale-revision conflicts, default
  deny, grant/revoke visibility, and admin isolation. Snapshot consistency adds
  6 passing cases covering drift rejection, cache invalidation, stable Delta
  enumeration, and snapshot-pinned metadata counts. Resource-safety gates are
  also green: 8 resource-group tests, 18 result-cache tests, 14 cancellation
  tests, and 9 retry tests passed without failures. SQL compatibility coverage
  adds 97 passing array, map, and JSON distributed/wire differential cases.
  External-client compatibility adds 8 passing tests for Trino-wire ordering,
  prepared-statement ownership and binding, and safe paging metadata. Runtime
  statistics and source qualification adds 30 passing cases for exact stats,
  broadcast safety, Bloom/row-group pruning, source identity invalidation,
  context refresh, and directory-source scheduling. The current integrated
  branch also passes formatting, strict Clippy, the locked workspace check, and
  the locked release build. The complete server target then passed 611 tests with
  zero failures in 551.66 seconds, including the full differential sweep. The
  complete locked workspace test run also passed every package group: catalog 89,
  CLI 211, SQL 259, execution 587, server 611, storage 154, and the remaining
  workspace targets, with no reported failures.
- The 500k-row Trino differential is exact across the current 12-case corpus.- Docker result-cache lifecycle and Azurite conditional-write contracts are
  qualified locally.
- Cloud execution is unavailable while the Azure subscription is read-only and
  the ADLS data plane reports `The specified account is disabled`.

## Week 1 — correctness boundaries

1. **Distributed joins (Track 1, item 3).** Add larger local skew fixtures,
   repeated spill/repartition runs, and physical spill-device exhaustion tests.
   Acceptance: exact local/distributed results, bounded memory, zero retained
   spill files, and explicit fail-closed errors.
2. **Distributed aggregates (item 4).** Expand high-cardinality grouped and
   `COUNT(DISTINCT ...)` cases, including skew, NULLs, overflow, and repeated
   spill. Acceptance: exact reference hashes and no state loss or leaked files.
3. **Snapshot consistency (item 11).** Exercise concurrent catalog revision,
   retry, and source mutation races using local Parquet/Delta/Iceberg fixtures.
   Acceptance: every fragment sees the statement's pinned snapshot or fails
   closed; no mixed-version result is accepted.
4. **DML and transaction boundaries.** Extend typed product-record DML with
   rollback, conflict, visibility, malformed-input, and restart cases.
   Acceptance: committed data is durable and conflicted writes never become
   partially visible.

## Week 2 — execution efficiency and resource safety

1. **Memory admission and spill (item 7).** Run mixed joins, windows, TopN, and
   aggregates under bounded memory, cancellation, and spill quotas. Acceptance:
   no process OOM, correct rejection telemetry, deterministic cleanup, and
   repeatable peak-memory measurements.
2. **High-cardinality aggregate optimization.** Profile hashing, Arrow decode,
   allocation, exchange serialization, and final merge separately. Optimize
   only paths whose counters show a bottleneck; retain exact differential tests.
3. **Global ORDER BY/TopN (item 6).** Add pressure tests for ordered streaming,
   OFFSET, ties, NULL ordering, UTF-8, decimal, and mixed-key heaps.
   Acceptance: bounded coordinator memory and exact Arrow-reference ordering.
4. **Cancellation and retry (item 8).** Add concurrent cancellation stress,
   timeout/retry races, and long-running local federation cancellation.
   Acceptance: prompt fan-out, no orphan tasks, no retained exchange/spill
   files, and one terminal query state.

## Week 3 — SQL and connector breadth

1. **Windows (item 5).** Complete the local corpus for ranking/value functions,
   mixed keys, frames, modifiers, unpartitioned windows, and shared exchanges.
   Acceptance: exact local/distributed results and explicit unsupported-shape
   errors.
2. **Runtime filtering (item 12).** Qualify local Parquet, Delta, and Iceberg
   pruning with measurable file/row-group reductions, plus remaining decimal,
   temporal, and exchange type edges.
3. **Join reordering/skew policy (item 13).** Build a larger plan-regression
   corpus and compare cost choices under stale, missing, and exact statistics.
   Acceptance: no unsafe broadcast, deterministic plan choice, and exact
   results under byte-skew splitting.
4. **Connector federation (item 14).** Test authenticated passthrough, pushdown
   boundaries, source errors, and cross-source rejection behavior with local
   mock sources. Acceptance: credentials never reach SQL text and unsupported
   cross-source joins fail explicitly.
5. **External client and SQL compatibility.** Extend Trino-wire metadata,
   prepared statements, paging, cancellation, arrays/maps/JSON, and DML
   differential coverage.

## Week 4 — operability, performance, and release evidence

1. **Local upgrade/rollback (item 15).** Rehearse image, schema, catalog, and
   transaction-log upgrades using Compose and retained local snapshots.
   Acceptance: rollback restores a known-good binary and readable state.
2. **Local sustained soak (item 16).** Run multi-client mixed workloads for
   progressively longer windows with resource samples, worker loss, restart,
   cancellation, and cleanup checks. Acceptance: exact results, bounded
   memory, zero retained files, and no unexpected restarts.
3. **Performance comparison.** Re-run the fixed local Kaveon/Trino corpus with
   identical data, warm/cold cache labels, CPU/memory samples, and failure
   counts. Report per-query results; do not generalize from a single QPS.
4. **CLI and Studio release checks.** Verify install, local Docker startup,
   catalog/schema/table discovery, SQL paging, query cancellation, cache
   invalidation, and error rendering on a clean machine.
5. **Evidence consolidation.** Replace tracker rows with current acceptance
   evidence, archive raw reports, and run formatting, Clippy, workspace build,
   API/web CI, security scan, and documentation validation.

## Cloud-only gates to run after Azure recovery

These cannot be completed honestly with local substitutes:

- ADLS authenticated reads/writes, conditional commits, and injected object-
  store faults;
- AKS node loss, node drain retry, rolling upgrade, rollback, and sustained
  endurance;
- physical node-disk exhaustion and multi-coordinator failover;
- cloud snapshot/version qualification, workload identity, and remote
  connector pushdown;
- immutable image publication and the final deployed release evidence.

## Definition of month-end readiness

Local completion means every local item above has code, focused tests, a
repeatable command, and an evidence report, with no unexplained failures. It
does not mean the cloud release is complete. Cloud readiness additionally
requires the precondition script to return `ready: true` and every cloud-only
gate to have current evidence.
