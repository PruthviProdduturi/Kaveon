# Local Engine and Studio Improvement Plan

Status: active local work plan  
Scope: Docker-based Kaveon only; no AKS or cloud dependency  
Owners: Track A = Codex, Track B = Claude  

## Objective

Improve Kaveon locally in two coordinated tracks:

1. Track A: add a narrow, tested Fabric/T-SQL compatibility layer to the Kaveon
   Engine.
2. Track B: harden and polish Studio maps and query interaction without adding
   redundant chart types.

This plan does not claim full Fabric compatibility. Unsupported features must
remain explicitly documented until they have parser, planner, executor, and
distributed tests.

## Current baseline

The local Docker stack is the qualification environment:

- Kaveon API: `http://127.0.0.1:8082`
- Kaveon Engine: `http://127.0.0.1:8081`
- Studio: `http://127.0.0.1:3000`
- Coordinator plus two workers
- PostgreSQL-free KaveonDB authority
- OpenSource Delta/Parquet catalog bootstrapped from `data/adls-mirror`

Already present in the Engine:

- Bracket identifiers
- `OFFSET ... FETCH`
- `CAST` and `TRY_CAST`
- Vectorized aggregation and distributed execution paths

Already present in Studio:

- World, USA, and NYC maps
- Flat and globe modes
- Automatic geographic framing
- Zoom and pan
- Labels, legends, number formatting
- Dashboard cross-filtering

## Track A — Engine Fabric/T-SQL compatibility

Owner: Codex (`/root/engine_tsql_slice`)

### A1. Completed first slice

- `TOP n`
- `TOP (n)`
- Actionable rejection for `TOP PERCENT`
- Actionable rejection for `TOP WITH TIES`
- `ISNULL(value, fallback)` lowered to the existing `COALESCE` path
- Focused tests for `TOP`, `ISNULL`, and `OFFSET/FETCH`

### A2. Date and time functions (P0)

Implement and test:

- `DATEADD`
- `DATEDIFF`
- `DATEPART`
- `GETDATE`
- `GETUTCDATE`
- `YEAR`, `MONTH`, and `DAY`
- `CONVERT`
- `TRY_CONVERT`

Required semantics:

- Correct null behavior
- Correct UTC and timestamp behavior
- Negative intervals
- Month-end and leap-year handling
- Invalid conversion behavior
- Identical local and distributed results

### A3. Planner and executor coverage (P0)

Verify every new function in:

- Projection
- `WHERE`
- `GROUP BY`
- `HAVING`
- `ORDER BY`
- Window expressions
- Join predicates

Prefer vectorized execution. Do not introduce a row-by-row fallback without a
benchmark and an explicit limitation note.

### A4. Query-shape compatibility (P1)

After A2 and A3 are green, evaluate:

- `PIVOT`
- `UNPIVOT`
- Supported `CROSS APPLY`
- Supported `OUTER APPLY`
- `QUALIFY`
- T-SQL alias and identifier edge cases

Do not implement `MERGE`, stored procedures, cursors, triggers, or procedural
batches in this tranche. Those require separate transaction and recovery
design work.

### A5. Engine performance qualification (P1)

Benchmark these query families locally:

- `TOP ... ORDER BY`
- Date filtering
- Date grouping
- `ISNULL` projections
- Aggregation over the 500M-row benchmark table
- Single-worker versus distributed execution

Capture planning time, scan time, CPU time, exchange time, serialization time,
rows scanned, and rows returned.

## Track B — Studio maps and query experience

Owner: Claude (`/root/ui_map_polish`)

The existing chart catalog is already broad. Do not add duplicate map or chart
types during this plan.

### B1. Map reliability (P0)

- ISO-2 and ISO-3 normalization
- Country-name aliases
- Region, state, and city validation
- Explicit unknown-geography bucket
- Null-geography messaging
- Clear diagnostic when the selected column is not geographic
- Retry after failed GeoJSON loading
- Explicit no-matching-geography state

### B2. Map interaction (P1)

- Click-to-filter
- Hover tooltip with formatted metrics
- Reset map filters
- Persistent dashboard filter state
- Cross-filter synchronization with charts

Amended 2026-10-28 (Claude). "Country-to-region-to-city drilldown where the
data supports it" is withdrawn from Track B and is not a UI task. The three
bundled GeoJSON files (`world.json`, `usa.json`, `nyc.json`) carry no
hierarchy between them, and a dataset has no key relating a country row to a
state row — the map renderer picks one region file from a chart option and
joins place names against it. Drilldown needs, in order:

1. A geography hierarchy in the dataset model: which column is the parent
   level, which is the child, and the key that joins them.
2. GeoJSON for the intermediate levels a drilldown would descend into, which
   Kaveon does not ship beyond the three above.
3. A query path that re-aggregates at the child level under the parent's
   filter, which is a server concern rather than a renderer one.

Until those exist, click-to-filter plus cross-filter synchronisation is the
honest version of the capability: clicking a country narrows every tile to it,
including a tile already grouped by a finer level. Re-open drilldown as its own
tranche with (1) specified, not as a Studio polish bullet.

### B3. Large-result rendering (P1)

- Server-side aggregation or result bounds before map rendering
- Loading skeleton
- Empty state
- Error state
- Retry state
- Rendering performance telemetry

Amended 2026-10-28 (Claude). "Progressive rendering for large geographic
results" is withdrawn. ECharts' `progressive` and `progressiveThreshold` apply
to series that draw one element per datum — scatter, lines, custom — and have
no effect on `type: "map"`, which draws one path per map feature regardless of
how many rows arrived. There is no partial-paint knob to turn, so the bullet
cannot be satisfied in the renderer Studio uses.

The underlying concern — a large geographic result taking a visible moment —
is addressed by bounding the result instead, which is the bullet above it: a
choropleth can draw at most one region per feature (215 in `world.json`, 52 in
`usa.json`, 5 in `nyc.json`), so rows beyond that were never going to appear.
`MAX_GEOGRAPHIC_ROWS` caps a map result at 1,000 in both the generated
statement and at execution, ranked by the chart's metric so the cap keeps the
largest regions, and the map says on screen when its result was capped.
Rendering telemetry is in place to show whether what remains is actually slow;
if it is, the next step is a measurement, not a progressive-render flag.

### B4. SQL Lab interaction (P1)

- T-SQL compatibility hint or dialect notice
- Clear unsupported-syntax errors
- Cancellation when leaving a page
- Query-result cache indicator
- Row-count and elapsed-time summary
- Separate execution and rendering states

## Track C — Shared compatibility corpus

Owner: Codex integrates; Claude contributes UI-facing fixtures.

Create a local corpus containing:

- 25 basic T-SQL queries
- 25 date/time queries
- 20 null and cast queries
- 20 joins and aggregations
- 10 window queries
- 10 unsupported-feature diagnostics

Every fixture records:

- Input SQL
- Expected result or expected error
- Local/distributed status
- Execution time
- Rows scanned
- Rows returned

## Track D — Documentation

Owner: Codex after behavior is verified; Claude supplies Studio examples.

Update or create:

- `docs/engine/sql-compatibility.md`
- Engine SQL examples
- Fabric/T-SQL support matrix
- Unsupported syntax matrix
- Map data requirements
- Local Docker qualification commands

Do not document a feature as supported until it has parser, planner, executor,
and distributed coverage.

## Track E — Qualification gates

Run locally before merging either track:

```powershell
cargo test -p kaveon-sql --all-targets
cargo test --workspace
npx tsc --noEmit
python -m services.postgresql_free_smoke_cli
python scripts/test_bootstrap_local_catalog.py
python scripts/test_local_kavedb.py
node scripts/validate-docs.mjs
```

Then run representative queries through the local Docker Engine and Studio.

The SQL integration test path is currently covered by a broad `**/tests/`
ignore rule. Before committing `engine/crates/sql/tests/tsql_compat.rs`, either
add a narrow unignore rule or force-add the intended test file and verify it is
present in the commit.

## Handoff and change-isolation rules

- Track A must not rewrite Studio files.
- Track B must not modify Engine parser, planner, or executor files.
- Preserve unrelated dirty work in the shared tree.
- Report files changed, tests run, and unsupported behavior after each slice.
- Do not commit broad formatting or generated files with either track.
- A track is complete only when its acceptance tests and documentation are
  green.

## Completion definition

This plan is complete when:

1. The supported T-SQL subset passes the compatibility corpus locally in both
   local and distributed execution.
2. Studio maps handle valid, invalid, empty, and large geographic results with
   reliable loading, retry, and filtering behavior. (Amended 2026-10-28:
   drilldown withdrawn from this tranche — see B2.)
3. All qualification commands pass.
4. The support matrix and user-facing docs match the tested implementation.

Full Fabric parity remains a separate future program, not an implied result of
this local tranche.
