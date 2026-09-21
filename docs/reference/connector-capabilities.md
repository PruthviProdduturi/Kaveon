# Connectors

Kaveon reads data in two ways. KaveonDB, the Engine, reads lake tables where
they live — Delta, Iceberg and Parquet on ADLS Gen2, local disk or S3 —
through catalogs it holds itself. The platform API also connects to SQL
systems through drivers for SQL Lab, datasets and the DLM. This page says,
for each, what is implemented, what is qualified on the AKS cluster, and how
to register it: from the API, from Studio, and from SQL DDL and the CLI.

"Implemented" means the code is in the shipping path. "Qualified" means it has
been run end to end on the test cluster against real data and recorded under
`docs/qualification/`. Neither implies that every deployment has the
credentials, network access or drivers configured.

## Lake formats on KaveonDB

| Format | Location | Reads | Not supported | Status |
|---|---|---|---|---|
| **Delta Lake** | A table directory holding `_delta_log/` | The snapshot at one pinned version, from the JSON commits and v1 checkpoints (classic and multipart). Active files become deterministic scan partitions; the version is pinned per query so retries and joins read one snapshot. `COUNT(*)` and planning statistics come from the pinned snapshot, not a scan. | Reader protocol v2: column mapping, deletion vectors and other table features; v2 checkpoint sidecars; unsupported logical types. These are refused by name, not read partially. | Implemented; qualified on ADLS Gen2 (TPC-H SF100 is Delta on the cluster) and run on local disk in the Docker stack. |
| **Iceberg** | A table directory holding `metadata/` | v1 and v2 snapshots from an immutable metadata JSON pointer, with field-ID projection and type promotion; the snapshot is pinned per query. | Delete manifests, equality and position deletes, encrypted tables, name mapping. | Qualified on local storage 2026-09-21 (`docs/qualification/iceberg/local-2026-09-21.md`: a pyiceberg v2 table with appends and a delete, seven statements against its Parquet twin); the ADLS Gen2 read uses the object-store path the Delta reads are qualified on but has not yet run against a table on the cluster. |
| **Parquet** | One file, or a directory of Parquet files | A single object; or a directory in the Hive and Spark layout (what Trino writes), listed once at query time. Hidden entries — any name, or any directory below the root, beginning with `_` or `.`, and zero-byte objects — are skipped; every other object ending in `.parquet` or with no extension is data; anything else is an error naming the object. The first listed file's schema is the table's; every other file must carry the same names, order and types. `key=value` directories are partition columns: Hive-decoded, `__HIVE_DEFAULT_PARTITION__` as NULL, typed by inference (bigint, date, else varchar) or by the table's column list with `partitioned_by = ARRAY['dt']`, appended after the file columns, and pruned by the scan predicate before any file is opened (`files_pruned_by_partition` on the query record). `COUNT(*)` comes from footers. | A file with a differing schema is an error, never cast; a mixed layout (files under different keys or depths) and a key that is also a file column are errors naming the file. | Implemented; qualified for single objects on ADLS Gen2 (ClickBench `hits` is one object of 99,997,497 rows) and run on local disk in the Docker stack; directories and partition columns are implemented with the rule above and covered by the differential sweep on local disk and the in-memory object store, not yet run against a partitioned layout on the cluster. |

The columns declared on a table definition are the Arrow schema KaveonDB
reads the table with. There is no schema inference through the catalog API:
registration names every column, and the verification read catches a
mismatch between the definition and the files.

## Storage

| Storage | Location fields | Credential | Status |
|---|---|---|---|
| **ADLS Gen2** | `account`, `container`, `root_path` | Workload identity on AKS (`AZURE_CLIENT_ID`, `AZURE_TENANT_ID`, `AZURE_FEDERATED_TOKEN_FILE`), or the Azure CLI for local development; the definition carries a credential *reference* (`workload_identity` + a name), never a key. Object identity (ETag or version, with size) is pinned before a footer is read; a replaced object fails closed rather than returning mixed data. | Implemented and qualified. |
| **Local disk** | `base_path` | None. | Implemented and qualified (Docker Compose stack, `KAVEON_DATA_DIR`). |
| **S3** | `bucket`, `region`, `prefix` | The AWS environment provider chain. | Implemented in the same object-store reader; **not qualified** — no S3 bucket has been read in qualification. |

