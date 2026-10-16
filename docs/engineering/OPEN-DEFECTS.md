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
**fixed** · 2026-10-14 · pre-existing, found while fixing D-24

`/catalog/OpenSource` listed every registered catalog's schemas rather than
that catalog's. The scope lived in the schema filter, which only narrowed when
a SCHEMA was also given — with a catalog and no schema, `!schemaName` was true
and everything passed. The catalogs are narrowed before their schemas are
listed instead, which also stops the page listing schemas for every catalog in
order to show one; at ~0.5 s per listing (D-3) that was most of its wait.
Matched case-insensitively, to agree with the check that decided the catalog
exists.

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
**fixed** · 2026-10-13

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
**fixed** · 2026-10-13

Dragging a tile into a row works, but only off a small grip that appears on
hover and only when the dashboard contains a container. "Drag by this square
to put the tile inside that row" is not self-evident.

A hover dropzone or a right-click "Move into…" would read better. Recorded
rather than guessed at, since it is a design choice and the mechanism
underneath is sound.

### D-37 · Insertion position inside a container is modelled, not offered
**fixed** · 2026-10-13

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
**fixed** · 2026-10-13

All six repaired in place and verified ASCII: `🚕 NYC busiest pickup zones`
(stored as `ð NYC busiest pickup zones`, D-42) became
`NYC busiest pickup zones`. Stripping the emoji removed both this and the
mojibake in one pass, and leaves names the corrupting write path cannot damage
again — which is why this repair is permanent while D-42 is still open.

### D-7 · Engine chart is outside CI/CD
**fixed (unverified)** · `.github/workflows/deploy-aks-engine.yml` · the
workflow has never run; rendering could not be checked locally because Helm is
not installed on this machine

`infra/helm/kaveon-engine` was deployed by no workflow, so every change to it —
`workers.replicas: 4`, the Trino anti-affinity, the node-sized resource limits
— was applied to the cluster by hand, and the cluster could silently disagree
with the chart.

**Deliberately not a copy of the platform deploy.** A push renders, lints,
checks ownership and prints the drift; it does not touch the cluster. Applying
is a `workflow_dispatch` with `mode: apply`. The coordinator is one replica
with the catalog on a PVC, it is not HA, running queries do not resume across a
restart, and this cluster also hosts the benchmark campaign and long
maintenance jobs — so a push-triggered `helm upgrade` would roll the
coordinator and every worker whenever anyone edited the chart. The continuous
half is the drift report; rolling the Engine stays a decision somebody makes.

Authentication, cluster targeting and the `environment: production` gate are
the platform deploy's: `azure/login@v2` with OIDC, `azure/use-kubelogin@v1`,
`az aks show` state check, `az aks get-credentials`, `kubelogin
convert-kubeconfig -l azurecli`.

Four guards, each a thing that has gone wrong before:

- **Adoption (D-30).** Every object the render names is looked up with
  `kubectl get -f` and must already carry `app.kubernetes.io/managed-by=Helm`
  plus `meta.helm.sh/release-name=kaveon` and
  `meta.helm.sh/release-namespace=kaveon`. Anything else is refused up front,
  with the `kubectl label`/`annotate` commands printed. Checked against the
  live cluster: all seven objects already carry them, and release `kaveon`
  exists at revision 1 (`deployed`), so no adoption work is needed. The release
  was installed by hand on 2026-10-09 from the chart's old name `kaveon-test`,
  which is why its recorded values are stale — the workflow passes
  `--reset-values` and supplies everything that matters, so none of them can
  survive.
- **Replicas are preserved, not declared.** The live counts are read back and
  passed through, clamped up to the availability guard's floor. The chart says
  four workers; the cluster runs three. A deploy that rewrote the count would
  take a node back from a benchmark round in flight, or restore four during
  one. Setting them is an explicit input.
- **The image is preserved.** The digest running on the coordinator is read
  back and re-pinned, so a chart-only deploy changes no image and the qualified
  image stays. A new one is an explicit input. The product-transactions
  account, container and prefix are read back the same way and a change to any
  of them is refused unless asked for — a prefix one segment short of the
  catalog is indistinguishable from a new installation.
- **Restarts are accepted, not discovered.** The rendered pod template is
  compared field by field against the running one (image, env values and their
  order, resources, nodeSelector, affinity, volumes) and apply is refused
  unless `accept_restart` is given, with the differing fields printed.
  Separately, apply is refused while any non-CronJob Job is active or the
  benchmark engine is up.

No `--atomic`, unlike the platform deploy: a rollback would roll the
coordinator and the workers a second time, and a failed upgrade left standing
can be read and decided on.

