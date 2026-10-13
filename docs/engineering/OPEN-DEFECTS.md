# Open defects

The working list. Updated as things are found and as they are fixed — a defect
leaves this file only when it is verified fixed on the live installation, and
anything reported more than once gets a note saying why it recurred.

Kept here rather than in a session, because the complaint that prompted it was
right: the same issues were being re-reported because nothing held them
between sessions.

Status values: **open** · **in progress** · **fixed (unverified)** · **verified**

---

## Open

### D-1 · New dataset offers no source on a lake-backed installation
**fixed (unverified)** · `81f26b1d` · needs a click to verify

`studio/app/datasets/new/page.tsx:334` reads `/api/v1/data-sources/active`,
which is the family for external database connections (Fabric SQL, Azure SQL,
PostgreSQL, MySQL). This installation has none and never will: it reads the
lake through **catalog sources**, and datasets bind with
`source: {kind: "engine", table_id: …}` (`services/engine_datasets.apply_binding`).

So the picker is empty by construction, and no amount of configuration fills
it. Measured: `/data-sources` returns `{"dataSources": []}` while
`/catalog-sources` returns three active catalogs.

The page needs an Engine-catalog path — pick catalog → schema → table and bind
by table id. The API side already exists.

### D-2 · Dashboard builder: cannot drag or move items
**fixed (unverified)** · `de3f76ec` · needs a click to verify.
NOT caused by `794af12f` — `useContainerWidth` never saw a node, so the grid
was never rendered. That commit was exonerated against the library source.

That commit gave the builder shell `height: 100dvh` + `overflow: hidden` and
made the canvas pane a scroll container. Putting a drag surface inside a new
overflow/scroll context is a known way to break grid drag. Must be checked
against the pre-`794af12f` behaviour before anything else is changed, and
reverted if it is the cause.

### D-3 · The Engine answers any product listing in ~0.5s regardless of size
**open** · Codex owns · logged in HANDSHAKE.md with measurements

Transport is 1ms; a pooled client makes no difference. `read_head()` re-reads
and re-decodes the whole snapshot per request even though snapshots are
immutable and content-addressed, and `fetch_product_document_at` runs in a
sequential loop. Concurrent listings also partially serialize (~0.15s per
added reader). This is the floor under every page in Studio.

### D-4 · Synthetic data does not look like real data
**generator fixed, live data unchanged** · `c9a4cb90` · the cause was a
complete user x day x surface cross product. Rebuilt: 506,542,814 rows,
14.6M UUID users, no two days equal, 20.6% grid fill, and the shape report is
now a build gate. Migration in docs/engineering/events-activity-rebuild.md,
not yet run.

`KaveonTelemetry.events.enriched` holds exactly 504,000,000 rows — 3,000,000
users x 28 days x exactly 18,000,000 rows per day, every day identical. And
`user_id` is a sequential integer (1, 2, 3 …) rather than an opaque
identifier. Both read as generated rather than observed, which undercuts a
demo whose point is credibility. `enriched` is also an ETL stage name, not a
description of content.

### D-23 · Benchmark scratch catalogs are shown to users as registered catalogs
**fixed** · `0519cf33` · 11 catalogs -> 2; measured inventory 36 tables -> 18

The Data catalog page lists 11 catalogs, 8 of them `bench_0dedb97f97ae`,
`bench_2166e3120b2f` and so on — scratch catalogs a benchmark run registers in
the Engine, pointed at `opensource/benchmarks/run-…`. They are presented to a
reader as "Registered catalog" beside OpenSource and KaveonTelemetry.

Two things to settle: the page should not show benchmark scratch at all, and
the harness should remove what it registers when a campaign ends. The second
is Codex's; the first is ours.

### D-24 · The Data catalog page and SQL Lab disagree about what exists
**fixed** · `0519cf33` + `1f0fdbf8` + `81f26b1d` · all three surfaces now read
the same grant-filtered registry; the product catalog's three spellings
resolve to one catalog

The Data catalog page reads **Engine catalog definitions** (11 right now). SQL
Lab reads **catalog source records** (3 active). So a catalog can be
"registered" on one page and absent from the other, which is exactly what was
noticed: if it is registered, it should be queryable, and if it is not
queryable it should not claim to be registered.

This is the same shape as D-10 and D-20: two representations of one fact, with
different surfaces reading different ones and no reconciliation. Worth fixing
as a rule rather than per page — one surface should be derived from the other,
or the two should be reported with their difference made visible.

### D-25 · The Engine's query log does not survive a coordinator restart
**open** · 2026-10-13 · Codex owns the Engine side

`GET /v1/queries` returned 0 rows with a coordinator uptime of 107 seconds,
having returned rows before the restart. So the Engine console's "No queries
yet" is not an empty history, it is a lost one — and the coordinator has been
restarted a dozen times today by benchmark campaigns.

The platform's own `query_history` is unaffected and durable: 1,000 records in
KaveonDB, verified at the same moment. Two different histories, one of which
reads as the other's absence, which is why it was reported as data loss.

### D-26 · An unavailable Engine is reported as an empty catalog
**open** · 2026-10-13

