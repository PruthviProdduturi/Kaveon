# The lake: Delta conversion, declared shapes, and the cube

How the `OpenSource` catalog's tables are stored and made fast, and the traps
found doing it on 2026-10-07. Everything here is implemented and deployed
unless it says otherwise.

## State

All 15 tables in the `OpenSource` catalog are registered `format: Delta`. None
is Parquet. The catalog is `platform-42e74bbd-7e3b-43c1-b6ea-14024c08c61f`
over ADLS account `kaveonlake`, container `opensource`, root path
`snapshots/2026-09-09-v1`.

## Converting Parquet to Delta without moving data

A Delta table is Parquet data plus a transaction log, and the data already in
the lake is exactly the data a Delta table would hold. So conversion writes a
`_delta_log` naming the files already present and re-uploads nothing — which is
what makes it viable for a 6 GB table.

    convert_to_deltalake(str(table_directory), mode="error")

The log records each file's name and **size**, so the local copy the log is
built from has to be byte-identical to what the lake holds. Check that rather
than assume it: list the blobs, compare name and `contentLength` against the
local files, and skip any table that differs. Then upload only `_delta_log/`.

Registering with an **empty** `columns` list is deliberate: the Engine then
infers the schema from the Delta log rather than a Parquet footer, which is the
thing worth proving. Registration is delete-then-create (there is no
change-the-format call), and `verify: true` makes the Engine read the location
through the Delta reader before accepting it. Capture each table's current
definition first and restore it if the Delta registration fails, because
between the delete and a failed create the table is absent and dashboards read
these tables.

### Trap: an all-NULL column is typed `void`

`ai_benchmarks.leaderboard` refused with:

    location 'ai_benchmarks/leaderboard' is not readable as delta:
    storage: unsupported Delta logical type void

Three score columns (`mt_bench`, `math_score`, `arc_challenge`) were NULL in
all 34 rows, so Parquet recorded them with the `null` type. Delta's protocol
has no such type, and neither does SQL — a column whose type is "no type"
cannot be read, compared or aggregated. The fix is to give the column the type
it always had (these sit beside `mmlu`/`humaneval`/`gsm8k`, all `double`), not
to drop it. `ai_benchmarks.benchmark_scores` hit the same thing on an all-NULL
`eval_date`, typed `date32` from the source DDL.

Check for this before converting:

```python
for field in pyarrow.parquet.read_table(path).schema:
    if pyarrow.types.is_null(field.type):
        print("will refuse as Delta:", field.name)
```

## File layout decides whether the cluster is used at all

**The Engine distributes work per file.** A table held in one large file is one
task on one worker, whatever the cluster size.

`public.kaveon_events_enriched` held all 504,000,000 rows in a single 6.1 GB
Parquet file. The consequences were not subtle:

- the second worker did no work at all;
- a single-dimension `GROUP BY` took 47–63s, sitting on the client's 60s bound,
  so charts appeared to fail at random when they were all on the same edge;
- the cube could not be built, because its one task cannot finish inside
  `REMOTE_TASK_TIMEOUT` (600s, a source constant in
  `engine/crates/server/src/api.rs`).

The file carried 168 row groups, so the parallelism was present in the data and
unreachable by the planner, which splits on files.

It is now 126 files of ~3.9M rows each, written by streaming
`ParquetFile.iter_batches` into a `ParquetWriter` per output file with a
1,000,000-row row-group size, so memory stays bounded over a 6 GB source.
Always verify the rewrite holds the same number of rows as the source before
publishing it.

### Publishing a relaid-out table without a broken window

Upload in this order so readers never see a half-published table:

1. the new part files, while the existing `_delta_log` still names only the old
   object — every reader keeps seeing the old table;
2. list the parts back and confirm each landed at its full byte size;
3. overwrite `_delta_log/00000000000000000000.json`. It is one object, so that
   write *is* the switch.

Leave the superseded object in place. Nothing references it once the new log
lands, so it costs storage and nothing else, and keeping it means the switch can
be reverted by restoring the previous log alone. Delete it only after the new
layout has been exercised.

## Declared shape and the cube

Statistics answer a table's totals and bounds; breakdowns need a cube, built
over a shape declared on the table definition. Nothing declared, no cube —
nothing is inferred. There is no API for this today; it is set through SQL:

    ALTER TABLE public.kaveon_events_enriched SET SHAPE (
        dimensions = ARRAY['surface', 'platform', 'license', 'segment',
                           'industry', 'region', 'country', 'deployment',
                           'acquisition_channel', 'team_size'],
        measures   = ARRAY['actions:sum,count', 'sessions:sum',
                           'queries_run:sum', 'charts_created:sum',
                           'errors:sum', 'rows_scanned:sum', 'cache_hits:sum',
                           'duration_sec:sum',
                           'latency_p75_ms:sum,count,min,max',
                           'user_id:count_distinct']
    )