**The one decision waiting for a human.** The chart and the cluster disagree
about the exchange spool. The chart declares `workers.exchange.spool: true` and
`coordinator.exchange.spool: false`; the cluster runs the inverse
(`KAVEON_COORDINATOR_EXCHANGE_SPOOL=true`, `KAVEON_WORKER_EXCHANGE_SPOOL=false`
— the posture release 1 was installed with). That is a behavioural difference,
not just a restart: it decides whether producers upload straight to the
consuming worker or the coordinator relays. The first apply flips it and rolls
every pod, which is why `accept_restart` exists and defaults to off. Decide
which side is right and make the other match before the first apply.

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
**fixed** · 2026-10-14

`DashboardColumnComponent`'s delete calls `removeItemFromContainer` /
`removeLayoutItem` straight from `onClick`. Every other tile type routes
removal through `ConfirmModal` first. A column holds children, so it is the
tile where an accidental delete costs the most, and it is the one that does not
ask. Found while verifying that each tile type owns a remove control for the
D-33 follow-up.

### D-41 · The column toolbar is hardcoded light
**fixed** · 2026-10-14

51 literal colours across inline style objects, hover handlers and `border`
shorthand, so the column's own controls stayed a white cluster on a dark
canvas. Mapped by role rather than by value — 49 replaced; the two that stay
literal are the drag ghost, a deliberate dark chip that follows the cursor
rather than sitting on the page, and a divider's own stored colour, which is a
reader's setting rather than chrome. Both say so in place.



### D-44 · The read warmer never runs before the first reader
**partly fixed** · 2026-10-14

`warm_reads` starts with the API and is on by default, but it only warms
principals it has already served or ones named in `KAVEON_WARM_PRINCIPALS` —
and that variable is empty in the pod and was declared in no chart. On a fresh
replica the recently-seen ring is empty, so the warmer warms nothing until
somebody takes the cold read. The person who would benefit is the person who
pays for it.

That matters more than it looks, because deploy-on-push is deliberate until
the platform is stable: every push replaces the pod, so every push puts the
next reader on the cold path. It is a large part of the intermittent
"Engine is unavailable" and slow-first-load reports.

`api.warmPrincipals` now exists in the platform chart and renders
`KAVEON_WARM_PRINCIPALS` when set. It is deliberately empty: naming a reader
puts their address in a public repository, so the value is an installation
decision rather than a committed default. Until it is set, the behaviour is
unchanged.

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
**fixed, with a residual** · 2026-10-14

A cube build and a chart compete in the same resource group, so while a build
runs the product degrades: a chart that takes 33 s on an idle cluster was
measured at 158 s total against 28.7 s of server time — 130 s of it admission
wait behind the build's own scans. D-38 fixed the build starving itself; it
does not stop the build starving everything else.

**The binding constraint is concurrency, not memory**, which is why bounding
the build's memory does not help. The default group allows four concurrent
statements and a cube build issues exactly that many groupings, so it takes
every slot:

```
during build — admitted: 4294967296 of 8589934592
  group default running 4 queued 0
chart statement: 429 after the full 60 s admission wait
```

Half the memory pool was free and the chart was still refused. Bounding the
parent to 2 GiB was worse — the children filled all 8 GiB — and at 1 GiB the
chart is still rejected on slots alone. So a build cannot be run safely on this
cluster at any memory setting, and the cube on
`KaveonTelemetry.events.enriched` is deliberately NOT built: three attempts
were cancelled rather than left to degrade the product.

The separation the Engine already supports: `resource_groups` selectors match
on `client_tag`, and `engine_bridge` now stamps `kaveon:maintenance` on every
`ANALYZE` (`MAINTENANCE_CLIENT_TAG`, covered by
`services/test_maintenance_tagging.py`). What is missing is the group itself —
there is no `resource_groups` configuration anywhere in the repo, so the
cluster runs the single built-in `default` group. A maintenance group with its
own small `max_concurrent` and share, selected by that tag, is the remaining
half and is Engine/deploy configuration.

Note the interaction with D-7: the groups live in the coordinator's runtime
copy, not in the chart, and the chart's `resourceGroups.enabled` must stay off
for that copy to remain editable. The deploy workflow refuses any render that
names `KAVEON_RESOURCE_GROUPS` unless the dispatch explicitly asks for the
takeover, so turning that flag on cannot happen as a side effect of a deploy.

**Applied and measured.** The document in
`infra/helm/kaveon-engine/examples/resource-groups.maintenance.json` is live as
the coordinator's runtime copy. With a cube build running:

```
   default      running=0 queued=0 admitted_bytes=0          limit=8589934592
   maintenance  running=2 queued=0 admitted_bytes=536870912  limit=2147483648
chart statement: 69.77 s, admission wait 0 ms
```