With the Engine scaled to zero, the catalog page reported "0 tables" and "No
tables found". That is the D-10 shape again on a third surface: a failed read
rendered as a verdict. The page should serve the last answer it had and say it
is stale, or say it could not reach the Engine — never claim a catalog is
empty because nobody answered.

### D-27 · Nothing is warmed before the first reader arrives
**open** · 2026-10-13 · the strategy asked for

Every hot read is computed on the first request that needs it, so the first
visit after a restart pays the full cost, and so does the first visit after
anything invalidates. `services/revalidating_cache.py` now exists and serves
the context banner from memory with no Engine round trip; what is missing is
something that fills it before a reader asks, and that keeps the other hot
reads (catalog listings, charts, dashboards, datasets) warm the same way.

### D-28 · Dashboard tabs pass v1 props to react-grid-layout v2
**open** · 2026-10-13 · found while fixing D-2

`studio/components/dashboards/components/DashboardTabsComponent.tsx:19,711`
loads the default export and passes `draggableHandle`, top-level `cols` and
`rowHeight` — v1 props that v2 ignores. A grid nested inside a Tabs component
therefore runs with library defaults and no drag handle.

### D-29 · Inventory ignores the catalog unless a schema is also given
**open** · 2026-10-13 · pre-existing, found while fixing D-24

`/catalog/OpenSource` lists every registered catalog's schemas rather than
that catalog's. Confined to registered catalogs by the D-24 fix, so it is no
longer showing benchmark scratch, but it still shows the wrong scope.

### D-5 · 4,068 query-history rows belong to synthetic accounts
**open, by decision** · 2026-10-13

They belong to 2,232 `@kaveon-demo.io` accounts that never sign in. Left out
of the product family deliberately: as records they would quadruple it and
slow every listing for no reader. Noted so the gap is not rediscovered as a
bug.

### D-6 · Seeded saved-query names contain emoji
**open** · 2026-10-13

e.g. `🚕 NYC busiest pickup zones`. They render in the Library, against the
no-emoji bar in CLAUDE.md. A seed-data fix.

### D-7 · Engine chart is outside CI/CD
**open** · long-standing

`infra/helm/kaveon-engine` is not deployed by any workflow, so every change to
it — including `workers.replicas: 4` and the Trino anti-affinity — has to be
applied to the cluster by hand, and the cluster can silently disagree with the
chart.

### D-8 · ~3.3s product writes
**open** · long-standing

Every append re-encodes its whole typed table. Needs chunking.

---

## Verified fixed

| | what it was | verified |
|---|---|---|
| **D-9** | Context banner took 6.5s on every load: three catalog listings and nine artifact reads, all serial, each artifact read rebuilding an Azure credential (0.24s of re-authentication per read) | 6.48s → 1.76s cold. `dc0299b1` |
| **D-10** | A failed read rendered as a verdict: Settings said "Metadata database not configured — run the setup wizard" and "KaveonDB unavailable — set KAVEON_ENGINE_URL" while both were correct and the coordinator was merely restarting | `e7f8b453` |
| **D-11** | Benchmark left the Engine at zero replicas: the restore sat outside `try`, so any failed round left the platform down | restore in `finally`, verified by a rigged failure. `34cdb5b8` |
| **D-12** | Kaveon worker scheduled onto a Trino node while the spare node sat empty, contaminating the measurement the node was added to protect | required anti-affinity; verified live during a round. `baea94a3` |
| **D-13** | Dashboard previews stored for only 3 of 8: capture sent up to 3,500,000 chars against an API bound of 256,000, and the 422 was swallowed | `74fd75ef` |
| **D-14** | "Refresh outstanding" permanently disabled: staleness depended on a localStorage baseline a fresh browser never has | staleness now from `thumbnail_updated_at`. `74fd75ef` |
| **D-15** | Viewer shown Add chart / Add dashboard / delete / Publish, and both builder entry pages had no role check at all | `72adb3b4` |
| **D-16** | SQL Lab results drew below the composer then jumped to the top: columns publish before rows, and the in-progress panel was in-flow above the grid | `660f87e5` |
| **D-17** | Dashboard builder: chart list did not scroll, canvas did not fill the page | root was `.page-shell` with no height, so every `flex: 1` beneath it was inert. `794af12f` — **see D-2** |
| **D-18** | Recorded row counts never refreshed: the freshness sweep was disabled in the PostgreSQL-free runtime and had no identity to read artifacts with | hourly CronJob, verified running. `0ed5e7a0` |
| **D-19** | Catalog manifest described pre-Delta single-file Parquet objects with stale counts; a rebuild from it would have failed | regenerated from the Engine. `d5d20f6c` |
| **D-20** | Saved Queries tab empty: 6 rows in the typed table, 0 ever backfilled into the product family | all 6 restored, verified |
| **D-21** | `OpenSource` catalog had no declared credential, working only because the pod identity happened to hold account-scope access; its source record also carried `root_path: ""`, which a re-sync would have written over the real prefix | `WorkloadIdentity` declared, reads verified |
| **D-22** | Kaveon Events bound to a 24-row projection instead of the 504M source table | re-pointed; 34 charts verified resolving |
