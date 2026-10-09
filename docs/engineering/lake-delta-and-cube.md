# The lake: Delta conversion, declared shapes, and the cube

How the `OpenSource` catalog's tables are stored and made fast, and the traps
found doing it on 2026-10-07 and in the events-table rebuild of 2026-10-09.
Everything here is implemented and deployed unless it says otherwise.

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

It was first relaid out as 126 files of ~3.9M rows each by streaming the single
file through `ParquetFile.iter_batches`. The table has since been **rebuilt**
rather than relaid out (below), and the layout is now **126 files of
3,600,000 or 4,500,000 rows, each holding exactly one `event_date`**. Always
verify a rewrite holds the same number of rows as the source before publishing
it.

### Publishing a relaid-out table without a broken window

Upload in this order so readers never see a half-published table:

1. the new part files, under names the published `_delta_log` does not mention
   — every reader keeps seeing the old table through the whole upload;
2. list the parts back and confirm each landed at its full byte size;
3. hold the table's latest version by padding the new log — see the trap below;
4. overwrite `_delta_log/00000000000000000000.json`. It is one object, so that
   write *is* the switch.

Leave the superseded objects in place. Nothing references them once the new log
lands, so they cost storage and nothing else, and keeping them means the switch
can be reverted by restoring the previous log alone. Delete them only after the
new layout has been exercised.

**New part files must take new names.** Overwriting `part-00000.parquet` in
place changes the bytes under an object the published log still names and
sizes, so readers break mid-upload. The rebuild names its files
`events-<date>-<block>.parquet` for that reason, which also leaves the previous
layout intact as the revert target.

### Trap: the latest Delta version must never go backwards

This one cost an outage, so it is worth stating plainly. The published log had
**two** commits — the conversion at version 0 and an `append-events.py` day at
version 1 — and the rebuilt log has **one**. Deleting the version-1 object
looked right: it added a file written with the old text `event_date`, which is
not readable under a schema that now says `date`, so it had to go before the
switch rather than after.

It took the table down for scans:

    DISTRIBUTED_EXECUTION_ERROR - worker 'worker-1' failed task with 500:
    {"error":"storage: requested Delta version is not available"}

**The coordinator pins a Delta version per statement and each worker resolves
that version from the log itself.** The coordinator had version 1 cached, so
every task asked its worker for a version that no longer existed and every task
failed. `COUNT(*)` kept answering — from statistics recorded at the old version
— so the table looked alive while nothing that read a file worked. Both
`source_version` and `current_source_version` reported version 1, which is the
tell: the coordinator had not noticed the log change at all, and would not,
because it was looking for a version number that had gone down.

The fix, and now what the tool does: **pad the new log instead of truncating
it.** Every published version above 0 is overwritten with a commit carrying
only `commitInfo` — a legal Delta commit that applies no action, so the version
exists and resolves to exactly what version 0 holds. The padding goes up
*before* the switch, where over the still-published version 0 it simply drops
what those commits had added, leaving a consistent readable table; then version
0 is overwritten and that is the switch. Same two writes, no deletion, no
window in which a version is missing.

Both log objects are copied to `backups/kaveon_events_enriched/_delta_log-<stamp>/`
before anything changes, so the revert is still one restore.

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

Declaring `time = 'event_date:day'` was refused:

    time 'event_date' (Utf8) cannot be bucketed at day grain; a date column
    takes day grain, a microsecond timestamp day or month

The refusal was correct — the row path has no way to truncate a string either,
and the cube answers nothing the row path cannot compute. The cost was measured
against the live Engine on 2026-10-08, with the cube built and the column still
text:

| statement over 504M rows | |
|---|---|
| a breakdown with no date filter (the cube answers it) | 906 ms |
| the same filtered to one day | 1,422 ms |
| the same filtered to a month | 61,250 ms — HTTP 504 |
| `GROUP BY event_date` | 48,750 ms |

Every date-ranged question fell off the cube and scanned the whole table.
**Fixed by rebuilding the table with `event_date` as `date32`** (below), which
is what makes `time = 'event_date:day'` declarable. The declaration and the
cube rebuild it needs have not been done yet.

The rule this leaves behind is general, and it is now a standing requirement
for every generator in this repo: **a date is stored as a date.** Not text, not
an integer year, not `yyyymmdd`. A date-shaped column stored as anything else
cannot be bucketed, cannot carry a time axis, and silently pushes every
time-ranged query onto a full scan. An audit on 2026-10-08 found the defect in
most of the lake-era curators — `curate-nyc-taxi.py`, `curate-who-covid.py`,
`curate-demo-extras.py`, `curate-kaveon-product.py`,
`consolidate-kaveon-product.py` and the old
`build-kaveon-events-parquet.py` all share one helper shape that maps integer →
`Int64`, float → `Float64` and *everything else* → string, which is how the
timestamps and dates became text — while every PostgreSQL-era loader under
`demo/` and `data/kaveon-usage/` got it right. `data/climate-energy/` is a
separate case: `year` and `month` are `SMALLINT`, and
`data/climate-energy/create_datasets.py` registers the `year` smallint as the
dataset's `date_column`, so the DLM's time-grain logic is pointed at an
integer. None of that is fixed here; it is recorded so it can be.