The refusal is gone and the chart no longer queues at all, where before it took
429 after the full 60 s wait. **The residual**: 69.77 s against ~30 s on an
idle cluster. Admission governs memory and slots, not CPU and disk, so a build
still competes for the workers themselves. Tightening maintenance to one slot
and parallelism one would narrow that further at the cost of a much longer
build; left as it is, because the cube is a one-off and makes these
aggregations answerable without a scan afterwards.

### D-45 · A ranking was led by the groups with no data
**fixed** · 2026-10-14

"Top 10 countries by energy consumption" answered with ten entities that have
no energy figures at all — `Africa (Ember)`, `Curacao`, `OPEC (Shift)` — each
shown as 0. Their `SUM` is NULL, and `ORDER BY … DESC` sorts NULL first, which
is standard SQL and what PostgreSQL and Trino both do. So the statement was
correct and the answer was useless.

Ranked statements now carry `HAVING <aggregate> IS NOT NULL`. Chosen over
`NULLS LAST` because the builder serves every dialect and SQL Server/Fabric has
no `NULLS LAST`, and because it reads better: a group with no measurement does
not belong in a ranking of that measure at all rather than belonging at the
bottom. Only where the aggregate can be null — a COUNT never is, so a count
ranking keeps its clean statement instead of carrying a clause that is always
true. The unranked shape the cube answers is untouched.

### D-46 · The energy dataset's country dimension contains aggregates
**fixed** · 2026-10-14

With D-45 fixed, the same question answers `World`, `Non-OPEC (EIA)`,
`High-income countries`, `OECD (EI)` — real figures, but not countries. The
`country` column of `climate_energy.energy` holds regional, economic and
organisational aggregates alongside countries, as the upstream data does.

`iso_code` separates them cleanly: every aggregate has NULL and every country
has a code, and filtering on it gives the right answer — United States,
China, Russia, Japan, India, Germany, Canada, France, United Kingdom, Brazil.

There was nowhere to put that rule: a dataset's `filters` were equality pairs,
so `iso_code IS NOT NULL` was inexpressible, and dropping the aggregate rows
would be wrong because `World` is a legitimate answer to a question about the
world.

Datasets now carry `row_filters` — structured clauses, not SQL, with a
whitelisted operator set extended with `IS NULL`/`IS NOT NULL`. They render
through the dialect, so an unknown operator degrades to equality rather than
being interpolated, and a malformed clause is dropped rather than raised:
these come off a stored document, and a renderer that threw would take down
every question asked of that dataset.

Applied by BOTH assemblers. The ask path and the chart path build statements
separately, and a predicate honoured by only one would make a chart and an
answer disagree about which rows the dataset contains. The chart route puts
them ahead of the chart's own filters so clearing a filter cannot drop them.
They also apply to the unranked shape the cube answers, so a cube-answered
question and a scanned one agree.

Set on `Global Energy`, and the question now answers: United States, China,
Russia, Japan, India, Germany, Canada, France, United Kingdom, Brazil.

### D-47 · A presentation word routes the question to another dataset
**fixed** · 2026-10-14 · the worst of the three

After two COVID questions, "can you make a world map chart" answered with
`SELECT SUM(charts_created) FROM events.daily_user_activity LIMIT 1` — the
telemetry dataset — and timed out at 504 against the billion-row table.

`_ROUTE_NOISE` holds aggregate words and stopwords only: `sum avg count total
max min the of by`. Nothing about presentation. The telemetry dataset's metrics
are `charts_created`, `queries_run`, `dashboards` — so "chart", the single most
common word in a request for a visualisation, scores against it at the x3
metric weight. "give me a chart" survived; adding "world map" pushed it over
the floor.

The fix is not simply to add `chart` to the noise list: that dataset's subject
genuinely IS charts, so "how many charts were created last week" must still
route to it. The real defect is that a FOLLOW-UP asking for a different
presentation of the same answer re-routes at all. A follow-up that introduces
no new subject should keep the conversation's dataset.

Fixed by what separates the two readings: the REQUEST FORM. A request to
produce a picture — an imperative plus the name of a presentation — is a
follow-up even with none of the connectives or pronouns `_FOLLOW_UP_RE` looks
for, so it keeps the conversation's dataset and the router's opinion about its
wording is ignored. "how many charts were created last week" matches neither
half and still routes on its own merits, which a noise-list fix would have
broken.
### D-48 · "Total across the world" answers with a breakdown
**fixed** · 2026-10-14