Table locations are relative to the catalog's container or root path
(`benchmarks/tpch/sf100/lineitem`, `clickbench/hits.parquet`), never URIs.
Credentials live in the Engine's environment; a definition references them by
kind and name, so nothing secret enters a catalog.

## Registering catalogs, schemas and tables

KaveonDB keeps durable definitions with stable ids, optimistic revisions and a
`Draft` → `Active` lifecycle, and publishes only `Active` definitions into its
query snapshot. Every path below registers as Draft, activates, and verifies
a table by reading it once; a table KaveonDB cannot read is removed again with
the storage error returned as the Engine reported it.

### From the platform API

The platform API (`/api/v1/engine/catalog`) speaks names and Trino types and
applies the platform's roles: **Admin** registers catalogs, **Editor** or
above registers schemas and tables, every role reads definitions. It talks to
the Engine over the server-side bridge with the catalog-admin credential; the
browser never holds it.

| Method | Path | Role | Does |
|---|---|---|---|
| `GET` | `/engine/catalog/definitions` | Viewer | Every catalog definition, in every lifecycle state |
| `POST` | `/engine/catalog/definitions` | Admin | Register a catalog: creates the platform source record, activates it and synchronizes it to the Engine in one call |
| `GET`, `POST` | `/engine/catalog/definitions/{catalog_id}/schemas` | Viewer, Editor | List schemas; add one (Draft → Active) |
| `DELETE` | `/engine/catalog/schemas/{schema_id}` | Editor | Remove an empty schema; `If-Match: <revision>` required |
| `GET` | `/engine/catalog/schemas/{schema_id}/tables` | Viewer | List table definitions |
| `POST` | `/engine/catalog/tables` | Editor | Register a table: Draft → Active → `SELECT COUNT(*)` with the result cache bypassed → removed again on failure |
| `GET`, `PUT`, `DELETE` | `/engine/catalog/tables/{table_id}` | Viewer, Editor, Editor | Read; revision-replace; remove — `PUT` and `DELETE` require `If-Match: <revision>` |

A table body names the schema by its id and the columns in Trino type names
(`bigint`, `integer`, `smallint`, `tinyint`, `double`, `real`, `boolean`,
`varchar`, `varbinary`, `date`, `timestamp`, `timestamp(3)`, `decimal(p, s)`)
or as Arrow type names and values:

```json
{
  "schema_id": "aks-benchmarks-tpch_sf1",
  "name": "region",
  "location": "tpch/sf1/region",
  "format": "Delta",
  "columns": [
    {"name": "r_regionkey", "type": "bigint", "nullable": false},
    {"name": "r_name", "type": "varchar"},
    {"name": "r_comment", "type": "varchar"}
  ]
}
```

A successful `POST` returns the active definition and the verification —
`{"probe": {"rowCount": 5, "elapsedMs": 12, "queryId": "…"}}`. An unreadable
table returns `422` with `{"code": "table_unreadable", "message": "<the
Engine's error>", "removed": true}`. `verify: false` skips the read, for a
location that is known to be empty for now. A catalog body gives the name,
the storage (`{"type": "adls_gen2", "account", "container", "root_path"}`,
`{"type": "local", "base_path"}` or `{"type": "s3", "bucket", "region",
"prefix"}`) and an optional credential reference.

The Engine's own catalog API (`/v1/catalog/definitions` and below, on the
coordinator) accepts the same definitions with the catalog-admin bearer
token, an actor header and `If-Match` revisions; the registration scripts
under `scripts/` use it directly. See the [HTTP API reference](api.md).

