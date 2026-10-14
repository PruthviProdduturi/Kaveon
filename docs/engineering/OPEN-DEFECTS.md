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
**fixed, browser-verified** · `de3f76ec` + `2285c465`. Two independent
defects: the grid was never rendered (a measurement gate on a ref that was
never attached), and then no tile had a drag surface — `DashboardItem` returns
before the header carrying the handle, so only the tabs fallback ever had one.
A grab strip is now rendered per grid child and declared to the grid, verified
in a browser: the strip drags and reorders, the chart canvas does not, and
resize works from all three handles.
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
**fixed, deployed** · `f769e508` + `d825ac30` · nineteen surfaces plus the home
page, which was the worst of them: a failed read satisfied every condition for
emptiness and the page disabled its own composer. `components/Reading.tsx`
holds the rule. **Residual, open:** `/dlm/coverage` still answers 200 with a
short list when the Engine is down, so no client can tell that from an
uncompiled installation — the `None` token in `revalidating_cache` is the
signal and is not on the response. And `app/lab/LabWorkbench.tsx` still shows
"No tables found" on a failed listing and titles a modal "Metadata database not
configured" from a sessionStorage flag.

With the Engine scaled to zero, the catalog page reported "0 tables" and "No
tables found". That is the D-10 shape again on a third surface: a failed read
rendered as a verdict. The page should serve the last answer it had and say it
is stale, or say it could not reach the Engine — never claim a catalog is
empty because nobody answered.

### D-27 · Nothing is warmed before the first reader arrives
**fixed, deployed** · `d825ac30` · verified live (`Hot-read warming started`).
Banner 2.276s cold against 48.5µs warm. **Residual, open:** only the warmer
reads the listing cache. Adopting it in `services/charts`, `services/dashboards`,
`services/datasets` and the sources listings needs a write hook invalidating on
mutation in the same change, or a reader would not see their own edit until the
staleness bound.

Every hot read is computed on the first request that needs it, so the first
visit after a restart pays the full cost, and so does the first visit after
anything invalidates. `services/revalidating_cache.py` now exists and serves
the context banner from memory with no Engine round trip; what is missing is
something that fills it before a reader asks, and that keeps the other hot
reads (catalog listings, charts, dashboards, datasets) warm the same way.

### D-28 · Dashboard tabs pass v1 props to react-grid-layout v2
**fixed, browser-verified** · `ff7d1a30` · and the consequence was worse than
the missing handle: on v2 defaults a row height of 150 rendered an eight-row
chart 1200px tall, and `width` was hardcoded to 1200 so every tab laid out for
a canvas nobody has.

`studio/components/dashboards/components/DashboardTabsComponent.tsx:19,711`
loads the default export and passes `draggableHandle`, top-level `cols` and
`rowHeight` — v1 props that v2 ignores. A grid nested inside a Tabs component
therefore runs with library defaults and no drag handle.

### D-29 · Inventory ignores the catalog unless a schema is also given
**open** · 2026-10-13 · pre-existing, found while fixing D-24

`/catalog/OpenSource` lists every registered catalog's schemas rather than
that catalog's. Confined to registered catalogs by the D-24 fix, so it is no
longer showing benchmark scratch, but it still shows the wrong scope.

### D-30 · The availability guard was hand-applied and broke every deploy
**fixed** · 2026-10-13 · my own doing, recorded because the lesson generalises

The guard was applied with `kubectl apply` to prove it worked before being
committed. Helm then refused to adopt objects it did not create — "missing
app.kubernetes.io/managed-by" — so three consecutive platform deploys failed
and nothing merged after `4cf47195` was live, while the tests passed and the
commits looked clean.

Testing it live was right. Leaving Helm-unowned objects behind was not: for
anything a chart also manages, the proof has to come from a `helm upgrade`.
Resolved by deleting the four objects and redeploying; the guard is now
`managed-by: Helm` and verified holding the floor.

### D-31 · One Row or Column kills drag and resize for the whole dashboard
**fixed, browser-verified** · `ff7d1a30` · containers are tiles now;
`hasContainers`/`isFlatGrid` deleted. Evidence it mattered: 8 dashboards, 73
items, zero rows/columns/tabs — the feature had never been used once, because
using it was worse than not.

