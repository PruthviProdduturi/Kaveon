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

The collection covers NYC Taxi, WHO-reported COVID values, climate and energy,
and the archived Open LLM leaderboard. Each chart queries actual Engine data;
these are snapshot analyses rather than real-time feeds. Source caveats and units
appear in the dashboard descriptions and chart information tooltips.

## Recreate the showcase

From a checkout with Python, Playwright, and Azure CLI available, first validate
all source SQL without creating objects:

```powershell
python scripts/seed-showcase.py --portal http://localhost:3000
```

Then create or refresh the managed collection with an authenticated Admin:

```powershell
python scripts/seed-showcase.py --portal http://localhost:3000 --apply
```

The script uses the portal's Entra configuration, keeps authentication in memory,
and verifies generated chart SQL before publishing each dashboard. Reruns update
only objects with the managed showcase description; name collisions with other
objects stop the operation. It does not delete other dashboards or source data.

Chart definitions are in `scripts/showcase-queries.json`. Each saved SQL dataset
uses schema-qualified names and an explicit dimension and measure. Dashboard
charts intentionally have independent scopes; cross-filtering is disabled for
these aggregate snapshots. Open a chart to inspect or edit its query configuration.

The seed result report contains dashboard IDs and validation counts, without
credentials or data files. Browser validation must additionally confirm all chart
tiles render and refreshing the dashboard succeeds before declaring the
showcase ready.

## Engine query boundary

The current Engine does not consistently resolve a derived table alias such as
`fact.pickup_date` over a saved aggregate SQL subquery. The showcase generator
uses unqualified outer columns for its single-source virtual datasets. Physical
table and join queries retain their normal qualification. Rankings sort by the projected measure alias because aggregate expressions
in `ORDER BY` also have an Engine resolution gap. General derived-table
alias and aggregate-sort support remain Engine compatibility tasks; the showcase is not evidence
of complete SQL or transactional parity.
