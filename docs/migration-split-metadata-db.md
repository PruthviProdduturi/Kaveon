# Migration: split platform metadata onto its own database

**Status:** runbook (not yet executed). The *visible* half — hiding platform
tables from SQL Lab — is already shipped. This document covers the *physical*
split, which needs a maintenance window and direct DB access, so it is deliberately
not run blind against production.

## Why

Today a single Azure Postgres database holds **both**:

- **Platform metadata** (control plane): `charts`, `dashboards`, `datasets`,
  `dataset_columns/dimensions/metrics`, `saved_queries`, `query_history`,
  `favorites`, `activity`, `local_users`, `user_recents`, `user_themes`,
  `auth_config`, `data_sources`, `context_snapshots`, `context_answer_cache`.
- **User / open-source data** (data plane): `covid_*`, `nyc_taxi_*`, …

Colocating them is wrong for three reasons: (1) analysts can see/query the
platform's internals in SQL Lab, (2) a heavy analytical query and the app's own
control-plane share one connection budget, (3) blast radius — a data-plane mistake
can touch control-plane rows. They should live in separate databases.

## What's already done (safe, shipped)

- **SQL Lab hides platform tables.** `routers/lab.py` filters
  `PLATFORM_METADATA_TABLES` out of `/lab/tables`, but only for the metadata
  database — a dedicated data source is returned unfiltered. So the moment the
  split below happens, nothing about SQL Lab needs to change.

## The architecture already supports the split

The metadata layer is *already decoupled* from the warehouse pool:

- `database/metadata.py` connects using `settings.METADATA_DATABASE` /
  `METADATA_DB_TYPE` (env `METADATA_DATABASE` / `METADATA_DB_TYPE`).
- `database/pool.py` (the warehouse pool) connects per `data_sources` row.

So splitting is: **stand up a metadata DB, copy the 17 tables into it, repoint
`METADATA_DATABASE`.** No application code change required.

## Runbook

Prereqs: psql access to the Azure Postgres server (admin login or an entra token),
a short maintenance window (write traffic paused or low).

```bash
# 0. Variables
SERVER=kaveon-db                     # Azure Postgres flexible server
RG=kaveon-rg
DATA_DB=<current db name>            # holds everything today
META_DB=kaveonmeta                   # new metadata-only database

# 1. Create the new (empty) metadata database — additive, reversible.
az postgres flexible-server db create \
  -g $RG -s $SERVER --database-name $META_DB

# 2. Apply the schema to the new DB.
psql "host=$SERVER.postgres.database.azure.com dbname=$META_DB sslmode=require user=<admin>" \
  -f apps/kaveon-api/schema_postgresql.sql

# 3. Copy the 17 platform tables (data + sequences) from DATA_DB -> META_DB.
#    Dump only the platform tables, then restore into META_DB.
PLATFORM_TABLES="activity auth_config charts context_answer_cache context_snapshots \
dashboards data_sources dataset_columns dataset_dimensions dataset_metrics datasets \
favorites local_users query_history saved_queries user_recents user_themes"

TFLAGS=$(for t in $PLATFORM_TABLES; do echo -n " -t $t"; done)
pg_dump "host=$SERVER.postgres.database.azure.com dbname=$DATA_DB sslmode=require user=<admin>" \
  --data-only --no-owner $TFLAGS \
  | psql "host=$SERVER.postgres.database.azure.com dbname=$META_DB sslmode=require user=<admin>"

# 4. Repoint the API at the metadata DB (Container App env).
az containerapp update -n kaveon-api -g $RG \
  --set-env-vars METADATA_DATABASE=$META_DB METADATA_DB_TYPE=postgresql

# 5. Verify: app loads, dashboards/charts list, SQL Lab shows ONLY data tables,
#    context/build + context/validity still work. Check row counts match:
#    SELECT count(*) FROM charts;  (in META_DB) == old count in DATA_DB.

# 6. Only after verification — drop the platform tables from DATA_DB so the data
#    plane is clean. KEEP a backup/snapshot first.
#    (Reversible until this step: to roll back, set METADATA_DATABASE back to DATA_DB.)
```

## Rollback

Until step 6, rollback is a single env flip:
`az containerapp update ... --set-env-vars METADATA_DATABASE=$DATA_DB`. The original
tables are untouched in `DATA_DB`, so no data is lost.

## Notes

- The warehouse `data_sources` row for the open-source data must point at
  `DATA_DB` (the data plane). Confirm it does not point at `META_DB`.
- `query_history.database_name` values are unaffected (they name data sources, not
  the metadata store).
- If a fully separate server is preferred over a separate database on the same
  server (stronger isolation, independent scaling), swap step 1 for a new flexible
  server and adjust the host in `METADATA_*` connection settings accordingly.