`DashboardCanvas.tsx:97,100,177-181`: if any root item is a `row` or `column`,
`hasContainers` is true, `isFlatGrid` is false, and the canvas renders a plain
vertical stack with no grid at all. So one click of Row or Column in the Items
palette permanently disables dragging and resizing for every tile on that
dashboard, with no indication that it has happened.

The `de3f76ec` commit message noticed this in passing and did not pursue it.
It is cheap now: with a drag handle declared, container items can live on the
grid without their own internal handles competing for the gesture, because
only the declared handle can start a tile drag.

Cold at the moment — every seeded dashboard is flat — so it is reachable only
by someone adding a Row or Column.

### D-32 · The remove control was invisible, and the builder was light-only
**fixed** · `aed9e6e8` · 2026-10-13

"There is no way to remove a chart" — the duplicate and remove buttons were
`opacity: 0` until the component was hovered: clickable, invisible, and on a
touch device genuinely unreachable. Shown while building now.

Underneath it, `dashboard.css` carried 121 literal colours with three behind
tokens, predating the dark theme entirely — the same cause as the white
empty-canvas slab and the light-only chart picker. Fifty-nine declarations
tokenised by what the property does rather than by colour value.

### D-33 · `moveItemToContainer` ignores a caller-supplied position
**fixed** · `42436ae7` · replaced with an insertion index, which is what a
container can honour — a child's position is decided by its parent, so a pixel
was never meaningful there. A second bug surfaced beside it: two of the three
removal paths did not re-split a row's twelve columns, so taking one child out
of a pair left the survivor at half width next to a permanent gap.

`DashboardContext.tsx:655` reads
`y: position?.y ?? (i.children || []).length > 0 ? Math.max(...) : 0`, which
parses as `(position?.y ?? (length > 0)) ? max : 0` — so a caller's
`position.y` is discarded and replaced by the maximum, and `y: 0` arrives at
zero by a different route than intended.

Exposed on the context and called by nothing, so it is currently harmless.
Recorded rather than fixed because a dead API with a latent precedence bug is
a trap for whoever first uses it.

### D-34 · A container taller than its tile spills over its neighbour
**open, needs a judgement** · 2026-10-13

Container tiles keep `overflow: visible` because a row's only delete control
sits at `top: -14px`, outside its own box. So a row whose contents exceed its
tile height overlaps whatever is below until the tile is resized. Clipping
would fix the overlap and hide the delete button.

Neither option is obviously right; it needs a design decision rather than a
patch.

### D-35 · A control written once, rendered for one tile type in six
**fixed twice, cause still open** · 2026-10-13

`DashboardItem` returns early per tile type before reaching the shared header
it renders for tabs. That is why a chart tile had no drag handle (fixed in
`2285c465`) and no remove button (fixed in `42436ae7`) — both controls existed,
were wired, were styled, and were rendered for exactly one of six types.

Both fixes work by moving the control into the canvas, per grid child, where a
tile type cannot miss it. The component itself is still shaped so the next
shared control added to that header will reach tabs and nothing else. Worth
restructuring `DashboardItem` so the header is not reachable only through one
branch — until then, treat anything added there as rendered for tabs only.

Two of my own diagnoses of the remove button were wrong before this was
measured: I believed the grab strip covered it, then that the chart overlay
did. It was never rendered. `elementFromPoint` settled it.

### D-36 · The nesting grip is correct and not discoverable
**open, needs a design decision** · 2026-10-13

Dragging a tile into a row works, but only off a small grip that appears on
hover and only when the dashboard contains a container. "Drag by this square
to put the tile inside that row" is not self-evident.

A hover dropzone or a right-click "Move into…" would read better. Recorded
rather than guessed at, since it is a design choice and the mechanism
underneath is sound.

### D-37 · Insertion position inside a container is modelled, not offered
**open** · 2026-10-13

`moveItemToContainer` takes an index and every caller appends, so a tile cannot
be dropped between two existing children of a row. The model supports it; no
drop target surfaces it.

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

### D-38 · A cube build starves on the memory pool it is itself holding
**verified** · 2026-10-13 · **Engine — @Codex** · `NEXT`

`ANALYZE … WITH (cube = true)` over a 10-axis shape runs one statement per
grouping, concurrently, out of the same admission pool. Each inherits the
default per-query budget of 4 GiB, so two of them fill the 8 GiB pool and the
third waits the full 60 s admission wait and fails the whole build. On a
completely idle cluster, with nothing else running:

