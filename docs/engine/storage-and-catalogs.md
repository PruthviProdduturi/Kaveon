# Storage and catalogs

Arrow `RecordBatch` is the execution unit. Storage exposes streaming readers
with strict projection validation: unknown, empty, or duplicate columns
fail. Storage predicates conservatively eliminate Parquet row groups on
footer statistics (including byte-array statistics a writer marked inexact,
which the format still defines as bounds) and, for an equality on a column
that carries one, on Bloom filters; a row filter over the offset index
skips the pages a selection never reaches; execution filters preserve
row-level correctness. Narrow integers are presented at computing widths and
dictionary-encoded string columns are carried as dictionaries end to end,
so predicates, functions and group keys run once per dictionary value.

## Formats

| Format | Where | What is read | Refused by name |
|---|---|---|---|
| Parquet | local disk, ADLS Gen2 (`abfss://`), S3 (`s3://`, not qualified) | One object per table; projection, row-group pruning by statistics and Bloom filters, page skipping over the offset index, parallel decoder lanes for large ADLS objects (`KAVEON_SCAN_PARALLELISM`), full-object and decoded-batch caches under the pinned identity; written in the [clustered layout](#layout) by `OPTIMIZE` | — |
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

## Layout

The Engine reads better than most writers lay data out: a table written as
one file of million-row row groups without a page index still touches
every row group for a filter on a column that looks clustered. A table
definition can carry a **layout** — `WITH (clustered_by = ARRAY['a', 'b'],
bloom = ARRAY['c'])`, or `ALTER TABLE … SET CLUSTERED BY (a, b)` — and
`OPTIMIZE` rewrites the table's files in it. Three readers prune by it: the
local file reader, the ADLS reader and the generic object-store reader all
apply the same three steps to every file they open.

What `OPTIMIZE` writes (`kaveon_storage::clustered_writer`), and why:

| Property | Value | What it buys |
|---|---|---|
| Row order | sorted by the clustering columns, ascending, NULLs last, within every file; the writer verifies the order row by row and refuses an out-of-order batch | a row group's min/max span a narrow key range, so the statistics pruning every reader already does drops the groups a point or range filter cannot touch |
| Row groups | closed at 128 MiB of encoded bytes or 1 M rows, whichever first (`OPTIMIZE … WITH (row_group_bytes = …, row_group_rows = …)`) | the unit the readers prune by; Spark's and Trino's size, so a rewritten table is no worse for another engine |
| Files | closed at the first row-group boundary past 1 GiB (`file_bytes = …`); a single-file table stays one file under its name | whole files spread over scan partitions by size |
| Page index | column index and offset index on every column, pages of 20 000 rows or 1 MiB | a row filter reads only the pages the selection touches; over an object store the offset index is what late materialisation fetches by |
| Bloom filters | one per row group on every clustering column and every `bloom` column, false-positive rate 0.01, sized for the row-group row cap | a point lookup (`=`, `IN`) on a column the statistics cannot narrow — a high-cardinality key scattered over the file — rejects the row groups that do not hold the value |
| Encoding | dictionary encoding, page statistics, ZSTD level 3, Parquet 2.0 | the dictionary-aware predicates and the columnar aggregate's arena keys run over dictionaries |
| Footer | `sorting_columns` per row group, `created_by = kaveon-storage <version>`, key-value `kaveon.layout.clustered_by` | another engine sees the order; the Engine sees the layout a file was written in |

How the readers use it, per file, before any data page is read:

1. **Statistics.** Row groups whose column min/max exclude the predicate
   are dropped (`row_groups_pruned`).
2. **Bloom filters.** For every remaining row group, the columns the
   predicate compares for equality (`=`, `IN`, under `AND`/`OR`) that
   carry a filter are probed — one filter per column per row group, the
   local reader from the file, the object readers by one range request
   each — and a row group whose filter does not know the value is dropped
   (`row_groups_pruned_by_bloom`, `bloom_filters_read`,
   `bloom_filter_bytes_read`). A value is hashed as the column's physical
   type stores it: a literal an `INT32` column cannot hold is absent, a
   `DOUBLE` literal is probed against a `FLOAT` column only when it
   narrows exactly. `INT96`, fixed-length and decimal columns are not
   probed. `NOT`, `IS NULL` and `LIKE` do not consult a filter.
3. **Pages.** With a predicate, the page index is loaded (the local reader
   loads it when every column chunk carries an offset index; parquet-rs
   cannot load a column index without one) and the decoder's row filter
   skips the pages the selection never touches: `compressed_bytes_read`
   falls below `compressed_bytes_selected` by what was left unread.

The measured skip, from the storage crate's proof
(`a_clustered_layout_reads_fewer_row_groups_pages_and_bytes_than_the_same_rows_unclustered`,
`bloom_filters_prune_the_row_groups_the_statistics_keep`): 2 M rows in
8 row groups clustered by a key against the same rows written the way
`hits.parquet` is (1 M-row groups, no page index) — a point and a range
filter select 1 of 8 row groups instead of 2 of 2, examine 250 000 rows
instead of 2 000 000, select 1.45 MB instead of 23.1 MB and fetch 0.78–0.80
MB instead of 23.1 MB through the object reader's offset index; 800 000 rows
in 8 row groups with a Bloom column whose values are scattered over every
row group — the statistics keep 8 of 8 for `user = …`, `tag IN (…)` and
`user = … AND value >= 0`, the Bloom filters prune 7 (8 filters, 1.0 MB
read), and 100 000 rows are examined instead of 800 000. Unit tests, not
benchmarks: they prove the skip, not a speed.

`OPTIMIZE [catalog.][schema.]table [WITH (…)] [WHERE predicate]` (admin
role) rewrites a **Parquet** table in its layout and answers one row —
`files_replaced`, `files_written`, `rows`, `row_groups`, `bytes_before`,
`bytes_after`, `clustered_by`, `recovered`:

- *Selection.* `WHERE` selects files, not rows. For a partitioned
  directory the predicate is folded over the path values first, the way a
  scan prunes; what it leaves open meets each file's footer statistics. A
  selected file is rewritten whole; the rest stay as they are, so a table
  can be clustered a partition or a key range at a time. A predicate the
  footers cannot evaluate (an expression, an unknown column) is refused.
- *Sort.* The selected files are read as one source and sorted by the
  executor's `SortOperator` over the statement's admitted memory and the
  spill machinery, so a table larger than the memory budget sorts through
  spill runs. A table with no clustering columns is compacted into the
  layout without a sort.
- *Publication, crash-safe.* The new files are staged under
  `_kaveon_optimize/<id>/` (hidden by the listing rule; the process temp
  directory for an object store), then: a manifest naming the files to
  replace and the files written is stored; the new files are renamed into
  place (local) or uploaded (object store); the replaced files are deleted;
  the manifest is deleted. A crash before the manifest leaves the table
  untouched; a crash after it leaves old and new files both visible, and
  the next `OPTIMIZE` of the table finishes the rewrite when every written
  file landed (deleting the replaced ones) or rolls it back when one is
  missing (deleting the ones that landed) — `recovered` counts these — so
  no row is lost. A query planned while a publication runs may list both
  sets: a plain Parquet directory has no snapshot to isolate it, which is
  the cost of the format. One rewrite runs per location at a time
  (`409 OPTIMIZE_IN_PROGRESS`).
- *Partitioned directories.* Each partition directory is rewritten as its
  own group and published on its own, its new files under its own
  `key=value` path; a rewrite that stops between groups leaves every
  partition consistent. Clustering by a partition column is refused: it
  is constant within every file.
- *Single files.* A single-file table is replaced by one file under its
  name (one rename or one PUT).
- *Not rewritten.* A **Delta** or **Iceberg** table is refused by name
  (`400 OPTIMIZE_UNSUPPORTED`): its files are named by a log the Engine
  does not write — there is no Delta commit writer — and moving them
  would leave the log pointing at files that are gone. Their `OPTIMIZE`
  belongs to the writer that owns the log.
- *Statistics.* Row counts are unchanged; a table's statistics on record
  are versioned by the listing digest, so the rewrite makes them stale:
  the next planning re-lists the rewritten files and, with the automatic
  refresh on, recomputes the record (a removal is never folded). The
  result cache is keyed by the catalog snapshot and is not cleared: the
  rows a statement answers are the same.

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

## Table statistics

The reference page for the knowing path — `ANALYZE`'s three forms, the
object, what answers without a scan, the `context` record, the settings
and the guarantees — is [The learning engine](learning-engine.md); this
section is the storage view.

A table's statistics are one object in the durable catalog, stored
beside the definition under the table's id (`table_statistics`, deleted
with the table) and **versioned by the source version** they were
computed from — the Delta version, the Iceberg snapshot, the digest of a
directory listing, a file's size and modification time or ETag. The
object holds the table facts (rows, bytes, files, row groups, last
modified, partition columns), per column the null count, the bounds and
whether they are the true extremes, the compressed bytes, and — after a
full read — a HyperLogLog distinct-count sketch (p = 12, six-bit packed
registers, Ertl's estimator, 1.6 % standard error; the register layout
and hash the DLM's sketch cuboids use, so two sketches at one precision
merge) and a KLL quantile sketch (k = 200, 1.65 % rank error); per file the rows, bytes and bounds while the table has at most
10,000 files. `ANALYZE` builds it from metadata alone; `WITH (sketches =
true)` reads the sketchable columns once, files in parallel, batches
reserved through the statement's memory admission; `WITH (distinct =
true | columns = …)` adds exact counts through the cluster. The
statements, the endpoints and the JSON are in the [API
reference](../reference/api.md#statistics-statements).

What the planner does with the record depends on one comparison, the
record's source version against the source's version as observed for
the statement:

- **Stale statistics cost, never answer.** Whatever their version, they
  estimate a filtered scan's cardinality (equality and `IN` from the
  distinct count, ranges from the quantiles or interpolated between the
  bounds, null tests from the null count; what they cannot judge stays at
  1.0) and the estimate decides the build side and a broadcast. Without
  a record, the exact row counts alone decide, as before.
- **Current statistics answer and skip.** A `COUNT(*)`, `MIN` or `MAX`
  with no predicate is answered from a record at exactly the pinned
  version (`execution.mode = "context"`), for a bound only when it is
  exact and the null count is known. So is an `APPROX_COUNT_DISTINCT` or
  `APPROX_PERCENTILE` with no predicate and no grouping, from the
  column's stored sketch — the record names the sketch and states its
  error (`execution.approximate`); an exact distinct count on record
  answers before the sketch, with no error. The record holds one sketch
  per column for the whole table, so a grouped or filtered approximate
  aggregate computes its own sketch over the rows instead. `settings.
  use_statistics = false` stands every such answer aside (the rows are
  read; `execution.detail` says `statistics bypassed`). A coordinator-local scan of a
  directory table under a predicate is pinned at the files the record's
  bounds admit (`files_skipped`), on top of partition pruning. Delta and
  Iceberg readers skip files from their own metadata — the add actions'
  `stats`, the manifests' bounds and null counts — on every node,
  statistics or not; the readers' footer pruning follows inside the files
  read.
- **A newer source version refreshes the record** in the background
  (`KAVEON_STATISTICS_AUTO_REFRESH`): files added to a full record are
  read and their sketches folded in; a removal, a schema change or a
  metadata-only record recomputes at the record's depth. One refresh per
  table at a time; a failed refresh leaves the previous record, which
  still costs.

The version endpoint, `GET /v1/catalog/tables/{id}/version`, observes
the source version from the least metadata that establishes it — the
Delta log's tail, the Iceberg pointer and manifests, a listing, a file's
identity — and is the platform's freshness signal.

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
