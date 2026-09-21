# The learning engine

The one reference page for the Engine's *knowing path*: how a table's
statistics are built, what they are, which statements they answer without a
scan, how the record says so, and what the readers skip when a scan does
run. Every claim on this page is checked against the code it names
(`engine/crates/core/src/{statistics,sketch}.rs`,
`engine/crates/storage/src/{table_statistics,source_statistics,footer_profile,parquet_directory,parquet_reader,scan_predicate,clustered_writer,table_rewrite,metrics}.rs`,
`engine/crates/optim/src/statistics.rs`,
`engine/crates/server/src/{api,settings,optimize}.rs`,
`engine/crates/catalog/src/lib.rs`); the statement grammar and the JSON
shapes are in the [API reference](../reference/api.md#statistics-statements),
the settings in the [settings reference](settings.md#per-request-settings),
the formats and layout in [Storage and catalogs](storage-and-catalogs.md).
Where this page and those disagree, the code wins and the page is wrong.
The last section says what is target rather than built.

## The idea

The Engine learns a table as it lands — `ANALYZE` turns the source's own
metadata, and optionally one pass over the columns, into a statistics
object versioned by the exact source it describes — and then plans and
answers from what it knows: a statement the record can answer exactly at
the version the statement is pinned to is answered without a scan; a
statement it cannot answer is read, with the record and the formats' own
metadata deciding which files, row groups and pages are never opened; and
every query record says which of the two happened (`execution.mode`),
which version answered, and which results are estimates and with what
error. The one rule that makes this safe is that a record whose version is
not the source's current version *costs* a plan but never *answers* a
statement.

## `ANALYZE`

`ANALYZE [catalog.][schema.]table [WITH (…)]` builds the statistics object
and stores it. Three forms, by how much of the table they read; the parser
accepts exactly the keys `distinct`, `columns` and `sketches`
(`api.rs`, `parse_analyze_body`; an unknown key is refused naming the
three, and `distinct` and `columns` together is refused — there is no
`depth` key, `depth` is a field of the stored object). Admin role only
(`execute_analyze`: any other role is 403 `FORBIDDEN`).

| Form | Reads | Produces | Cost |
|---|---|---|---|
| `ANALYZE t` (metadata only) | Parquet footers; the Delta log's add actions (their `stats` when every add carries them, else the active files' footers); the Iceberg metadata pointer and manifests, plus the live files' footers by field id. Never a data page. (`kaveon_storage::metadata_statistics`, `footer_profile.rs`) | Table facts (rows, bytes, files, row groups, uncompressed bytes, last modified, partition columns); per column the null count, `min`/`max` with `bounds_exact`, compressed bytes; per file rows, bytes and bounds; `depth: "metadata"` | Metadata reads only: a 504 M-row lake table profiles in tens of milliseconds (HANDSHAKE 2026-09-18) |
| `ANALYZE t WITH (sketches = true)` | Every sketchable column once, on the coordinator, files in parallel (eight at a time, `ANALYZE_SKETCH_THREADS`), batches reserved through the statement's memory admission; a source that changes under the read is refused (`kaveon_storage::full_statistics`) | Everything above, plus per column a HyperLogLog distinct-count sketch (p = 12) and, for numeric and temporal columns, a KLL quantile sketch (k = 200); exact bounds and exact null counts; `depth: "full"` | One full read of the table's columns |
| `ANALYZE t WITH (distinct = true)` or `WITH (columns = ARRAY['a', 'b'])` | One `SELECT COUNT(DISTINCT "col") FROM t` per selected column, run through the coordinator's own statement path (workers, admission, cancellation; result cache off; the child records tagged `analyze:<parent id>`), up to four at a time and never more than the resource group's `max_concurrent` (`ANALYZE_COUNT_CONCURRENCY`) | The exact distinct count per selected column as `distinct_exact`, kept beside whatever the record holds at this source version; the source version is re-read after the counts and a change is 409 `SOURCE_CHANGED` | One distinct-count scan per column (a 504 M-row table, 21 columns: 56 s on the local Compose stack, single run, not a claim) |

The forms combine: `sketches = true` with `distinct` or `columns` reads
every column once and then counts the named ones exactly. A column not
counted keeps the previous exact count while the source version is
unchanged; a metadata-only `ANALYZE` at the same version keeps the previous
full read's sketches and exact bounds; a record at a new source version
carries only what was measured under it. The result is one row: `table`,
`row_count`, `distinct_columns` (how many columns this statement counted;
`0` for the first two forms). A parse error in any form is 400
`SYNTAX_ERROR`; a column the table does not have is 400 `ANALYSIS_ERROR`
before any count runs.

Two statements read the record and never the source:

- **`SHOW STATS FOR t`** (any statement-capable role): one row per column —
  `column_name`, `data_type`, `data_size`, `nulls_fraction`,
  `distinct_values_count` (the exact count when one is on record, else the
  sketch's estimate, else null), `low_value`, `high_value` — and a summary
  row with a null `column_name`, the table's `row_count` and its
  `data_size`; `analyzed_at` on every row. A table never analyzed is 400
  `STATISTICS_UNAVAILABLE` (`no statistics for c.s.t; run ANALYZE c.s.t`).
- **`DESCRIBE DETAIL t`** (any statement-capable role): `format`,
  `location`, `created_at` (always null: no source records it),
  `last_modified`, `num_files`, `size_in_bytes`, `row_count`,
  `delta_version`, `partition_columns`, `analyzed_at`, `catalog_snapshot`
  — from the record when the table was analyzed, else from a fresh
  metadata read (400 `DESCRIBE_FAILED` with the storage error when the
  source cannot be read).

The CLI passes both through and renders them (`kaveon table stats|detail`;
[CLI guide](../guides/engine-cli.md)).

## The statistics object

`TableStatistics` (`core/src/statistics.rs`, document version 3;
versions 1 and 2 were the product catalog's documents and are no longer
written or read):

| Field | Meaning |
|---|---|
| `table_id` | The table's durable catalog id |
| `source_version` | `{identity_sha256, kind, …}` — what the record describes (below) |
| `computed_at_ms` | When it was computed or last refreshed |
| `depth` | `metadata` or `full` (the columns were read once for the sketches) |
| `format`, `location` | The table as registered |
| `rows`, `bytes`, `files`, `row_groups`, `uncompressed_bytes`, `last_modified_ms`, `partition_columns` | The table facts; `bytes` is the data files' bytes as stored |
| `columns[]` | Per column: `name`, `data_type`, `null_count` (absent means not recorded, never zero), `min`, `max`, `bounds_exact`, `bytes`, and after the relevant `ANALYZE` form `distinct` (HLL, base64), `distinct_exact`, `quantiles` (KLL, base64) |
| `per_file[]`, `per_file_complete` | Per file `path`, `rows`, `bytes` and per-column `min`/`max`/`null_count`, kept while the table has at most 10,000 files (`MAX_PER_FILE_STATISTICS`); beyond that the list is empty and `per_file_complete` false, and file skipping falls back to the readers' own footer pruning |

**`source_version.kind`** is the identity the record is versioned by, one
per format (`source_statistics.rs` builds the digest):

| Kind | Extra field | Identity digested |
|---|---|---|
| `delta_version` | `version` | The Delta version (and, on object storage, the file list) |
| `iceberg_snapshot` | `snapshot_id` | The metadata pointer and snapshot id (and the file list) |
| `listing` | `files` | A directory of Parquet files at one listing: every file's path, size and ETag or version (object storage — a file without one is refused), or modification time (local) |
| `file` | — | One Parquet file: its ETag or version and size (object storage), or its length and modification time (local) |

The label a record is named by (`statistics at delta v12 (3f1a…)`,
`listing of 3 files (…)`) is the kind plus the first twelve characters of
the digest.

**Sketches** (`core/src/sketch.rs`). The distinct-count sketch is
HyperLogLog at p = 12 (4 096 six-bit registers, Ertl's improved estimator,
relative standard error 1.04 / √4096 = 1.6 %), hashing each value's
canonical text with a port of PostgreSQL's `hash_bytes_extended` — the
register layout and hash the DLM's sketch cuboids use, so two sketches at
one precision merge. The quantile sketch is KLL at k = 200; its stated
error is the normalized rank error 2.446 / k^0.9433 = 1.65 %, holding with
about 99 % confidence.

**Where it lives.** In the SQLite catalog beside the definition:
`table_statistics(table_id PRIMARY KEY → tables ON DELETE CASCADE,
source_version, computed_at_ms, depth, document)`, catalog migration 2
(`catalog/src/lib.rs`). Every write is an audit event (`statistics`,
with the source version, depth, rows and files). Statistics do not enter
the catalog snapshot identity, so an `ANALYZE` does not clear the result
cache. There is one store: the product catalog's `statistics/<op>.json`
is not written or read by the server.

**Automatic refresh** (`KAVEON_STATISTICS_AUTO_REFRESH`, default `true`,
coordinator). When planning observes a source version newer than the one
on record, the coordinator refreshes in the background, one refresh per
table at a time, as the actor `engine-statistics-refresh`
(`kaveon_storage::refresh_statistics`): files added to a full record are
profiled and read and their sketches folded in; a removed file, a schema
change, a metadata-only record or an incomplete per-file list recomputes
at the record's depth. A failed refresh leaves the previous record. Exact
distinct counts do not survive a refresh (they were measured under the
old version). Off, statistics change only through `ANALYZE`.

**The staleness rule.** A record is *current* for a statement when its
`identity_sha256` equals the source version the statement is pinned to
(`TableStatistics::is_current_for`) — and, for an answer, when its row
count equals the source's exact row count as well. A record that is not
current still costs (next section) and never answers.

## What answers from statistics

Planning loads the record for every table on a join side, under a filter
or in an aggregate, beside the source's current version and exact row
count (`api.rs`, `optimize_with_durable_statistics`). Two uses:

**Stale or current, the record costs.** `optim/src/statistics.rs` turns a
filtered scan into an estimated cardinality: equality and `IN` from the
distinct count (exact when one is on record, else the sketch), ranges from
the KLL sketch's fraction between the bounds or, without one, interpolated
between `min` and `max` (range terms on one column merged into one
interval), null tests from the null count, `OR` as `a + b − ab`, and
anything the record cannot judge — `LIKE`, an unknown column — at 1.0 so a
side is never understated. The estimated rows and bytes decide the build
side and a broadcast: an inner join broadcasts a build side of at most
1,000,000 estimated rows and 256 MiB with a probe at least four times
larger (`BROADCAST_BUILD_MAX_ROWS`, `BROADCAST_BUILD_MAX_BYTES`,
`BROADCAST_MIN_PROBE_TO_BUILD_RATIO`); without a record the exact row
counts alone decide, as before.

**Current, the record answers** (`api.rs`, `context_answer_shape` and
`context_answer`). The statement must be one aggregate node directly over
one scan — no predicate, no grouping, at most a projection that keeps the
aggregates in order under other names — whose aggregates are all of:

| Aggregate | Answered when | Value |
|---|---|---|
| `COUNT(*)` | The record is current and its `rows` equals the pinned source's row count | `rows`, exact |
| `MIN(col)`, `MAX(col)` | As above, and the column's null count is known and `bounds_exact` is true (an empty or all-null column answers null) | The recorded bound, exact |
| `APPROX_COUNT_DISTINCT(col)` (`APPROX_DISTINCT`) | As for `COUNT(*)`, and the column has `distinct_exact` or a `distinct` sketch | The exact count with `error: 0` and `sketch: "exact count"` when one is on record, else the HLL estimate with its 1.6 % standard error |
| `APPROX_PERCENTILE(col, p)` / `(col, ARRAY[…])` | As for `COUNT(*)`, and the column has a `quantiles` sketch | The KLL quantile(s) with the 1.65 % rank error |

Any other aggregate in the list, any predicate, grouping, join, set
operation, `HAVING`, `DISTINCT` or an exact `COUNT(DISTINCT)` takes the
computed path; so does a record at another version, a missing sketch, and a
bound that is not exact. The record holds one sketch per column for the
whole table (not per file or partition), which is why a grouped or
filtered `APPROX_*` — including grouping by a partition column — computes
its own sketch over the rows. Every answered statement carries
`execution.approximate` naming each estimate (next section); the same list
appears on a computed statement whose aggregates are sketched, so an
estimate is labelled on both paths. What is *not* approximated: exact
`COUNT(DISTINCT)` and every other function, unless the statement writes an
`APPROX_*` function or sets `approximate = true`
([Approximate aggregates](../reference/engine-sql-compatibility.md#approximate-aggregates)).

## The `context` mode on the query record

`execution` on a query record has five modes — `pending`, `distributed`,
`coordinator`, `cache`, `context` — and a `context` record has this shape
(`api.rs`, `ExecutionPlacement::context`):

```json
"execution": {
  "mode": "context",
  "detail": "statistics at listing of 3 files (9b1f2c0a4d7e) (hyperloglog p=12, kll k=200)",
  "source_version": {"identity_sha256": "9b1f2c0a4d7e…", "kind": "listing", "files": 3},
  "current_source_version": {"identity_sha256": "9b1f2c0a4d7e…", "kind": "listing", "files": 3},
  "approximate": [
    {"function": "APPROX_COUNT_DISTINCT", "argument": "user_id", "sketch": "hyperloglog p=12",
     "error": 0.01625, "error_kind": "relative_standard_error"},
    {"function": "APPROX_PERCENTILE", "argument": "latency", "sketch": "kll k=200",
     "error": 0.0165, "error_kind": "rank_error"}
  ]
}
```

`detail` is `statistics at <label>`, with the set of sketches that
answered in parentheses when any did; `source_version` and
`current_source_version` are equal by construction (the record only
answers when they are); `approximate` is absent when every result is
exact, and `error` is `0` with `sketch: "exact count"` when an exact
distinct count answered. A `context` record has no `stages` and `scans:
[]`. Under `use_statistics = false` the statement runs and its `detail`
ends with `; statistics bypassed` when the statistics would have answered.
`function` is the name as written: `COUNT` when `approximate = true`
rewrote a `COUNT(DISTINCT col)`.

Studio's Engine query page shows the placement on its *Ran on* row as
`Statistics · no scan · statistics at …` and an *Approximation* row with
each estimate's function, sketch and error (`Exact` when there is none);
the *Settings* row reads `statistics bypassed` when the statement set
`use_statistics = false`. SQL Lab's statistics line distinguishes only
`From cache` from `Live query` (`studio/app/lab/LabWorkbench.tsx`); a
`context` answer reads `Live query` there today — the query page is where
the placement is visible.

## The settings

Per statement, through the `settings` object of `POST /v1/statement` or
leading `SET SESSION <key> = <value>;` statements in the same request
(`server/src/settings.rs`; HTTP is stateless, a setting lives as long as
the statement it arrived with, and a request that is only `SET SESSION`
statements is refused). An unknown key or an out-of-range value is 400
`INVALID_SETTING`.

| Key | Default | What it does on the knowing path |
|---|---|---|
| `use_statistics` | `true` | `false` stands every *answer* from statistics aside: no `context` answer for `COUNT(*)`/`MIN`/`MAX`, no statistics answer for `APPROX_*`; the rows are read and `execution.detail` ends with `; statistics bypassed` when they would have answered. The record still *costs* the plan (join placement) and still prunes a directory scan's files — pruning, not answering. `scripts/scale-suite.py`, `benchmark-throughput.py` and `differential-cases.py` send `{"result_cache": false, "use_statistics": false}` on every statement: a benchmark measures the read path |
| `approximate` | `false` | `true` lowers every plain `COUNT(DISTINCT col)` as `APPROX_COUNT_DISTINCT(col)` under COUNT's output name, decided in the SQL front end before the lone `COUNT(DISTINCT)` would become a DISTINCT stage; the record's `approximate` note names the function `COUNT`. Nothing else is approximated |
| `result_cache` | `true` | `false` bypasses the coordinator's result cache for the statement (no lookup, no insertion). The cache is a third way a statement is not read — `execution: {mode: "cache", detail: "hit"}` with `cached_from` — keyed by the normalised statement text, catalog, schema, the published catalog snapshot identity, the planner's pinned Delta versions and the time zone, and cleared by every catalog publish and `DELETE /v1/cache` ([settings](settings.md#result-cache)) |

`SET SESSION use_statistics = false;` and `SET SESSION approximate =
true;` are the SQL spellings; the query record's `settings` field shows
what a statement set.

## Reading less

When a statement is read, four layers of metadata decide what is never
opened, in this order per file, and each is proven by a counter on the
scan telemetry of the task and the query record (`storage/src/metrics.rs`,
`ScanMetricsSnapshot`):

| Layer | Applies to | What is skipped | Metric |
|---|---|---|---|
| Hive partition pruning | Directory Parquet tables with `key=value` paths | The scan predicate folded over each file's path values under three-valued logic before any file is opened ([partition columns](storage-and-catalogs.md#partition-columns)) | `files_pruned_by_partition` |
| File skipping by bounds | Directory Parquet tables on both paths, from the current record's per-file bounds while `per_file_complete` holds and every listed file is on record (the pinned listing minus the proven-empty files, `kaveon_storage::skip_listing_files`, `planner::SourcePins`; the pruned listing travels to the workers in the fragment, [the listing travels with the plan](storage-and-catalogs.md#directory-parquet-tables)); Delta on every node, from the add actions' `stats` (`delta_reader.rs`); Iceberg on every node, from the manifests' `lower_bounds`/`upper_bounds`/`null_value_counts` by field id (`iceberg_reader.rs`, `with_predicate`) | Files whose recorded bounds cannot match, before any footer is read | `files_skipped` (against `files_considered`, `files_opened`) |
| Row-group statistics | Every Parquet file, all three readers | Row groups whose column-chunk min/max exclude the predicate, including byte-array bounds a writer marked inexact | `row_groups_considered`, `row_groups_selected`, `row_groups_pruned` on the record |
| Bloom filters | Row groups the statistics admitted, for `=`/`IN` under `AND`/`OR` on a column that carries a filter (`parquet_reader.rs`, `bloom_probes`, `bloom_can_match`; values hashed as the physical type stores them; INT96, fixed-length and decimal columns not probed) | Row groups whose filter does not know the value; the local reader reads the filter from the file, the object readers by one range request each | `row_groups_pruned_by_bloom`, `bloom_filters_read`, `bloom_filter_bytes_read` |
| Page index and row filter (late materialisation) | The local reader always (page index loaded when every column chunk carries an offset index; parquet-rs cannot load a column index without one); the ADLS and object readers by `KAVEON_LATE_MATERIALISATION` | The predicate's columns are decoded first as a decoder row filter (`scan_predicate.rs`, `CompiledPredicate`, `RowFilterPlan`: one stage per top-level conjunct over only its columns), the rest of the projection only for the rows that survive, and the pages the selection never touches are not fetched | `row_filter_rows_examined`, `row_filter_rows_admitted`; `compressed_bytes_read` below `compressed_bytes_selected` by what was left unread |

`KAVEON_LATE_MATERIALISATION` is `auto` (default; `always`/`on`,
`never`/`off`; anything else is `auto`): `auto` runs the row filter when
the rest of the projection holds at least four times the compressed bytes
of the predicate's columns (`LATE_MATERIALISATION_RATIO`) or the object is
held whole in memory (an object of at most 64 MiB with at least 32 row
groups is preloaded into the full-object cache); otherwise the decoder lanes filter decoded
batches with the same compiled predicate (a decoder filter over object
storage costs a second fetch round per row group, measured 1.7× slower on
two-column aggregate shapes). The pushed predicate shapes are comparisons
with a typed literal, `IS [NOT] NULL`, `IN`, `[NOT] [I]LIKE` with a
literal pattern, `AND` (a conjunct with no storage form dropped), `OR`
whole, `NOT` exact; the executor's filter above the scan is unchanged and
remains the truth. Dictionary-encoded columns are carried as dictionaries
end to end, so predicates, functions and group keys run once per
dictionary value. A distributed directory scan reads the coordinator's
pruned, skipped listing on every worker (the fragment carries it, assigned
per task; a listing over `KAVEON_FRAGMENT_LISTING_MAX_FILES` travels as
its digest and the tasks list and prune for themselves), so `files_skipped`
holds on both paths. Not yet: Iceberg row-group pruning inside the files
read.

## The layout

The readers prune by what the writer left: a table written as one file of
million-row row groups without a page index still touches every row group
for a filter on a column that looks clustered. A table definition can carry
a layout — `CREATE TABLE … WITH (clustered_by = ARRAY['a', 'b'], bloom =
ARRAY['c'])`, `ALTER TABLE … SET CLUSTERED BY (a, b)` (`()` clears),
rendered by `SHOW CREATE TABLE` (`TableLayout {clustered_by, bloom}` on the
definition) — and `OPTIMIZE` rewrites the files in it
(`storage/src/clustered_writer.rs`, `ClusteredParquetWriter`):

| Property | Value | What the readers get |
|---|---|---|
| Row order | Sorted by the clustering columns within every file; the writer verifies the order row by row and refuses an out-of-order batch | Narrow min/max per row group, so the statistics pruning drops what a point or range filter cannot touch |
| Row groups | Closed at 128 MiB of encoded bytes or 2^20 rows, whichever first (`DEFAULT_TARGET_ROW_GROUP_BYTES`, `DEFAULT_MAX_ROW_GROUP_ROWS`; `OPTIMIZE … WITH (row_group_bytes, row_group_rows)`) | The unit the readers prune by |
| Files | Closed at the first row-group boundary past 1 GiB (`DEFAULT_TARGET_FILE_BYTES`; `file_bytes`); a single-file table stays one file under its name | Whole files spread over scan partitions by size |
| Page index | Column and offset index on every column; pages of 20,000 rows or 1 MiB (`DEFAULT_PAGE_ROWS`, `DEFAULT_PAGE_BYTES`) | The row filter reads only the pages the selection touches; over an object store the offset index is what late materialisation fetches by |
| Bloom filters | One per row group on every clustering column and every `bloom` column, false-positive rate 0.01 (`BLOOM_FILTER_FPP`), sized for the row-group row cap | A point lookup on a high-cardinality key the statistics cannot narrow rejects the row groups that do not hold it |
| Encoding | Dictionary encoding, page statistics, ZSTD level 3, Parquet 2.0 | Dictionary-aware predicates and the columnar aggregate's arena keys |
| Footer | `sorting_columns` per row group, `created_by = kaveon-storage <version>`, key-value `kaveon.layout.clustered_by` (`LAYOUT_METADATA_KEY`) | Another engine sees the order; the Engine sees the layout a file was written in |

`OPTIMIZE [catalog.][schema.]table [WITH (…)] [WHERE predicate]` (admin
role; `server/src/optimize.rs`, `storage/src/table_rewrite.rs`) rewrites a
**Parquet** table only: `WHERE` selects files (path values folded first,
then footer statistics), the selected files are read as one source and
sorted by the executor's spill-aware `SortOperator` under the statement's
admitted memory (compacted without a sort when there is no clustering),
staged under `_kaveon_optimize/<id>/` (hidden by the listing rule) and
published crash-safe — manifest, new files into place, replaced files
deleted, manifest deleted; the next open finishes or rolls back an
interrupted rewrite and reports it as `recovered`. A partitioned directory
is rewritten one partition at a time; clustering by a partition column is
refused (400 `OPTIMIZE_INVALID`: it is constant within every file). One rewrite per location at a
time (409 `OPTIMIZE_IN_PROGRESS`). The result is one row: `table`,
`files_replaced`, `files_written`, `rows`, `row_groups`, `bytes_before`,
`bytes_after`, `clustered_by`, `recovered`. **Delta and Iceberg are refused** (400
`OPTIMIZE_UNSUPPORTED`): their files are named by a log or by manifests
the Engine does not write — there is no Delta commit writer — and moving
them would leave the log pointing at files that are gone. After a
rewrite the table's statistics are stale (the listing digest changed) and
the automatic refresh recomputes them; the result cache is not cleared,
the rows are the same.

Why the layout is a contract: every reader's skip is a function of the
file it opens — the sorted order, the page index, the Bloom filters — and
of nothing the Engine keeps elsewhere, so a file written in this layout
is skipped the same way by every Kaveon node and read correctly by any
engine that reads Parquet; and a file that is not in the layout is still
read correctly, only more of it. The measured skip is in
[Storage and catalogs](storage-and-catalogs.md#layout) (unit tests, not
benchmarks).

## The freshness signal

Two endpoints on the catalog API tell a caller whether what the Engine
knows is what the source now is (`api.rs`, `get_table_statistics`,
`get_table_version`; the catalog service credential or any authenticated
principal):

| Endpoint | Returns |
|---|---|
| `GET /v1/catalog/tables/{id}/version` | `{table_id, table, source_version, observed_at_ms}` — the current source version from the least metadata that establishes it (the Delta log's tail, the Iceberg pointer, a listing, a file's identity; no footer, no data page). Cheap enough to call before every answer |
| `GET /v1/catalog/tables/{id}/statistics` | `{table_id, table, source_version, current_source_version, observed_at_ms, stale, statistics}` — the record and the version observed now; `stale` when the two differ. 404 `STATISTICS_UNAVAILABLE` when never analyzed, 409 `TABLE_NOT_PUBLISHED` for a draft or retired table, 502 `SOURCE_UNAVAILABLE` when the source cannot be read |

`GET /v1/statistics` (admin) lists every table with a record (`table`,
`table_id`, `row_count`, `source_version` label, `depth`, `computed_at`,
`current`); the platform API calls it behind `GET
/api/v1/engine/console/statistics`, which Studio's catalog pages read.
The DLM reads `/version` (`api/services/engine_bridge.py`,
`table_version`) for a dataset bound to an Engine table: at generation it
records the version the artifact was compiled at, and its freshness
scorer compares that with the version observed now — equal is no change,
a moved identity is a change of the half fraction and the sweep rebuilds
the value index. The DLM's answers over such a table are not served from
its own cells at all: each is one statement the Engine answers from the
cube or the statistics at the pinned version (`execution.mode =
"context"`) or reads, and the answer's evidence carries the record's
`source_version` (or `/version` for a read) — see
[Over Engine tables](../guides/nl-to-sql.md#over-engine-tables).

## Guarantees

- **Every statistics-answered statement is differentially checked against
  a scan** in the gate suite (`server/src/api.rs` tests):
  `context_answers_equal_the_scanned_answers_and_refuse_a_changed_source`
  runs seven aggregate shapes scanned, then after `ANALYZE` answered from
  the record — same columns, same rows, `mode: "context"`, empty `scans`,
  `source_version == current_source_version` — and checks that a
  predicate, a grouping, `SUM`, an exact `COUNT(DISTINCT)` and a join are
  never answered from it, that a file landing makes the next statement
  scan, and that the background refresh restores the answer at the new
  listing; `approximate_aggregates_answer_from_statistics_within_their_stated_error`
  holds the sketch answers within their stated error and checks the exact
  count's `error: 0` and the `; statistics bypassed` suffix;
  `stale_statistics_cost_but_never_answer` and
  `current_statistics_skip_directory_files_by_their_bounds` are what
  their names say. The differential sweep
  (`server/src/differential_tests.rs`) runs 43 exact cases and 4
  approximate cases through nine executions each — six node-local layouts
  (dictionary, plain, three-file directory, clustered, Hive-partitioned,
  Hive-partitioned in another order) and three in-process distributed
  runs — and compares them pairwise; approximate cases compare within a
  per-case tolerance, zero for HyperLogLog (its registers merge exactly, so
  a distinct count is the same on every path) and 0.1 for KLL (a sketch
  merged from partials compacts differently from one built in sequence,
  so percentiles agree within the rank error).
- **Approximate is opt-in and labelled.** Nothing is estimated unless the
  statement writes an `APPROX_*` function or sets `approximate = true`,
  and every estimate is on the record's `execution.approximate` with the
  sketch that produced it and the error it states, on the statistics path
  and the computed path alike.
- **A stale record never answers.** The answer path requires identity
  equality and row-count equality with the pinned source; a record at any
  other version costs a plan and is refreshed in the background.
- **Benchmarks bypass all of it.** The suite scripts send
  `use_statistics = false` and `result_cache = false` on every statement,
  so a published number measures the read path.

## Not built yet

Stated as target, not as the product:

- **The cube beyond what it declares.** The incremental cube over a
  declared shape is built ([Declared shape and the
  cube](storage-and-catalogs.md#declared-shape-and-the-cube)); partition
  columns as axes, `ORDER BY`/`LIMIT` over a cube answer, a date column at
  month grain and a `/cube` freshness endpoint beside `/statistics` are
  not.
- **The DLM over Engine tables beyond one table.** A dataset bound to an
  Engine table is answered from the cube and the statistics with its
  evidence and the `/version` freshness signal
  ([Over Engine tables](../guides/nl-to-sql.md#over-engine-tables)); a
  dataset joining Engine tables, and a time axis the DLM asks by year or
  month against a cube's `time` grain, still take the row path.
- **Heavy-hitter sketches.** `APPROX_MOST_FREQUENT` is not implemented:
  the statistics object holds no count-min or space-saving sketch, and
  adding one is a third sketch kind in the object, the exchange encoding
  and the DLM's cuboids.
- **Per-file and per-partition sketches.** The record holds one sketch per
  column for the whole table, so a grouped-by-partition-column or
  partition-predicate `APPROX_*` computes; per-partition sketches are what
  a grouped statistics answer needs.
- **Distributed file skipping by the record.** A distributed directory
  scan lists on each worker and does not carry the coordinator's pruned
  listing; carrying the listing (or the split assignment) in the task
  belongs to the split-assignment workstream.
- **Iceberg row-group pruning inside the files read**, beyond the
  manifest-level file skipping.

The program these belong to, with the exit criterion per area, is the
[9/10 program](../engineering/nine-of-ten-program.md); the calls behind the
design are in the [decision log](../engineering/decision-log.md).
