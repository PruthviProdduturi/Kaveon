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
| Parquet directory | local disk, ADLS Gen2, S3 | A directory of Parquet files (the Hive/Spark layout) listed once per scan under one visibility rule, files spread over the scan partitions by size, one schema checked file by file; `key=value` directories are partition columns (Hive default partition is NULL, types inferred or declared) read as constant columns and pruned by the scan predicate before any file is opened | A mixed layout (files under different keys or depths), a key that is also a file column, a stray file of another extension |

Telemetry measures file and row-group selection, compressed bytes, output,
footer/read/snapshot time, lane count with the lightest and heaviest lane,
and throughput.

## Directory Parquet tables

A `Parquet` table's location may be one object or file, or a directory of
Parquet files — the Hive/Spark layout, and what Trino writes (`sf100/region/`
holding N files). The reader decides at open time: a `HEAD` that finds an
object reads that object as before; ADLS answers a directory `HEAD` with
`x-ms-resource-type: directory` (surfaced as not-found) and S3 has no object
at a prefix, and either is followed by one recursive listing of the prefix.
A local path that is a directory is listed the same way through the
`object_store` local filesystem, so both paths share one rule:

- Hidden, skipped: an object whose name, or any directory below the root,
  begins with `_` or `.` (`_SUCCESS`, `_delta_log/…`, `_temporary/…`,
  `.part-….crc`). A zero-byte object holds no rows and is skipped too.
- Data: every other object whose name ends in `.parquet` (any case) or has
  no extension at all (Trino's file names).
- Anything else (`README.md`, `part-0.orc`) is an error naming the object, so
  a stray file is neither read as data nor silently dropped from the table.
- The listing is sorted by path. Every directory between the root and a
  data file must be a `key=value` segment; those keys are the table's
  partition columns (next section). A directory that is not one is an
  error naming the file under it.

The schema is the first listed file's with the partition columns appended,
projected in the query's column order. Every other file is checked against
it — same names, order and types; a file may declare a column non-nullable
where the first declares it nullable, never the reverse — and a difference
is an error naming both files. No file is cast to another's schema.

### Partition columns

A Hive-partitioned directory (`sales/dt=2026-09-01/region=eu/part-0.parquet`,
what Hive, Spark and Trino write) carries columns in its paths. The rule,
applied while listing, before any file is opened:

- Every directory between the root and a data file is one `key=value`
  segment. Keys and values are Hive-decoded (`%2F` is `/`, `%25` is `%`; a
  `%` that no two hex digits follow stands for itself); the value
  `__HIVE_DEFAULT_PARTITION__` is NULL. Every file must carry the same keys
  in the same order — a file at another depth, or under other keys, is an
  error naming that file and the first listed one — so a flat directory
  and a partitioned one are never mixed silently.
- A key's type is inferred from its non-null values: `bigint` when every
  value is a canonical integer (`7`, `-12`; not `007` or `+1`), `date` when
  every value is `YYYY-MM-DD`, else `varchar` (a key with only NULLs is
  `varchar`). The table definition can declare the type instead: `CREATE
  TABLE sales (id BIGINT, dt VARCHAR, region VARCHAR) WITH (location =
  'sales', format = 'parquet', partitioned_by = ARRAY['dt', 'region'])`
  stores `dt` as text, and a scan reads the key as the type the catalog
  serves for that column. A value that does not read as the declared type
  (`dt=2026-09-01` under `bigint`) is an error naming the file. Declared
  keys must be exactly the path's keys in their order; a location whose
  paths carry keys is recorded as partitioned whether or not `partitioned_by`
  is given, and `SHOW CREATE TABLE` renders the option.
- The partition columns come after the file columns in the table schema
  and are nullable. A key that is also a column inside a file is an error
  naming the file: a column has one source. A scan appends each file's
  values to its batches as constant arrays — text as a one-value `Int32`
  dictionary, `bigint` and `date` as plain arrays — and projects them like
  any column; a projection of keys alone still reads the narrowest file
  column, by compressed bytes, for its row counts.
- The scan predicate is folded over each file's path values before any file
  is opened. Comparisons, `IN`, `IS [NOT] NULL`, `LIKE`/`ILIKE` and
  `AND`/`OR`/`NOT` over them fold under SQL's three-valued logic: a file
  whose values make the predicate false or NULL is pruned; a term the path
  cannot decide (a literal of another type, a column that is not a key)
  keeps the file, and `NOT` folds only over an operand that folded exactly,
  since the negation of a weakened predicate is not implied. What the fold
  leaves open on the file columns is the predicate the kept file's reader
  runs (row-group pruning, late materialisation, as before); the executor's
  filter above the scan is unchanged and remains the truth. A NULL
  partition compares to NULL and is pruned by every comparison, kept by
  `IS NULL`. Pruned files are never opened and never counted as considered:
  they appear as `files_pruned_by_partition` on the task's scan metrics, on
  the query record's scan telemetry and in Studio (dealt round-robin over
  the scan partitions so the tasks of a stage sum to the total), while
  `files_considered` counts the kept files a partition was assigned and
  `files_opened` those it opened. When every file is pruned the scan is
  empty but still advertises the table's schema from the first listed
  file's footer (a metadata read, not an open).
