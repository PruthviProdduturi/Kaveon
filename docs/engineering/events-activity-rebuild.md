# Rebuilding the telemetry table: `events.enriched` → `events.activity`

**Status:** the generator change is **implemented and tested**. The live
migration is **written, not run** — nothing in this change has touched the
installation. Defect D-4 in `docs/engineering/OPEN-DEFECTS.md`.

---

## 1. What is wrong with the published table

Measured live against `KaveonTelemetry.events.enriched` on 2026-10-13, through
the Engine as `pruthvi.prodduturi@gmail.com` / Admin:

| Fact | Value | Why it reads as fabricated |
|---|---|---|
| Rows | exactly **504,000,000** | `3,000,000 × 28 × 6` — a round product of round factors |
| Days | 28, each exactly **18,000,000** rows | perfectly flat; a Saturday identical to a Tuesday |
| `COUNT(DISTINCT user_id)` | exactly **3,000,000** | a round headline figure |
| `user_id` | `1, 2, 3 …` | not an identifier any system emits |
| Rows per user | exactly **168** for every user (28 days × 6 surfaces) | every user opened all six surfaces every single day of July |
| Name | `enriched` | an ETL stage name, not a description of content |

The last row of that table is the root cause of the first three. Verified
directly:

```sql
SELECT user_id, COUNT(*), COUNT(DISTINCT event_date), COUNT(DISTINCT surface)
FROM events.enriched WHERE user_id <= 6 GROUP BY user_id
-- [1,168,28,6] [2,168,28,6] [3,168,28,6] [4,168,28,6] [5,168,28,6] [6,168,28,6]
```

One thing that is **already correct** and was checked before changing anything:
the categorical columns are properties of the *user*, not re-rolled per row.

```sql
SELECT country, region, segment, industry, license, team_size, deployment,
       platform, acquisition_channel, COUNT(*)
FROM events.enriched WHERE user_id = 424242
GROUP BY country, region, segment, industry, license, team_size, deployment,
         platform, acquisition_channel
-- one row: Canada / North America / Enterprise / Professional Services /
--          Free / Solo / Cloud / Desktop / Partner, 168 rows
```

One combination across all 168 rows. The generator draws each dimension once
per user and reuses it for every day, and `region` is derived from `country`
rather than drawn, so the two cannot disagree. That property is kept.

## 2. What produced the data

**`scripts/build_kaveon_events_table.py`** — it exists, it is in the repo, and
it is what wrote the published table. Confirmed by `event_date` being `Date32`
live, which only this build produces (`build-kaveon-events-parquet.py` and
`build_504m.py`, both marked superseded in their own docstrings, wrote it as
text). `scripts/append-events.py` is its incremental half and imported the
same domains.

The defect was in one loop. `build_file` wrote, for every day, for every user
in the block, for every one of the six surfaces, exactly one row:

```python
for surface_index, surface in enumerate(SURFACES):
    metrics = surface_metrics(...)           # per-user metrics, drawn
    writer.write_table(pa.table(columns))    # per_block rows, always
    written += per_block
```

`day_volume` already modelled weekday/weekend and a trend, but it only scaled
the *values* inside the rows. The *number* of rows was `users × days × 6` by
construction, and `verify` asserted that product against a hardcoded
`EXPECTED_ROWS = 504_000_000`. So the shape was baked in and then enforced.

## 3. What changed in the generator (generator-only, no live effect)

`scripts/build_kaveon_events_table.py`

1. **Presence is drawn, not assumed.** Each user carries an `engagement` rate
   (Beta, bounded in (0,1)), a `primary_surface`, and a `breadth` for the
   surfaces that are not their home one. A row exists only when
   `random() < engagement × affinity × surface_presence × day_shape`, capped at
   0.97 so nobody is unconditionally present. Row counts are therefore a
   consequence of behaviour.
2. **`day_shape` replaces `day_volume`** and now drives row *existence* as well
   as metric magnitude — the coherent reading of a quiet Sunday is fewer people
   doing less. It adds a small per-day lognormal shock (σ = 0.045) on top of the
   weekday factors and the trend, so the series is not a perfect weekly sawtooth
   with a ruler through it.
3. **`user_id` is a UUID v4 as text.** Drawn as 16 bytes per user from a
   dedicated full-range `uint32` stream (full-range so it consumes exactly one
   word per element and stays prefix-stable, which is what lets the appender
   rebuild a prefix of the user table and get the same people back), with the
   version and variant nibbles set so the value actually parses as a UUID.
   Formatted canonically, `8-4-4-4-12`.
4. **`N_USERS = 14_627_893`.** Not round, so `COUNT(DISTINCT user_id)` is not
   round either. Larger than 3,000,000 because only ~20% of the user × day ×
   surface grid is now populated, and the table's order of magnitude — roughly
   half a billion rows — is load-bearing for the demo and for the benchmark
   program.