## Rebuilding the events table — 2026-10-09

`public.kaveon_events_enriched` was regenerated rather than relaid out, because
the three things wrong with it were in the data, not the layout. The tool is
`scripts/build_kaveon_events_table.py` with its geography in
`scripts/kaveon_events_geography.py`; both supersede
`scripts/build-kaveon-events-parquet.py` and `scripts/build_504m.py`, which are
kept only as the record of how the table was first assembled.

| | before | after |
|---|---|---|
| rows | 504,600,000 | 504,000,000 |
| files | 126 parts + 1 appended day | 126 |
| size | 6.7 GB (snappy) | 6.00 GB (zstd) |
| `event_date` | `Utf8` | **`date32`** |
| countries | 26 | **185** |
| industries | 13 (incl. a stray `Finance`) | 12 |
| metric values | `low + abs((user_id * seed) % range)` | drawn per metric |

Every other column keeps its type and every other dimension keeps its value
domain and, to within a fraction of a percent, its share — so the existing
breakdowns keep their shape.

### What `date32` changes

`event_date` is the one column whose type moved. Everything that reads it keeps
working: the Engine coerces a string literal against a date column, verified
against a `Date32` column on a registered Delta table, so
`WHERE event_date = '2026-07-20'` and `BETWEEN '2026-07-10' AND '2026-07-12'`
still parse and still match. `scripts/differential-cases.py` and the chart SQL
therefore need no rewrite. The dataset record already declared
`event_date` as `date` with `date_column: event_date`, so the retype makes the
physical column agree with what the catalogue always claimed;
`scripts/register-kaveon-events-dataset.py` said `varchar` and now says `date`.

### Values that read as telemetry

The previous generator's `low + abs((user_id * seed) % (high - low + 1))` is a
sawtooth over the primary key: uniform inside a hard range, with the range
bounds — 100, 500, 1000, 3000 — heavily hit. Each metric is now drawn from a
distribution chosen for it, per surface: lognormal for latency, scan volume and
session length, where a long right tail is the real shape; Poisson for counts,
so zero carries real mass; a per-user lognormal intensity factor, so power
users exist; a weekday/weekend factor and a mild trend across the window, so a
time axis has something true to show. Latency follows the user's *country*
rather than their intensity, scaled from connectivity, because latency is a
property of the path to the nearest region.

Determinism comes from seeding, not from arithmetic on the key: every chunk
takes `default_rng([SEED, day, block, surface])`, so any single file rebuilds
byte-for-byte without rebuilding the ones before it, and the whole table
reproduces exactly. Measured over four sampled files (16.2M rows):

| | min | median | p99 | p99.99 | max | on a multiple of 100 |
|---|---|---|---|---|---|---|
| `latency_p75_ms` | 30 | 487 | 3,555 | 10,095 | 36,804 | 1.01% |
| `rows_scanned` | 0 | 27,182 | 4,197,025 | 50,406,991 | 1,141,230,628 | 1.09% |
| `actions` | 1 | 7 | 114 | 587 | 2,393 | 0.03% |

1% on a multiple of 100 is what any smooth distribution gives, which is the
point: there is no clustering left to see. `errors` is zero on 87.7% of rows.

### The geography and its weighting

A country's share of the 3,000,000 users is proportional to its **addressable
technical audience**:

    weight = population x internet_penetration x adoption_index

Population alone puts Ethiopia above the Netherlands, which no software
product's telemetry looks like; population times penetration gives the online
population, which is the real upper bound on who could use the product; the
`adoption_index` (0.06–3.00, world average 1.0) is the per-online-person
propensity to use a self-hosted open-source data platform, standing in for
developer density, cloud spend per capita, ecosystem reach and income band.
The shares are then mixed 98/2 with a uniform floor, which guarantees every
country carries users — about 325 at the smallest — while moving the large
markets by under 2% of their share. Without the floor the smallest states round
to zero and the choropleth shows gaps that read as "no data" where the honest
answer is "a little".

