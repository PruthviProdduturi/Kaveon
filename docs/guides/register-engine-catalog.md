# Register an ADLS catalog

A catalog contains schemas, which contain tables. Registering a catalog does not
copy PostgreSQL data or automatically discover every file in storage.

## Requirements

- Administrator access to the portal and Engine catalog APIs.
- An ADLS Gen2 container containing supported Parquet files.
- Engine workload identity with read access to that container.
- A column definition matching each Parquet table.

Use credential references, never storage keys or access tokens, in catalog
definitions. Azure role grants and the workload-identity configuration must
already exist; a reference alone does not grant storage access.

## Register the source

In the portal's Catalog Sources form, choose the native adapter, ADLS Gen2,
Parquet, a catalog name, account/container/root path, and workload identity
credential reference. Saving creates a platform source record in draft state.

The corresponding authenticated API is `POST /api/v1/catalog-sources`. For
example, a new catalog source would use its own name and storage location:

```json
{
  "name": "ExampleLake",
  "engine_catalog": "ExampleLake",
  "storage_type": "adls_gen2",
  "storage_config": {
    "account": "kvtesticmwwliihpppo",
    "container": "opensource",
    "root_path": ""
  },
  "data_format": "parquet",
  "adapter_type": "native",
  "adapter_config": {},
  "credential_kind": "workload_identity",
  "credential_ref": "kaveon-test-reader"
}
```

This is a registration shape, not a substitute for table registration or a
claim that a storage location is queryable.

## Synchronize with the Engine

Activate the platform source through
`POST /api/v1/catalog-sources/{source_id}/transition` with
`{"lifecycle":"active"}`, then call
`POST /api/v1/catalog-sources/{source_id}/engine-sync` with `{}` for its first
synchronization. Subsequent changes require the current Engine
`expected_revision` in the request body. The returned catalog includes its stable
Engine ID and revision. A conflict means reload and review before retrying.

System Settings shows a server-verified Engine connection and embeds catalog
source registration. The native-source Sync action calls this API explicitly;
it does not automatically activate the source or register tables. Merely seeing
an active source does not prove it is queryable.
The Engine catalog and the PostgreSQL platform source registry are separate
stores. Do not create a second same-named Engine catalog through a different
registration path.

`OpenSource` is a bootstrap-managed, queryable catalog with Engine ID
`aks-opensource`. It should not be recreated through the generic Sync flow. A
conflict requires reviewing the source-to-Engine mapping; do not delete and
recreate a live catalog to suppress it.

## Register schemas and tables

The shortest path is the CLI or a SQL script, which registers each table
only after the coordinator has read its metadata:

```bash
kaveon schema add ExampleLake.sales --server https://engine.example --ca-cert ./kaveon-ca.crt
kaveon table register ExampleLake.sales.orders --location sales/orders --format delta
kaveon table describe ExampleLake.sales.orders
```

or, in the shell or with `-f`:

```sql
CREATE SCHEMA IF NOT EXISTS ExampleLake.sales;
CREATE TABLE IF NOT EXISTS ExampleLake.sales.orders WITH (location = 'sales/orders', format = 'delta');
SHOW CREATE TABLE ExampleLake.sales.orders;
```

