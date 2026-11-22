# Engine public-availability gate — 2026-11-22

This is the single release tracker for moving Kaveon Engine from **Alpha** to
**Publicly available**. It separates evidence that is already reproducible in
local Docker from gates that require an approved, billable cloud qualification
run. A passing local gate does not silently imply cloud production readiness.

## Local gates — passed

- `cargo fmt --all -- --check`
- `cargo clippy --workspace --all-targets --release -- -D warnings`
- `cargo build --workspace --release --locked`
- `cargo test --workspace --release --locked -- --test-threads=1`
  (2,417 discovered test entries; all non-ignored tests passed; the server
  release suite passed 721/721)
- Two-worker Docker distributed smoke over 500,000 rows: 12/12 exact cases,
  zero mismatches, distributed stages observed.
- Docker exchange-loss recovery: a consumer worker was terminated after
  exchange materialization; the coordinator returned the exact join result and
  cleaned all exchange chunks. Evidence is in
  [`track1-docker-exchange-loss-2026-11-22.md`](track1-docker-exchange-loss-2026-11-22.md).
- Docker pressure/cancellation: grouped joins, sort spill, bounded windows,
  resource refusals, quota failure, and cancellation all passed with cleanup.
  Evidence is in
  [`track1-docker-pressure-cancellation-2026-11-22.md`](track1-docker-pressure-cancellation-2026-11-22.md).
- Native Docker soak: 231 queries over 50.25 seconds, intentional worker loss,
  cancellation/queue recovery, bounded history, retained-file cleanup, and
  RSS bounds all passed. Evidence is in
  [`track1-docker-soak-2026-11-22.md`](track1-docker-soak-2026-11-22.md).
- A fail-closed validator now checks these four reports against the exact Engine
  binary digest; see
  [`track1-local-release-validator-2026-11-22.md`](track1-local-release-validator-2026-11-22.md).
- Storage release suite: 158 passed, 0 failed, and 3 explicitly ignored
  Azurite-only tests under `--release --locked`. Evidence is in
  [`track1-storage-release-suite-2026-11-22.md`](track1-storage-release-suite-2026-11-22.md).
- Bracketed `DISTINCT` identifiers: local REST execution returns the exact
  typed result; evidence is in
  [`track1-bracket-distinct-runtime-2026-11-22.md`](track1-bracket-distinct-runtime-2026-11-22.md).
- Documentation integrity: `node scripts/validate-docs.mjs` passes.
- GitHub Actions run `45018913739` for commit `9d491fe9` passed its complete
  Check & Test job (format, Clippy, compatibility validation, and release test
  suite). Publish/Release jobs were correctly skipped for this manual
  qualification run; they do not count as a cloud deployment.

## Release gates — still open

1. Cloud ADLS data-plane qualification: reads, conditional commits, recovery,
   and fault injection against the deployment target.
2. Distributed endurance: sustained load, node loss, rolling upgrade/rollback,
   spill cleanup, and multi-coordinator recovery.
3. Identity and security: live credential rotation/revocation, tenant and
   catalog isolation, and adversarial authorization tests.
4. Compatibility and scale: broader SQL/DML corpus, external-client contract,
   high-cardinality joins/aggregates, and published latency/throughput SLOs.
5. Release operations: reproducible image/Helm artifacts, upgrade and rollback
   rehearsal, backup/restore evidence, and a signed release manifest.

These gates are not executable while AKS is stopped and the subscription's
current-month Cost Management query returns no usable rows. The fail-closed
budget record is [`cloud-budget-live-check-2026-11-22.md`](cloud-budget-live-check-2026-11-22.md).

## Budget boundary

The default qualification path is local Docker. No AKS, NAT gateway, VM, extra
worker, logging workspace, or second storage account may be started for this
goal without an explicit spend review proving the monthly ceiling remains below
`$100`. A future cloud run is limited to one `Standard_B2s` system node and at
most two workers, with teardown and a post-run cost check.

## Status

The Engine is **release-qualified for the declared local Docker scope** and is
not yet a cloud-production release. Keep the public maturity label scoped until
the five release gates above have measured evidence.