Result: United States 17.5%, India 16.1%, China 11.3%, Japan 3.5%, Germany
3.2%, United Kingdom 3.1%, Brazil 2.9%, France 2.2%, Russia 1.9%; by region
Asia 45.0%, North America 21.9%, Europe 21.0%, South America 5.4%, Africa 5.2%,
Oceania 1.5%. 185 countries across all six regions, none under 10 countries —
North America 19, South America 12, Europe 40, Asia 46, Africa 54, Oceania 14.

Two rules make that survive contact with the UI:

- **Country names are the exact `properties.name` of the bundled Natural Earth
  GeoJSON** (`studio/public/geo/world.json`), so the choropleth matches on the
  stored value with no alias lookup. `WorldMapGlobe` *drops* a row whose name
  is not a feature, so a mistyped country would be invisible on the map while
  still inflating every other breakdown — `validate_against_geojson` fails the
  build instead, and runs in the `plan` step. This is why the table says
  `Czech Rep.`, `Korea`, `Lao PDR`, `Dem. Rep. Congo`. All 26 previous country
  values are in the new domain, so nothing that referenced one broke.
- **`region` is derived from the country, never drawn**, so the two columns
  cannot disagree. Regions follow the UN M49 continental grouping with the two
  simplifications the table already published: Central America and the
  Caribbean fold into North America, and Russia stays in Europe. Western Asia
  is therefore Asia and Egypt is Africa.

### The appender can no longer drift

`scripts/append-events.py` held its own copies of the value domains, and they
had already drifted from the table: it listed 12 countries where the table had
26, and its `industry` list said `Finance` where the table says
`Financial Services` — so the appended day of 2026-08-01 put 60,000 rows of a
13th industry value into a 12-value cube axis. It now imports the schema, the
domains, the surface profiles and the drawing functions from
`build_kaveon_events_table`, which makes that class of drift impossible rather
than merely discouraged. Its local table path moved with the rebuild, to
`data/adls-mirror/opensource/kaveon/kaveon_product/kaveon_events_enriched_typed`.

The rebuild does not carry the appended day forward, so the published table is
28 days again and `append-events.py` will offer 2026-08-01 as the next append —
this time drawn from the same domains as the rest of the table.

### Pre-flight: prove the reader before publishing 6 GB

The rebuild changes three things an Engine reader has to cope with at once — a
`date32` column, `DELTA_BINARY_PACKED` integer columns and Parquet v2 data
pages (the encodings are what keep a table of *drawn* values smaller than the
patterned one it replaces; dictionary pages are useless on irregular values and
plain `INT64` would have roughly doubled the table). So one real part file was
published to a scratch path, registered as a throwaway Delta table with
`verify: true`, and queried: the register returned `event_date: Date32` read
from the Delta log, and `COUNT(*)`, `GROUP BY event_date`, a date equality, a
date range, grouped `AVG`/`MAX` and `COUNT(DISTINCT country)` all answered
correctly. Then the table definition and the blobs were deleted. Worth doing
again for any change to how these files are written.

### What the published table reports

Read back through the live API after the switch, with the statistics and the
cube still stale, so every breakdown here is a full scan and none of these
timings is a benchmark claim:

| | |
|---|---|
| `SELECT * … LIMIT 3` | `Date32, Int64, Utf8, Int64, Utf8, Utf8` — latencies 282, 191, 235; countries `Dominican Rep.`, `India`, `United States` |
| `MIN`/`MAX`/`COUNT(DISTINCT event_date)` | `2026-07-04`, `2026-07-31`, 28 |
| `WHERE event_date = '2026-07-20'` | 18,000,000 rows (3,000,000 users x 6 surfaces), **1,344 ms** — the day's files prune, which is new |
| `WHERE event_date BETWEEN '2026-07-01' AND '2026-07-31'` | 504,000,000 — the whole table, confirming the published row count |
| `COUNT(DISTINCT country)` | 185 |
| `GROUP BY country, region` for one day | 185 rows — one region per country, largest 63.1M actions (United States), smallest 38,551 (Micronesia) |

Every one of those 185 values is a feature of the registered map, checked
against `studio/public/geo/world.json`: **0 dropped**. 30 of the bundle's
features carry no usage, and all of them are micro-territories, small islands,
uninhabited or disputed areas, plus `Dem. Rep. Korea` — which has no public
internet and is absent on purpose. No large landmass is empty. Taiwan is not a
feature of the bundle at all, so it cannot be covered from the data side.

And `GROUP BY event_date`, which the table could not do at all before, shows
the weekly rhythm the generator models — Saturday 154.2M actions, Sunday
135.0M, Monday 341.9M, Tuesday 356.6M, Wednesday 364.8M, Thursday 356.0M,
Friday 320.4M, then 158.4M and 138.5M the following weekend.

One thing the pre-flight exposes: **registering or deleting a table changes
the catalog snapshot, and every distributed statement fails until the workers
pick the new one up** —

    DISTRIBUTED_EXECUTION_ERROR - NO_COMPATIBLE_WORKER:
    no active worker has catalog snapshot sha256:…

