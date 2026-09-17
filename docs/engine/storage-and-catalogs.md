# Storage and catalogs

Arrow `RecordBatch` is the execution unit. Storage exposes streaming readers
with strict projection validation: unknown, empty, or duplicate columns
fail. Storage predicates conservatively eliminate Parquet row groups on
footer statistics (including byte-array statistics a writer marked inexact,
which the format still defines as bounds) while execution filters preserve
row-level correctness. Narrow integers are presented at computing widths and
dictionary-encoded string columns are carried as dictionaries end to end,
so predicates, functions and group keys run once per dictionary value.

## Formats

| Format | Where | What is read | Refused by name |
|---|---|---|---|
| Parquet | local disk, ADLS Gen2 (`abfss://`), S3 (`s3://`, not qualified) | One object per table; projection, row-group pruning, parallel decoder lanes for large ADLS objects (`KAVEON_SCAN_PARALLELISM`), full-object and decoded-batch caches under the pinned identity | — |
| Delta Lake | local disk, ADLS Gen2 | The snapshot at one pinned version from the JSON commits and v1 checkpoints (classic and multipart); active files become deterministic scan partitions; the version is pinned per query so retries and joins read the same snapshot. This is the multi-file table on object storage today: TPC-H SF100 is generated as Delta for both engines | Reader protocol v2 (column mapping, deletion vectors, table features), v2 checkpoint sidecars, unsupported logical types |
| Iceberg | local disk, ADLS Gen2 | v1/v2 snapshots from an immutable metadata JSON pointer with field-ID projection and type promotion | Delete manifests, equality/position deletes, encrypted tables, name mapping |
| Parquet directory | — | Not a table yet: a Parquet table definition names one object, and a directory of Parquet files with no Delta log is not a table for the Engine. Directory tables are in progress | — |

Telemetry measures file and row-group selection, compressed bytes, output,
footer/read/snapshot time, lane count with the lightest and heaviest lane,
and throughput.

## Cloud read boundary

An ADLS Parquet scan pins the object identity returned by `HEAD` (ETag or
version, together with size) before loading the footer. Footer, object
metadata, and decoded batches are cached only under that identity. A range
read uses the same identity with conditional `GET`; replacement of an
object therefore fails closed and invalidates the metadata cache instead of
returning mixed data. Parquet range requests are bounded to 16 in flight per
reader. Large immutable objects may use the bounded full-object cache (64
MiB per object, 256 MiB per process); the decoded-batch cache holds 256 MiB
per process. Exact source statistics are keyed by the pinned object identity
or Delta snapshot version; if identity or statistics cannot be established
the reader returns an error rather than guessing.

Credentials come from provider chains, never URIs: on AKS the pod's
workload identity (`AZURE_CLIENT_ID`, `AZURE_TENANT_ID`,
`AZURE_FEDERATED_TOKEN_FILE`); for development the Azure CLI. The same
object-store reader builds an S3 client from the environment; no S3 bucket
has been read in qualification.

## Catalogs

The native SQLite/WAL catalog provides transactions, migrations, stable IDs,
optimistic revisions, lifecycle validation, structured Arrow schemas,
credential references, and audit history. Secrets are forbidden in
definitions. Workers execute coordinator-resolved fragment sources rather
than consulting mutable local catalogs; a published catalog snapshot has an
identity that every query record and result-cache key carries.

The platform PostgreSQL source registry and the Engine catalog are separate
stores, connected by the native catalog synchronization API
(`POST /api/v1/catalog-sources/{id}/engine-sync`, revision-aware). Saving a
platform source does not import data or register its tables; registration
scripts (`scripts/register-curated-catalog.py`,
`register-clickbench-catalog.py`, `register-tpch-catalog.py`) register
tables from a manifest and verify each `COUNT(*)` against it. See the
[registration guide](../guides/register-engine-catalog.md).

The product transaction store (`KAVEON_PRODUCT_TRANSACTIONS_ENABLED`) commits
typed product records to ADLS with conditional writes; it is a bounded
metadata protocol, not general DML (see the [ADLS transaction
protocol](../engineering/adls-transaction-protocol.md)).