5. **`file_plan` no longer requires round user counts.** It used to refuse any
   user count that did not divide the per-day file count exactly, which is a
   constraint on nothing real and is part of how 3,000,000 got chosen.
   Remainders are spread instead.
6. **`files_per_day` allocates files in proportion to the day's shape.** A
   Sunday takes 4 files and the busiest Wednesday 13, so every file is about
   the same size even though the days are not. The Engine's 600s per-task bound
   applies to the largest file, so that is the figure worth holding constant.
7. **`verify` checks the files against the seeds, per row group.** The expected
   count for every file comes from replaying only the presence draws — a
   separate RNG stream from the metrics, precisely so this is cheap (24s for the
   whole table). The old `verify` compared a grand total to a constant;
   a short row group inside a file of the right length passed it.
8. **`report_shape` is a gate, not a printout.** The build and the plan both
   fail if two days hold the same number of rows, if any weekend day outranks
   any weekday, or if the total is an exact multiple of a million. The defect
   cannot come back silently.
9. **`upload` publishes to a new location and refuses a non-empty one.** The
   in-place switch machinery — backing up the published log, writing no-op
   commits over its later versions, then overwriting version 0 — is **removed**.
   A rebuild that can overwrite a live table is one mistake away from destroying
   the installation's context.
10. **Constants point at the table's real home.** `ACCOUNT/CONTAINER/REMOTE`
    were still `opensource/snapshots/2026-09-09-v1/public/kaveon_events_enriched`,
    which the table left when `KaveonTelemetry` was registered. They are now
    `kaveonlake / telemetry / events/activity`.

`scripts/append-events.py` — imports the presence model and the identifiers
rather than holding its own copies, so an appended day is as ragged as a built
one; its local path and remote target follow the generator's.

`scripts/test_build_kaveon_events_table.py` — 37 tests, all passing. The new
ones assert: identifiers are well-formed v4, unique, stable across populations
and addressable by block offset; the table fills well under 60% of the grid;
no two days are equal; weekends are quieter *in rows*; the median user touches
fewer than three surfaces on a day they are active; a user carries exactly one
set of dimensions everywhere; a truncated part and an unplanned part are both
refused; and the three shape gates reject flat, round and weekend-busy tables.

`scripts/register-kaveon-events-dataset.py` — binds `KaveonTelemetry.events.`
`activity` (it was still addressing the pre-catalog `Kaveon.usage.`
`kaveon_events_enriched`), declares `user_id` as `varchar` rather than `bigint`,
drops the row and user counts from the published description, and its preflight
now prints what the dataset is bound to *now* beside what it would bind to.

`scripts/register-events-activity-table.py` — **new.** Registers the one table
into the existing catalog and schema, compares the registration probe against
the build manifest's own row count rather than a literal, and declares the cube
shape. It refuses if the table is already registered and never touches
`enriched`.

## 4. Evidence

Full-scale projection, from the seeds, writing nothing
(`build_kaveon_events_table.py plan`, 24s):

```
506,542,814 rows over 28 days, drawn from 14,627,893 users
COUNT(DISTINCT user_id) will be 14,600,965; the 26,928 users with no activity
in the window have no rows
mean 18,090,815 rows/day, min 8,242,052 (2026-07-05), max 26,290,986 (2026-07-29)
  2026-07-04 Sat    9,640,395  ################
  2026-07-05 Sun    8,242,052  ##############
  2026-07-06 Mon   20,626,294  ###################################
  2026-07-07 Tue   19,552,604  #################################
  2026-07-08 Wed   21,282,817  ####################################
  2026-07-09 Thu   21,473,653  ####################################
  2026-07-10 Fri   19,701,678  #################################
  2026-07-11 Sat    9,859,253  #################
  2026-07-12 Sun    8,251,693  ##############
  …
  2026-07-29 Wed   26,290,986  ############################################
  2026-07-30 Thu   22,253,635  #####################################
  2026-07-31 Fri   23,160,774  #######################################

rows by surface:
  Dashboard       172,266,779   34.0%
  Chat            144,230,363   28.5%
  SQL Lab          70,119,290   13.8%
  API              57,852,655   11.4%
  Chart Builder    41,615,863    8.2%
  Export           20,469,794    4.0%
```

A real sample, built locally to a scratch directory (40,037 users, 28 days, 28
files — **not** to the lake):

