# OpenSource product showcase

The showcase uses the `OpenSource` catalog in Kaveon Engine, backed by the ADLS
snapshot described in [OpenSource data](opensource-data.md). Saved datasets,
charts, and dashboard definitions currently remain in PostgreSQL. This showcase
does not complete the native transactional metadata migration.

## Open the portal

Sign in to Azure and select the test cluster, then keep this command running:

```powershell
kubectl --context kaveon-test-aks -n kaveon port-forward service/kaveon-portal 3000:3000 --address 127.0.0.1
```

Open `http://localhost:3000`, sign in with the approved Microsoft account, and open
**Dashboards**. Engine chart execution currently requires Analyst or Admin.

The [AKS validation record](../engineering/showcase-validation-2026-09-09.json)
contains the deployed image digests and live query evidence.

The initial snapshot collection covers NYC Taxi, WHO-reported COVID values, climate and energy,
and the archived Open LLM leaderboard. Each chart queries actual Engine data;
these are snapshot analyses rather than real-time feeds. Source caveats and units
appear in the dashboard descriptions and chart information tooltips.

## Recreate the canonical eight dashboards

The primary showcase is the exact API contract exported from the live Vercel
deployment: **8 dashboards and 70 charts**. The source-controlled contract is
[`vercel-live-dashboard-contract.json`](../../data/dashboard-templates/vercel-live-dashboard-contract.json).
The importer preserves dashboard names, chart membership, layout, filters,
chart types, query configuration, and visualization configuration. It changes
only legacy dataset and chart IDs so the definitions target physical
`OpenSource` Engine relations.

Eight supporting relations are exact table exports or deterministic rebuilds of
their saved dashboard contracts. `OpenSource.public.kaveon_events_dashboard` is
the one exception: it is a compact synthetic projection that reproduces the
dimensions and measures needed by the three Kaveon product dashboards. It is
showcase data and must not be described as customer telemetry or an exact copy
of Vercel event values.

Keep the Portal forward running, then perform the read-only preflight:

```powershell
python scripts/import-vercel-live-dashboards.py --portal http://localhost:3000
```

Inspect `tmp/vercel-live-dashboard-import.json`. Apply only when
`ready_to_apply` is `true`; the preflight checks all nine Engine relations and
does not create or delete anything.

```powershell
python scripts/import-vercel-live-dashboards.py --portal http://localhost:3000 --apply
```

The importer uses private source markers as idempotency keys. It first registers
datasets, upserts all 70 charts, upserts and exactly verifies all 8 dashboards,
and only then removes stale dashboards from this same managed marker namespace.
It never deletes an unmarked dashboard.

Run the API and real-browser qualification after every apply:

```powershell
python studio/qualification/vercel_live_dashboards.py --portal http://localhost:3000
```

Qualification requires all eight managed dashboards to be published, verifies
their exact saved structures and 70-chart membership, executes every chart
against Engine, and renders every dashboard without browser or API errors. Its
query IDs, row counts, and screenshot paths are written to
`tmp/vercel-live-dashboard-qualification.json`. An importer success without this
qualification is not showcase completion.

## Engine query boundary

The current Engine does not consistently resolve a derived table alias such as
`fact.pickup_date` over a saved aggregate SQL subquery. The showcase generator
uses unqualified columns for single-source Engine datasets. Engine datasets with
dimension joins must first be materialized into a single table. Other database
backends retain their normal qualification. Rankings sort by the projected measure alias because aggregate expressions
in `ORDER BY` also have an Engine resolution gap. General derived-table
alias and aggregate-sort support remain Engine compatibility tasks; the showcase is not evidence
of complete SQL or transactional parity.