It cleared on its own in well under a minute, but it is a real (brief) outage
for anything mid-flight, so catalog DDL against the live deployment is not
free.

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

## What has to come off a statement before the cube will answer it

A cube-shaped aggregate is answered from precomputed cells, but two things a
chart statement routinely carries put it outside that match, and both are
removed in the API by `services/engine_cube_rewrite.py` rather than worked
around in the chart.

- **`ORDER BY` or `LIMIT` disqualifies the match outright.** A breakdown
  carries both, so the statement is issued without them and the ordering and
  the bound are applied over the handful of rows that come back.
- **`AVG` is not a cell the cube holds.** It is reissued as `SUM` and `COUNT`,
  which are, and the quotient is formed in the API. The identity is exact:
  SQL's `AVG` ignores NULLs and `COUNT(col)` counts exactly the non-NULL
  values, so both sides divide the same sum by the same population.

The second reason stands on its own, and missing that cost a KPI tile 23
seconds. `SELECT AVG(latency_p75_ms) AS "ms" FROM public.kaveon_events_enriched`
is what a `big_number` chart with no `groupby` generates — no `ORDER BY`, no
`LIMIT`, because it returns one row. The rewrite required a clause to remove,
so the AVG was never substituted and the statement scanned 504M rows:

    SELECT AVG(latency_p75_ms) …                      12,747 ms  distributed
    SELECT SUM(latency_p75_ms) …, COUNT(latency_p75_ms) …  182 ms  context · cube

Both return `818.6926933214427`. The tile went from 23,047 ms to 906 ms, and
the Kaveon Events dashboard from 115 s to 15.1 s of sequential tile time.

### What a tile's remaining time is, and is not

At 906 ms a tile is no longer waiting on the query. Measured on `kaveon-vm`:

| | |
|---|---|
| transport + auth (a 404 round trip) | 62 ms |
| the Engine's own `elapsed_ms` | 182–196 ms |
| the rest — the API's control plane | ~660 ms |

Only one Engine statement is issued per request, so the remainder is not query
work. A KaveonDB control-plane read costs about 125 ms above transport
(`GET /datasets/144` at 188 ms against a 63 ms 404), and a chart request makes
several of them to resolve a dataset to a catalog, check permission and record
the run. **So what is left is transactional, not distributed** — worth
remembering before reaching for the cluster when a dashboard feels slow.

The same statement also varies from 906 ms to 3,328 ms run to run, which is
the burstable host rather than anything in the path; a single reading over
roughly a second says nothing on this host.

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
- **The cube and the statistics over `public.kaveon_events_enriched` have to be
  rebuilt.** The rebuild changed every file, so both are stale, and stale
  statistics are *served*: `SELECT COUNT(*)` still answers `504,600,000` from
  `statistics at delta v1` while the table holds 504,000,000, and every
  breakdown has fallen back to a full scan (a single `GROUP BY` is ~60 s
  again). Rebuilding needs the host resized (`Standard_D8as_v5`, then back) and
  is a cost decision, so the rebuild tool deliberately stops at the switch.
  Re-declare the shape with `time = 'event_date:day'` at the same time — that
  is the whole point of the retype, and it was never declarable before.
- The dataset's DLM artifact is stale after the switch: its manifest still says
  `event_date: varchar` and its value index holds 75 values against roughly 233
  in the new domain. The source hash changes with the switch, so freshness
  reports the dataset as changed and the ask path rebuilds it; no action unless
  it does not.
- The date-as-text defect is still present in the other lake curators, listed
  under the trap above, and `data/climate-energy/` still registers a `SMALLINT`
  year as a dataset's `date_column`.
- Until the cube is rebuilt, a single-dimension breakdown over the whole table
  is back on the client's 60s bound — `SELECT country, SUM(actions) … GROUP BY
  country` returned `Engine statement exceeded the client bound (60s)`, and a
  burst of concurrent full scans made the API answer 502 with an empty body for
  a minute while the cluster itself stayed healthy (coordinator uptime
  unbroken, both workers on the current catalog snapshot). The same breakdown
  filtered to one day answers in 14.4s, because the day's files prune. This is
  the pre-cube state the earlier layout work described, not a new defect.
- The superseded objects left in the lake, all unreferenced: the single-file
  `combined-v1.parquet`, the 126 `part-000NN.parquet` of the relaid-out layout,
  and the `part-00000-afd04987-….snappy.parquet` appended day. Delete them only
  once the rebuilt table has been exercised; until then they are the revert
  target, together with the log objects backed up under
  `backups/kaveon_events_enriched/_delta_log-<stamp>/`.