### From Studio

Catalog → **Add schema** on the overview, **Add table** on a schema page.
The sheet asks for the catalog, the schema, the name, the location relative
to the catalog root, the format, and the columns one per line
(`order_id bigint not null`). **Verify and add** registers the table,
reads it once on KaveonDB and shows the row count and elapsed time — or the
storage error verbatim, with the definition already removed. **Remove** on a
table page takes the definition out of the catalog after a confirmation; the
files are not touched. Editors and Administrators see the actions; Viewers
and Analysts see the catalog read-only. A catalog itself is registered under
Settings → Storage and synchronized to KaveonDB from there.

### From SQL DDL and the CLI

SQL DDL for schemas and tables, and the CLI commands that drive it, are being
added to the Engine; they register the same definitions through the same
lifecycle. See the [Engine CLI guide](../guides/engine-cli.md) for the
statements as they land and their status. The CLI's `SHOW CATALOGS`, `SHOW SCHEMAS`, `SHOW TABLES` and
`DESCRIBE` read what any path registered.

A partitioned Parquet directory registers with its keys as columns whether
or not they are declared: `CREATE TABLE sales WITH (location = 'sales',
format = 'parquet')` infers them from the paths, and `CREATE TABLE sales (id
BIGINT, dt VARCHAR, region VARCHAR) WITH (location = 'sales', format =
'parquet', partitioned_by = ARRAY['dt', 'region'])` names them and types
them from the column list. The declared keys must be exactly the path's keys
in their order (`400 TABLE_NOT_READABLE` otherwise), `partitioned_by` is
refused for Delta and Iceberg (their partitioning is in their own metadata),
and `SHOW CREATE TABLE` renders the option. The `/v1/catalog/*` service path
carries no partition declaration; a table registered there is partitioned as
its paths say and typed by inference.

## SQL sources on the platform API

These connect through the platform API's drivers for SQL Lab, datasets, charts
and the DLM. They are not KaveonDB catalogs; a query runs on the source.

| Source | Studio picker | API driver | Authentication | DLM notes |
|---|---|---|---|---|
| Fabric SQL analytics endpoint | Current | `pyodbc` | `DefaultAzureCredential` token | PostgreSQL-specific statistics and HLL profiling are unavailable |
| Fabric SQL warehouse | Current | `pyodbc` | `DefaultAzureCredential` token | Same limitation |
| Azure SQL | Current | `pyodbc` | `DefaultAzureCredential` token | Same limitation |
| PostgreSQL | Current | `psycopg2` | Password or configured Azure token path | Full profiler path |
| StarRocks | Current | `pymysql` (MySQL protocol) | Username and password | Manifest and precomputation may work; the PostgreSQL statistics and HLL path is unavailable |
| MySQL and MariaDB | API only | `pymysql` | Username and password | Not in Studio's source picker |
| Trino | Registration only | **Target** | None implemented | No executable driver |

## Boundaries

- A platform query executes against one selected source or one KaveonDB
  catalog. Cross-source federation is not implemented.
- `POST /api/v1/data-sources/{id}/test` is a stub; first-run setup and the
  metadata-admin probe endpoints perform real connection checks. Table
  registration on KaveonDB is verified by a real read.
- Connection strings for PostgreSQL, MySQL and StarRocks may contain
  credentials and are stored plaintext in `data_sources`; API responses
  suppress the field. Vault-backed storage is target work. KaveonDB catalogs
  hold credential references only.
- Fabric and Azure SQL require ODBC Driver 18 in the API runtime.
- SQL Lab lists KaveonDB catalogs in its source picker where
  `KAVEON_ENGINE_URL` is configured.

See the [Engine storage and catalogs manual](../engine/storage-and-catalogs.md),
the [registration guide](../guides/register-engine-catalog.md), the
[data-source guide](../guides/data-sources.md), [configuration](configuration.md)
and [security](../../SECURITY.md).
