# OpenSource sample data

`OpenSource` is the public analytical data catalog. It is separate from the
planned internal `Kaveon` product catalog and the existing `kavedb` synthetic
fixtures. PostgreSQL still owns the portal's mutable metadata.

The first AKS run completed in 5 minutes 10 seconds. All 11 tables passed exact
Engine row-count checks. It loaded 3,523,552 original taxi rows, retained
3,460,174 cleaned trips, and kept 63,378 rejected trips separately. WHO supplied
573,360 records for 240 countries/territories, dated January 4, 2020 through
July 19, 2026. Coverage describes the retrieved source, not current-day reporting.
See the [validation report](../engineering/opensource-validation-2026-09-09.json).
An authenticated live SQL Lab query returned 48,131 cleaned green taxi trips.

The extras import adds four verified tables: complete OWID energy data (23,377
rows), its 130-indicator codebook, 1,764 NASA GISTEMP global monthly anomalies,
and 4,576 Open LLM leaderboard results. The catalog now has 15 tables across
seven schemas. Its source URLs, pinned archive revisions, and checksums are in
`tmp/extras-manifest.json`; counts are recorded in
`tmp/extras-registration.log`. These are source snapshots for demonstrations,
not a claim of continuously refreshed energy, climate, or benchmark reporting.

## Sources and scope

- [NYC TLC trip records](https://www.nyc.gov/site/tlc/about/tlc-trip-record-data.page):
  the complete January 2025 yellow and green taxi files and taxi-zone lookup.
  This is one month, not the entire historical TLC archive.
- [WHO COVID-19 data](https://data.who.int/dashboards/covid19/data): the complete
  reported-cases/deaths CSV available at extraction time. The manifest records
  date coverage and a SHA-256 hash. Report dates describe surveillance reporting,
  not infection dates. WHO reporting frequency has changed over time.
- [OWID energy data](https://github.com/owid/energy-data): the full CSV and
  codebook at the commit pinned in the extras manifest.
- [NASA GISTEMP v4](https://data.giss.nasa.gov/gistemp/): global monthly
  land-ocean anomalies relative to the 1951-1980 baseline; `***` is null.
- [Open LLM Leaderboard](https://huggingface.co/datasets/open-llm-leaderboard):
  the pinned Parquet archive recorded in the extras manifest.

The sources are public datasets; no PostgreSQL production records are included.
Additional previously loaded datasets require their names/source inventory.

## Tables

| Schema | Tables | Meaning |
| --- | --- | --- |
| `bronze` | `yellow_trips`, `green_trips` | All source trip rows, with normalized Engine-compatible types |
| `silver` | `yellow_trips`, `green_trips` | Trips passing the documented checks |
| `silver` | `yellow_rejected`, `green_rejected` | Excluded trips with the first rejection reason |
| `reference` | `taxi_zones` | TLC location IDs and names |
| `covid` | `reported_cases`, `country_latest` | WHO time series and latest dated row per country |
| `gold` | `daily_trips`, `covid_reported_by_date` | Daily trip totals and sums of newly reported COVID counts |
| `climate_energy` | `energy`, `global_temperature_monthly` | OWID country/year indicators and GISTEMP monthly anomalies |
| `reference` | `energy_indicators` | OWID indicator codebook |
| `ai_benchmarks` | `open_llm_results` | Pinned Open LLM leaderboard archive |

Trip cleaning requires pickup during January 2025, drop-off at or after pickup,
and finite, nonnegative distance and total amount. These are explicit sample-data
rules, not a claim that every rejected trip is invalid. Dates are ISO strings;
trip monetary source fields remain doubles. Gold monetary totals round each
trip to integer cents using half-up rounding. Accepted plus rejected rows must
equal the bronze count. COVID negative corrections remain negative; missing
values remain null, including entirely unreported daily aggregate values.

Original files remain under `raw/`; the Engine tables reference curated Parquet
files. No Delta transaction log or transactional write support is claimed.

## Execution

The fixed test-cluster Job is
[`opensource-curation-job.yaml`](../../infra/aks/opensource-curation-job.yaml).
It runs the pinned image built from
[`Dockerfile.curate`](../../scripts/Dockerfile.curate) on an existing worker,
with 1 CPU requested, 2 CPU maximum, 4 GiB memory maximum, a one-hour deadline,
and temporary local storage. It does not scale the node pools.

Create `kaveon-opensource-scripts` from the two `curate-*.py` files and
`upload-curated-adls.py`. Supply a short-lived, authorized storage access token
through the `kaveon-opensource-upload` Secret's `token` key; never put the token
in source files, shell history, manifests, or logs. Apply the Job only in
`kaveon-test-aks`, namespace `kaveon`. Remove the temporary upload Secret after
successful upload. Engine queries use the existing read-only workload identity.

Outputs use the private ADLS container `opensource`, prefix
`snapshots/2026-09-09-v1`. Existing blobs with a different hash are refused.
To import changed upstream data, use a new snapshot prefix consistently in the
uploader and registrar, and review versioned catalog updates instead of
overwriting an active snapshot.

The Job emits credential-free `NYC_MANIFEST` and `COVID_MANIFEST` log records.
Save those as JSON and run
[`register-curated-catalog.py`](../../scripts/register-curated-catalog.py) in
the API environment with both file paths. It creates native catalog objects,
checks every table's count through the Engine, then publishes the platform
source record for SQL Lab. This is an explicit bootstrap workflow, not automatic
filesystem discovery.

## Query

Choose `OpenSource` in SQL Lab. Or start CLI 0.2.0 with
`--catalog OpenSource --schema silver`, then run:

```sql
SHOW SCHEMAS;
SHOW TABLES;
SELECT COUNT(*) FROM yellow_trips;
SELECT * FROM gold.daily_trips LIMIT 10;
SELECT country, cumulative_cases, cumulative_deaths
FROM covid.country_latest LIMIT 10;
```

The [product catalog assessment](../engineering/product-catalog-migration.md)
documents the separate transactional migration required before PostgreSQL can
be replaced.

## Local snapshot cache

Retain the registered public snapshot locally for repeatable tests without
copying credentials into the repository. Sign in with the Azure CLI, then run:

```powershell
python scripts/download-demo-snapshot.py
```

The downloader obtains a short-lived Storage token from the current Azure CLI
session without printing or saving it. It mirrors
`opensource/snapshots/2026-09-09-v1` to `data/opensource/2026-09-09-v1`,
preserves paths, verifies every `x-ms-meta-sha256`, skips matching files, and
refuses to overwrite a different local file. Its credential-free
`inventory.json` records local paths, byte counts, and checksums. Run it again
after a completed extras import to cache newly registered blobs.

If the workstation is outside the storage account's permitted network paths,
use the existing API pod as the authorized relay without changing storage
network rules:

```powershell
python scripts/download-demo-snapshot.py --via-aks --kubeconfig tmp/aks-kubeconfig
```

The caller's short-lived Azure Storage token is passed only over kubectl stdin.
The pod streams a verified tar archive to a local temporary file; the downloader
then checks every archive path, byte count, and checksum before placing files in
the cache. Neither the token nor the temporary archive is retained.