Then `ANALYZE … WITH (cube = true)` builds it.

Two decisions in that declaration worth keeping:

- **The cardinality caps are left at the default on purpose.** A cap is what
  axis *pairs* are planned from, and at the default (10,000) no two dimensions
  pair. The plan is then the grand total plus each single axis — about a hundred
  thousand cells against a `KAVEON_CUBE_MAX_CELLS` limit of a million. Every
  chart on these dashboards is a single-dimension breakdown, so pairs would be
  pure cost. Declaring a dimension's true cardinality is what unlocks its
  combinations, and should be done only when a chart asks for one.
- **`latency_p75_ms` keeps `count` beside `sum`** because the charts average it
  and a weighted average needs both. `user_id` is a measure, not an axis: it is
  only ever counted distinct, which the cube keeps as a HyperLogLog sketch.

### Trap: a date column stored as text takes no time axis

Declaring `time = 'event_date:day'` is refused:

    time 'event_date' (Utf8) cannot be bucketed at day grain; a date column
    takes day grain, a microsecond timestamp day or month

`event_date` is stored as text on this table. The refusal is correct — the row
path has no way to truncate a string either, and the cube answers nothing the
row path cannot compute. The table therefore has no time axis, and anything
date-ranged over it is weaker than it should be. **Open:** retyping that column
is a data change that 33 charts read, so it belongs in its own change rather
than riding along inside a layout rewrite.

## The host has to be able to do the work

A cube build is one pass over the whole table computing every column's
statistics and the sketches, and it is bounded per task at 600s. That bound is
not negotiable from the API side, so the host must be fast enough to finish
inside it.

`kaveon-vm` runs the coordinator, both workers, the API, Studio and Caddy. On
`Standard_B2als_v2` — 2 vCPU, 3 GB RAM, with ~1 GB free and
`KAVEON_LOCAL_PARALLELISM=4` oversubscribing two cores — the cube build failed
three times, always as:

    statistics read failed: worker 'worker-1' did not finish the task within 600s

B-series are *burstable*: a multi-minute scan exhausts its credits and is then
throttled to a fraction of a core, so the failure is not a timeout to be raised
but a host that cannot do the work. Splitting the file did not fix it, because
this stage gives a worker one task over all of its files.

The approach that works is to resize for the build and resize back, because a
built cube serves breakdowns from precomputed cells and needs little CPU
afterwards:

    az vm resize -g kaveon-rg -n kaveon-vm --size Standard_D8as_v5
    # build statistics and the cube
    az vm resize -g kaveon-rg -n kaveon-vm --size Standard_B2als_v2

The public IP is `Static`, so the address and
`kaveon-api-wus2.westus2.cloudapp.azure.com` survive the restart. Containers
come back on their own; check `docker ps` before building. **Resize back** —
`Standard_D8as_v5` is roughly $140/month against $30 for `B2als_v2`.

Run the build off the request path, inside the API container, so neither the
ingress nor a browser imposes a shorter bound:

```python
engine_bridge.analyze_table("OpenSource", "public", "kaveon_events_enriched",
                            actor, "Admin", sketches=True, distinct=True,
                            cube=True, timeout=10_800)
```

`ANALYZE`'s own bound is now split in `engine_bridge`:
`CUBE_ANALYZE_TIMEOUT_SECONDS` for a cube and
`READ_ANALYZE_TIMEOUT_SECONDS` for a plain read. The earlier single 600s bound
*cancelled* the statement when it passed, so every attempt threw away its whole
pass rather than leaving it to finish.

## Reading an Engine query's lane

`GET /api/v1/engine/console/queries` is the fastest way to see what actually
happened. The statement is in `sql`, **not** `statement` — filtering on the
wrong key silently matches nothing. `execution.mode` is the lane:

- `context` — answered from statistics or the cube, no scan. `COUNT(*)` over
  the 504M table answers in ~358ms as
  `statistics at delta v0 (c25f525ec2a8)`.
- `pending` — never settled, which is what a cancelled scan looks like.

A `CANCELED` row at ~60,100ms is the API's client bound; at ~600,100ms it is
the per-task ceiling.

## Open items

- `ORDER BY` an aggregate that is not in the `SELECT` list fails under
  distributed execution — `column 'count_<col>' not found in batch`, from every
  worker. The partial-aggregate column is computed for the sort and never
  projected through the exchange. Reproducible on a 1,169-row table, so it is
  unrelated to scale. Engine-side; raised with @Codex in `HANDSHAKE.md`.
- `REMOTE_TASK_TIMEOUT` is a source constant, so a single large file is
  permanently un-cubeable. Worth making a setting, or worth splitting a large
  file's work by row group. Also raised with @Codex.
- `event_date` retyping, above.
- The superseded `combined-v1.parquet` is still in the lake, unreferenced.
