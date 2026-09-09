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
example, a proposed OpenSource catalog would use:

```json
{
  "name": "OpenSource",
  "engine_catalog": "OpenSource",
  "storage_type": "adls_gen2",
  "storage_config": {
    "account": "kvtestegmf6oweugsno",
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

This is an example, not evidence that OpenSource data has been imported.

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

Existing bootstrap-managed catalogs such as `kavedb` and `OpenSource` have
`aks-*` Engine IDs, while new platform-managed sources use `platform-*` IDs.
They are already queryable and should not be recreated through Sync. A conflict
requires reviewing the source-to-Engine mapping; do not delete/recreate a live
catalog to suppress it.

## Register schemas and tables

Use the authenticated Engine catalog API:

1. `POST /v1/catalog/definitions/{catalog_id}/schemas` creates each schema.
2. `POST /v1/catalog/schemas/{schema_id}/tables` creates each table, including
   its Parquet location and column definitions.
3. Create objects as `Draft`, revision 1. Activate them using their object PUT
   endpoint, `If-Match: 1`, revision 2, and lifecycle `Active`.

Schema object updates use `/v1/catalog/schemas/{schema_id}`; table updates use
`/v1/catalog/tables/{table_id}`. Table locations are relative to the catalog's
container/root path. The working native ADLS example is
[`aks-kavedb-bundle.py`](../../scripts/aks-kavedb-bundle.py); it registers the
Engine objects directly, with its platform source seeded separately by the test
deployment. Its private credential bundle must not be committed or printed.

## Verify

Use CLI 0.2.0 or newer to check `SHOW CATALOGS`, `SHOW SCHEMAS`, `SHOW TABLES`,
and `DESCRIBE`. Run row counts and selected aggregates against source-system
expectations. In SQL Lab, select the catalog and verify all schema groups,
column types, and a real query.

Live AKS evidence currently covers the native `kavedb` ADLS registration,
schema/table/column discovery, workload-identity reads, and exact SQL results.
The platform synchronization bridge has automated tests. A complete generic
portal-only registration flow and the proposed PostgreSQL-to-OpenSource import
have not yet been validated end to end.