"total number of covid deaths across the world" returned 50 rows, one per
country, titled "Deaths by country", with "...and 45 more. Ask for top 10 to
narrow down". The question asks for one number. `across the world` reads as a
grouping cue where it is a scope cue, and `total` is in `_ROUTE_NOISE` so it
carries no weight at all.

Fixed: a whole-population phrase cancels the grouping cue. "across the world",
"worldwide", "globally", "overall", "in total" scope the question to
everything; "across countries" and "across all countries" still group, which
is the distinction the pattern has to keep.
### D-49 · The thinking line loops instead of reporting work
**fixed** · 2026-10-14

"Analyzing the data model / Building the query / Finding the right approach"
cycle while a question runs, repeating rather than tracking the stages the
question actually goes through — routing, context lookup, statement assembly,
execution. The DLM knows which of those it is in; the line does not say.


Fixed as far as honesty allows. The phases advanced on a second interval with
`% length`, so a ten-second question went "Understanding your question" twice
and claimed to be analysing a data model it had finished with eight seconds
earlier. There is no stage signal on the ask endpoint — it is one POST — so
this cannot report true progress and no longer pretends to: the label is
derived from elapsed time off a single timer, advances once in order, and holds
on "Running the query", which is where a long wait is actually spent. Real
stage reporting needs the endpoint to stream them.
### D-50 · `--danger` was never defined, and its two fallbacks disagreed
**fixed** · 2026-10-14

Seventy-four call sites carried it as a fallback — fifty-four
`var(--danger, #ef4444)` and twenty `var(--danger, #dc2626)` — so the product
had two different reds for one meaning and neither followed the theme.
`--success` and `--warning` existed, but only in the light block.

Measured rather than picked: on white the minority value is the better one
(#dc2626 at 4.83:1 against #ef4444's 3.76:1), and on the dark surface the red
was the weak member of the set at 4.43:1 where the green and amber already
passed. All three are now defined in both themes, with danger at 6.03:1 on
dark and the other two lifted a step so the set reads as one family rather
than one corrected member beside two inherited ones.

Found by the agent sweeping dashboard colour literals, which reported all three
as undefined; two of them were defined, and the correction mattered because it
changed which value was wrong.


### D-51 · A two-axis grouping costs a full scan the first time it is asked
**open** · 2026-10-15 · **Engine — @Codex**

Over the 1,017,865,653-row `KaveonTelemetry.events.daily_user_activity` with a
built cube, single-axis groupings answer from the cube and two-axis groupings
do not. Measured through the chart path and through raw SQL, both agreeing:

```
actions by surface           0.78 s   cube
users by region (DISTINCT)   0.55 s   cube
avg latency by region        0.62 s   cube   (AVG decomposed to SUM + COUNT)

region x industry   run 1  122.92 s   runs 2-5  0.63-0.70 s
platform x license  first  116.50 s
segment x deployment first 119.68 s
region x country    first  121.26 s
```

Every FIRST execution of a distinct pair costs a full scan; repeats are fast
because the result cache serves them, not because a cuboid does. `platform x
license` has at most 12 cells and still scanned, so cell count is not the
reason.

The build reported `cube_cells: 290`, and that number is now fully accounted
for: 1 grand total + 233 single-axis cells + 56 `event_date` days = 290.
Eleven axes, not ten.

Two earlier readings of that 290 were wrong and are corrected here rather than
left standing. The first assumed ten axes summing to 148 cells and concluded
141 cells were unaccounted for, which suggested the cube held pair cuboids it
would not serve. It did not. The time axis was the missing eleventh and one
country count was short by 85; there was never a surplus.

The open question — whether pair cuboids were stored and not matched, or
counted and not stored — has an answer, and it is neither. **No pair was ever
planned.** `TableShape::groupings` admitted a pair only when
`cap_left x cap_right` fit `CUBE_PAIR_MAX_CELLS` (1,000,000), and a dimension
with no declared cap takes `DEFAULT_DIMENSION_CAP` of 10,000, so every pair in
the table priced at 100,000,000 cells and every pair was declined. The build
reported success because by its own rule it had succeeded. `platform x license`
is twelve cells and was priced at a hundred million.

Fixed in `cec6a098`: pairs are admitted on the product of the counts the build
actually observed. The silence that hid it is fixed separately — a declined
pair was dropped with a `continue`, so the planner now returns a refusal with
one of four reasons (`CountUnknown`, `AxisEmpty`, `PairLimit`,
`CubeCellBudget`) and the cube document records them in `declined_pairs`
beside the groupings it holds.

Residual: the build goes from twelve statements to sixty-six, and the first
read of a pair is not cheap even once planned.

Consequence for the product: a chart grouping on one dimension is now about
fifty times faster than before, and a two-dimension chart costs about two
minutes the first time each combination is viewed.


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