```
28 files, 1,384,256 rows, 0.05 GB (33.4 bytes/row)
manifest: row_count 1,384,256  users 40,037  distinct_users 39,958  seed 20260704

distinct user_id             39,958        (old table: exactly 3,000,000)
distinct event_date          28
rows/day distinct values     28 of 28      (old table: 1 of 28)
rows per user over 28 days   min 1, median 33, mean 34.6, max 138
                                           (old table: exactly 168, every user)
surfaces on an active day    mean 1.77, max 6
                                           (old table: exactly 6, always)
grid fill                    20.6%         (old table: 100.0%)

sample identifiers
  b39ec3e4-7cec-4a0a-824b-cde5d3ff8ffe
  c38c8e95-743f-4c80-a19c-d2ef0816c110
  3f3c1ec6-f0a1-4d4a-be2c-beba7e5aa38f
  2327618c-40dc-492e-98a0-839ef276113b

segment share  Mid-Market 33.1%  SMB 22.5%  Enterprise 22.4%  Startup 22.1%
countries present  184 of 185
```

Reproduce it with:

```
python scripts/build_kaveon_events_table.py build \
    --output <scratch>/activity-sample --users 40037 --files 28
```

### The cost this carries

Per-column compressed bytes from that sample:

| Column | B/row | Share |
|---|---|---|
| `user_id` | 20.69 | 62% |
| `rows_scanned` | 2.68 | 8% |
| everything else (19 columns) | 9.79 | 30% |
| **total** | **33.16** | |

The published table is ~6 GB at 11.9 B/row. A 36-character random identifier
does not compress — no shared prefix, no useful dictionary, since each user
appears once per row group — so the rebuilt table lands near **15–17 GB**. Read
the real figure off the build's own `verify` line before uploading; the sample's
50,000-row row groups amortise dictionary pages worse than the full build's 1.4
million, so the dimension and metric columns will come out somewhat smaller at
scale while `user_id` will not.

That is the honest price of an opaque identifier, and it is worth it: a
sequential key leaks the user count and the creation order. If the cost ever
has to come down, the option not taken here is sorting each row group by
`user_id` and encoding it `DELTA_BYTE_ARRAY` — shared prefixes would recover
perhaps 15% of that column at the price of a sort per row group.

## 5. The live migration

Everything above is code. **Nothing below has been run.** The order matters:
the installation keeps reading `events.enriched` until step 4, and every step
before it is additive.

### What is bound to the old table right now

| Thing | Value |
|---|---|
| Dataset | `Kaveon Events`, id **515678609**, `KaveonTelemetry.events.enriched`, published |
| Charts on it | **34** of the installation's 77 |
| Charts referencing `user_id` | **9**, all `COUNT_DISTINCT(user_id)` (e.g. "Active Users", "Users by Region") |
| Sibling table | `KaveonTelemetry.events.dashboard`, 24 rows, a pre-aggregated projection — untouched by any of this |

The 9 `COUNT_DISTINCT(user_id)` charts keep working: the column name does not
change, and distinct counting goes through the cube's HyperLogLog sketch, where
a text key costs no more than an integer did.

### Step 0 — build and verify locally

```
python scripts/build_kaveon_events_table.py plan
python scripts/build_kaveon_events_table.py build          # hours; writes data/adls-mirror/telemetry/events/activity
python scripts/build_kaveon_events_table.py convert
```

`build` fails rather than writes if the daily shape is flat, a weekend outranks
a weekday, or the total is round. `convert` re-verifies before writing the Delta
log. Note the `row_count` in `build-manifest.json` — every later step is checked
against it.

### Step 1 — upload to a new lake location

```
az login
python scripts/build_kaveon_events_table.py upload
```

Publishes to `kaveonlake / telemetry / events/activity`. It **refuses** if that
location already holds a `_delta_log`, so it cannot overwrite
`events/enriched`. Parts go up first, are listed back and confirmed
byte-for-byte, and only then is the commit written — so the location is not a
readable table until everything is there.

Nothing in the installation can see this yet. Reversible by deleting the
directory.

### Step 2 — register the table

```
kubectl --context kaveon-aks -n kaveon port-forward svc/kaveon-portal 13015:3000

python scripts/register-events-activity-table.py                   # preflight
python scripts/register-events-activity-table.py --apply --shape
```

Adds `KaveonTelemetry.events.activity` beside `enriched` and `dashboard`,
verifies it by registration probe *and* by `COUNT(*)` through the catalog
against the manifest, and declares the cube shape. Reversible with
`DELETE /api/v1/engine/catalog/tables/{table_id}`.

At this point both tables are registered and queryable, and nothing points at
the new one. **This is the moment to look at it in SQL Lab** before anything
moves:

```sql
SELECT COUNT(*), COUNT(DISTINCT user_id), MIN(event_date), MAX(event_date)
FROM events.activity;

SELECT event_date, COUNT(*) FROM events.activity
GROUP BY event_date ORDER BY event_date;

SELECT user_id, COUNT(*) FROM events.activity
GROUP BY user_id ORDER BY COUNT(*) DESC LIMIT 5;
```