The columns come from the table's own metadata (a column list may be
declared instead); an unreadable location fails the statement and registers
nothing. The catalog itself can be created the same way (`kaveon catalog add`
or `CREATE CATALOG … WITH (storage = 'adls', …)`, admin role) when it is not
bootstrapped or synchronized from the platform. See the
[CLI guide](engine-cli.md#catalog-administration) and the
[API reference](../reference/api.md#catalog-statements).


From Studio, open Catalog and use **Add schema** and **Add table**; from the
platform API, `POST /api/v1/engine/catalog/definitions/{catalog_id}/schemas`
and `POST /api/v1/engine/catalog/tables` with names and Trino column types
(Editor role). Both register as Draft, activate, and verify the table with a
read before keeping it; see [Connectors](../reference/connector-capabilities.md#registering-catalogs-schemas-and-tables).

The manifest scripts below use the authenticated Engine catalog API directly
with the catalog-admin credential, which the same definitions also accept:

1. `POST /v1/catalog/definitions/{catalog_id}/schemas` creates each schema.
2. `POST /v1/catalog/schemas/{schema_id}/tables` creates each table, including
   its Parquet location and column definitions.
3. Create objects as `Draft`, revision 1. Activate them using their object PUT
   endpoint, `If-Match: 1`, revision 2, and lifecycle `Active`.

Schema object updates use `/v1/catalog/schemas/{schema_id}`; table updates use
`/v1/catalog/tables/{table_id}`. Table locations are relative to the catalog's
container/root path. A Parquet location may name one object or a directory
of Parquet files (the layout Trino and Spark write): the directory is listed
at query time, hidden `_`/`.` entries are skipped, and every file must carry
the first file's schema. See the Engine storage documentation for the rule. The current OpenSource bootstrap is registered by
[`register-curated-catalog.py`](../../scripts/register-curated-catalog.py) from
curation manifests. It contains `silver.yellow_trips`, `silver.green_trips`,
and `nyc_taxi.daily_trips`; the last has `pickup_date`, `service_type`,
`trip_count`, `total_amount_cents`, and `total_trip_distance`. Private
credential material must not be committed or printed.

## Restore after a cluster rebuild

The Engine catalog is SQLite on the coordinator volume. It is not in the
PostgreSQL snapshot, so a rebuilt cluster comes back with datasets, artifacts
and answers but an empty `/v1/catalog/definitions`. The 2026-09-14 westus2
rebuild was recovered this way, in this order:

1. Copy the lake prefix to the new account server-side (`azcopy copy
   https://<old>.blob.core.windows.net/opensource/snapshots/2026-09-09-v1
   https://<new>.blob.core.windows.net/opensource/snapshots --recursive`, signed
   in with the Azure CLI). Compare file counts and bytes on both sides before
   going on; 509 files and 6,951,933,217 bytes for the current snapshot.
2. Confirm the `kaveon-test-reader` identity holds Storage Blob Data Reader on
   the new account.
3. From a running API pod, with `KAVEON_LAKE_ADLS_ACCOUNT=<new account>`,
   register each catalog from its own manifest (the registrar intentionally
   rejects mixed-catalog invocations):
   `PYTHONPATH=/app python register-curated-catalog.py opensource-catalog-manifest.json`
   using [`infra/aks/opensource-catalog-manifest.json`](../../infra/aks/opensource-catalog-manifest.json).
   Then run `PYTHONPATH=/app python register-curated-catalog.py
   kaveon-catalog-manifest.json` using
   [`infra/aks/kaveon-catalog-manifest.json`](../../infra/aks/kaveon-catalog-manifest.json).
   The second manifest registers Kaveon-owned usage tables under
   `Kaveon.usage`; it reuses the same immutable ADLS objects and does not
   migrate or rewrite the backing files.
   After both registrations pass their exact row-count checks, run the Kaveon
   manifest once more with `--retire-legacy-kaveon`. That flag removes only the
   old `OpenSource.kaveon_product` definitions and the old
   `OpenSource.public.kaveon_events_dashboard` definition. It never deletes or
   copies a lake object and refuses to remove a table without its current
   revision. Then run the dashboard importer preflight/apply so the existing
   Kaveon Events dataset keeps its identity while pointing at `Kaveon.usage`.
   The script registers the catalog, every schema and table, runs `COUNT(*)`
   on each and refuses to update the platform source registry unless every
   count matches the manifest.
4. Run `scripts/qualify-dlm-questions.py --execute-live` through the portal;
   the corpus names its datasets, so the restored ids do not matter.

## Verify

Use CLI 0.2.0 or newer to check `SHOW CATALOGS`, `SHOW SCHEMAS`, `SHOW TABLES`,
`DESCRIBE` and `SHOW CREATE TABLE` (or `kaveon table show-create`). Run row counts and selected aggregates against source-system
expectations. In SQL Lab, select the catalog and verify all schema groups,
column types, and a real query.

Live AKS evidence covers OpenSource schema/table/column discovery,
workload-identity reads, exact row-count checks, and SQL Lab retrieval of
48,131 cleaned green trips. See the
[validation report](../engineering/opensource-validation-2026-09-09.json).

`aks-kavedb-bundle.py` is retained only as a legacy synthetic qualification
script. It is not the default catalog or a guide for a new registration.
