# Current authority and connector-language audit — 2026-11-04

## Scope

This audit prevents historical migration records from being mistaken for the
current Kaveon runtime contract. It covers the current reference, engineering,
and guide documentation outside `docs/archive/`.

## Current contract

- The qualified native profile uses KaveonDB for product metadata and
  transactional state; a live cloud deployment must still pass the runtime
  authority and restart gates before it can make the same claim.
- PostgreSQL is a legacy migration/rollback connector, not a native metadata
  authority.
- Fabric SQL/Azure SQL are registered passthrough sources. Kaveon executes the
  selected source request through the API connector boundary; Engine catalogs
  are not federated with those sources in one query.
- Historical PostgreSQL authority and cutover statements remain only in files
  explicitly labeled as historical transition records.

## Evidence

The current contract is stated in:

- `docs/reference/connector-capabilities.md`
- `docs/reference/api.md`
- `docs/reference/configuration.md`
- `docs/guides/register-engine-catalog.md`
- `docs/engineering/aks-test-deployment.md`
- `docs/guides/deploy-vercel-azure-postgres.md` (superseded runbook)

The historical records are explicitly labeled at their document headers in:

- `docs/engineering/postgresql-authority-inventory.md`
- `docs/engineering/product-catalog-migration.md`
- `docs/engineering/codex-continuation.md`

The audit search found no current, unlabeled claim that PostgreSQL is the
qualified native metadata authority or that Engine performs cross-source Fabric
federation. Historical mentions are retained for migration provenance and are
not deployment instructions. This language result does not override the
static dependency inventory or prove that a deployed API has cut over.

This language audit is not a PostgreSQL-retirement proof. The current static
dependency check (`python scripts/check-postgresql-dependencies.py`) still
reports active API call sites for catalog sources, data sources, datasets and
dataset semantics, charts, dashboards, favorites, saved queries, themes,
recents, query history, activity, context cache, DLM generation, and chat
history. Those paths include both legacy backfill readers and active
read/write services. They must be migrated or explicitly fenced before a
PostgreSQL-unavailable restart can be claimed.

## Verification

- `node scripts/validate-docs.mjs` — passed.
- `git diff --check` — passed.
- `python scripts/validate-tsql-compatibility-tracker.py` — passed.
- `python scripts/check-postgresql-dependencies.py` — completed; 16 authority
  families reported, with the active call sites listed above.

This audit does not claim cloud deployment state; live authority remains a
runtime configuration and evidence question.