- Files are spread over the scan partitions from the kept set: the pruned
  bytes weigh nothing in the assignment. When planning takes statistics for
  a directory table and every scan of that table in the plan carries the
  same storage predicate, the listing pinned for the query is the pruned one
  and the relation's row count is the kept files' (a footer read); a table
  scanned under different predicates, or once without, is planned at its
  whole listing.

Not partitioning: a Delta or Iceberg table's partition columns come from
its own metadata, and `partitioned_by` is refused for those formats.

Files are spread over the scan partitions by size: whole files go to the
partition with the fewest bytes so far, largest first; if the heaviest
partition would then carry more than a quarter over its fair share, the
largest whole file is split by row group across every partition instead
(the row-group modulo a single-file table has always used) and the rest are
placed again. The assignment is a pure function of the listing and the
partition count, so every task of a query derives the same one from the same
listing. Each file is read through the per-object reader with its own
identity-pinned footer cache, row-group pruning, projection and decoder
lanes; `files_considered` counts the files a partition was assigned and
`files_opened` the files it opened, and the listing time is reported as the
scan's snapshot time.

`ANALYZE` and planning statistics list the directory once, take the exact
row count from every file's footer, and key the statistics by a digest of
the listing (path, size, ETag or version per file; modification time for
local files). The listing that planning analyzed is pinned for that query on
the coordinator (`SourcePins`), the way Delta versions are, so the
coordinator-local scan reads the files the statistics came from even if a
file lands meanwhile. Workers of a distributed query list the location
themselves under the same deterministic rule: the executable fragment names
the location and carries no listing (its wire format is unchanged), so a
file that lands between two tasks' listings is a window the fragment does
not yet close; carrying the listing in the fragment belongs to the split
assignment workstream. A local data directory (`KAVEON_DATA_DIR`) registers
each child directory of Parquet files without a `_delta_log` as a Parquet
table, alongside `*.parquet` files and Delta directories.

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

Three surfaces write those definitions, and all three produce the same
records:

- **Catalog statements** on `POST /v1/statement` — `CREATE CATALOG`,
  `CREATE SCHEMA`, `CREATE TABLE … WITH (location, format)`,
  `CALL system.register_table`, `ALTER TABLE … SET LOCATION`, `DROP …`,
  `SHOW CREATE TABLE`, `DESCRIBE`, `SHOW CATALOGS|SCHEMAS|TABLES` — under
  the submitting principal's role (admin for catalogs, analyst or admin for
  schemas and tables). `CREATE TABLE` without a column list reads the
  columns from the source with a metadata-only probe (the Delta log, the
  Iceberg metadata pointer, the Parquet footers) and stores them; the table
  is created as a draft, probed, and activated only when the location is
  readable — a failed probe deletes the draft and reports the storage error.
  The grammar and error codes are in the
  [API reference](../reference/api.md#catalog-statements).
- **The `kaveon` CLI** — `kaveon catalog add|drop|list|show`,
  `kaveon schema add|drop|list`, `kaveon table register|relocate|drop|
  describe|show-create|list` — which submits those statements to the
  coordinator ([CLI guide](../guides/engine-cli.md#catalog-administration)).
- **The catalog HTTP API** (`/v1/catalog/definitions`, `…/schemas`,
  `…/tables`, revisioned with `If-Match`) under the catalog service
  credential, which the registration scripts use
  (`scripts/register-curated-catalog.py`, `register-clickbench-catalog.py`,
  `register-tpch-catalog.py`: tables from a manifest, each `COUNT(*)`
  verified against it). This surface does not probe a location; a table
  registered through it is verified by the script's count.

The platform PostgreSQL source registry and the Engine catalog are separate
stores, connected by the native catalog synchronization API
(`POST /api/v1/catalog-sources/{id}/engine-sync`, revision-aware). Saving a
platform source does not import data or register its tables. See the
[registration guide](../guides/register-engine-catalog.md).

The product transaction store (`KAVEON_PRODUCT_TRANSACTIONS_ENABLED`) commits
typed product records to ADLS with conditional writes; it is a bounded
metadata protocol, not general DML (see the [ADLS transaction
protocol](../engineering/adls-transaction-protocol.md)).