```
cube grouping over platform failed: memory admission wait of 60 s expired:
8589934592 of 8589934592 bytes admitted, 1 statements waiting,
resource group 'default' running 2 of 4
```

The API surfaces this as `429 Engine query capacity is temporarily exhausted`,
which reads as contention and sends the reader looking for another tenant.
There is none: the build is competing with itself, and no amount of waiting or
retrying helps.

Fixed at the substatement boundary: cube grouping children divide the parent
statement's effective memory budget by the configured concurrent width before
admission. The bound is always positive, is sent to workers through the normal
task settings, and is covered by
`cube_child_memory_budget_is_shared_across_concurrent_groupings`.

### D-39 · A cube build scans the table once per grouping
**open** · 2026-10-13 · **Engine — @Codex**

Observed during the D-38 build: each grouping is an independent
`SELECT <axis>, COUNT(*), SUM(…) … GROUP BY` over the full table. A 10-axis
shape is 10 single-axis groupings plus the 45 two-axis pairs the cube holds, so
a 504,000,000-row table is scanned on the order of 55 times for one build at
four statements at a time. One scan feeding every grouping is the same work
once.

### D-40 · A column deletes with no confirmation, unlike every other tile
**open** · 2026-10-13

`DashboardColumnComponent`'s delete calls `removeItemFromContainer` /
`removeLayoutItem` straight from `onClick`. Every other tile type routes
removal through `ConfirmModal` first. A column holds children, so it is the
tile where an accidental delete costs the most, and it is the one that does not
ask. Found while verifying that each tile type owns a remove control for the
D-33 follow-up.

### D-41 · The column toolbar is hardcoded light
**open** · 2026-10-13

Its buttons declare `background: '#fff'`, `border: '1px solid #e2e8f0'` and
`color: '#64748b'` inline, so the column's own controls stay a white cluster on
a dark canvas. Same shape as the Customize tab and the Add charts dialog: a
surface written before the dark theme and never mapped to tokens.



### D-42 · Every non-ASCII character in every SQL statement is corrupted
**open** · 2026-10-13 · **Engine — @Codex** · data integrity

`engine/crates/sql/src/parser.rs`, `normalize_bracket_identifiers`: the
function walks the statement with `sql.as_bytes()` and rebuilds it with
`out.push(b as char)`. In Rust `u8 as char` is a Latin-1 to Unicode
conversion, so UTF-8 byte 0xF0 becomes U+00F0 and the String re-encodes it as
0xC3 0xB0. Every statement passes through this function on its way to the
parser, so every non-ASCII character in any statement is replaced by its
Latin-1 misreading — identifiers and string literals alike.

Demonstrated with a literal and nothing else:

    sent      SELECT 'Probe (taxi emoji) en-dash' FROM events.dashboard LIMIT 1
    returned  the same string with each non-ASCII byte expanded to its
              Latin-1 character

Product records are written as SQL literals, so this silently corrupts stored
data, once per write. The read path is innocent: the wire bytes for a stored
name are already the double-encoded form at rest.

**Corrupted data now in the catalog.** The six seeded saved queries carry one
pass each. A chart named `New Chart - Oct 13, 2026` (with an en dash) carried
three, because the passes compound on every rewrite — it had been saved three
times. Repair is `encode('latin-1').decode('utf-8')` applied once per pass,
and must wait until the write path is fixed or it will simply be re-corrupted.
A helper already exists at `api/services/dlm_run_backfill.py:61`.

### D-43 · A maintenance build takes capacity from interactive queries
**open** · 2026-10-13

A cube build and a chart compete in the same resource group, so while a build
runs the product degrades: a chart that takes 33 s on an idle cluster was
measured at 158 s total against 28.7 s of server time — 130 s of it admission
wait behind the build's own scans. D-38 fixed the build starving itself; it
does not stop the build starving everything else.

The Engine already supports the separation: `resource_groups` selectors match
on `client_tag`, and `engine_bridge` already stamps a client tag on every
statement. Maintenance work — `ANALYZE`, cube builds, the freshness sweep —
should carry a reserved tag and be bound to a small group, so a build can
never consume the interactive share.

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