### Step 3 — statistics and the cube

Off the request path, inside the API container, with the host resized up first
(`docs/engineering/lake-delta-and-cube.md` has the resize and the reasoning):

```python
engine_bridge.analyze_table("KaveonTelemetry", "events", "activity",
                            actor, "Admin", sketches=True, distinct=True,
                            cube=True, timeout=10_800)
```

Then resize back down. This is the long, expensive step; it affects only the new
table, and the dataset still points at the old one while it runs.

### Step 4 — re-point the dataset (the switch)

```
python scripts/register-kaveon-events-dataset.py                   # preflight
python scripts/register-kaveon-events-dataset.py --apply
```

The preflight prints the dataset's current binding beside the one it would
write. `--apply` issues one `PUT /api/v1/datasets/515678609` with
`database_name KaveonTelemetry`, `schema_name events`, `table_name activity`,
and `user_id` as `varchar`. **All 34 charts follow immediately** — they bind by
`dataset_id`, so there is nothing to edit per chart.

This is the only irreversible-feeling step, and it is reversible: re-run with
`--table enriched` to put the binding back. The old table is still registered
and still has its cube.

### Step 5 — recompile the DLM

```
python scripts/register-kaveon-events-dataset.py --apply --generate --ask
```

**This is the step that has gone wrong before** — an OOM-killed API during
regeneration destroyed existing context. Mitigations, in order: the per-dataset
context spec is curatable and survives regeneration, so a curated spec is not
lost; `--ask` runs the five qualification questions and times them, so a broken
compile is visible immediately; and if generation fails, step 4's revert puts
the dataset back on a table whose context is intact.

Watch the API pod's memory while it runs:

```
kubectl -n kaveon get pod -l app=kaveon-api -w
```

### Step 6 — verify before retiring anything

- The Data catalog page lists `events.activity` as measured, with its row count.
- All 34 charts render, and the 9 user-count charts show ~14.6M rather than 3M.
- The home page's ask path answers "total actions by surface" and "active users
  by region" from context, not from a full scan.
- `SELECT event_date, COUNT(*) …` shows the weekday/weekend shape.

### Step 7 — retire `events.enriched`

Only once step 6 is clean, and deliberately as its own change:

1. `DELETE /api/v1/engine/catalog/tables/{enriched_table_id}` — removes the
   catalog definition. The objects stay in the lake.
2. Update the scripts that still name the old table (they must not move before
   this point or they break against a live table):
   `scripts/qualify-aks-fault-pressure.py`,
   `scripts/qualify-aks-coordinator-loss.py`,
   `scripts/soak-aks-current.py`,
   `scripts/differential-cases.py` (its `TABLE_A` default).
3. Update `scripts/register-kaveon-telemetry-catalog.py` — its `TABLES` list
   still declares `enriched` with `row_count: 504_000_000`. *(Owned
   elsewhere; a REQUEST, not a change made here.)*
4. Leave the lake objects under `telemetry/events/enriched` for at least one
   release, then delete the directory. That delay is the real revert window.

### Not part of this migration, but stale because of it

Out of this change's file scope; listed so they are not forgotten:

- `api/services/query_generator.py:927` — a comment citing
  `public.kaveon_events_enriched (504,600,000 rows)`.
- `studio/app/home/page.tsx:1091` — a comment naming
  `OpenSource.ai_benchmarks.kaveon_events_enriched`.
- `infra/aks/opensource-catalog-manifest.json`,
  `infra/aks/kaveon-catalog-manifest.json` — both still declare
  `kaveon_events_enriched`, one of them at a pre-Delta single-file location.
- `scripts/test_vercel_live_dashboard_import.py:77` asserts a total of
  `504000000` against imported dashboard metadata.
- `docs/engineering/lake-delta-and-cube.md` uses
  `public.kaveon_events_enriched` in its shape and ANALYZE examples.
- `docs/engineering/OPEN-DEFECTS.md` D-4 — closed by whoever runs the
  migration, not by this change.

## 6. Generator-only versus live, at a glance

| Change | Generator only | Needs a live step |
|---|---|---|
| Presence model, daily shape, trend, shock | ✓ | rebuild + publish |
| UUID `user_id` | ✓ | rebuild + publish, and the dataset's column type |
| Non-round user count and total | ✓ | rebuild + publish |
| Proportional file allocation, seed-checked `verify` | ✓ | — |
| Shape gates in `plan`/`build` | ✓ | — |
| `upload` refusing a non-empty destination | ✓ | — |
| Table named `activity` | ✓ | register table, re-point dataset, retire old |
| Dataset bound to `KaveonTelemetry.events.activity` | — | step 4 |
| `user_id` declared `varchar` | — | step 4 |
| Cube and statistics | — | step 3 |
| DLM context | — | step 5 |
