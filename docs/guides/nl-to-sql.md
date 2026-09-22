# NL→SQL: How Natural Language Queries Work

Kaveon's homepage lets you ask questions in plain English and get back charts, with **no LLM dependency**. Three engines sit behind it, tried in order:

1. **DLM (Data Language Model) — the primary path.** A per-dataset compiled context artifact in the API. It resolves the question deterministically and, for common cases, **answers from precomputed context with no database scan at all** — returning a result badged **"From context · no scan"**. Only novel slices fall through to a single live query, badged **"Live query · Xs"**. Over an Engine table the context is the Engine's own cube and statistics, and the badge is the Engine's word ([Over Engine tables](#over-engine-tables)). Every answer carries its evidence: the statement, the source and the version it reflects, the lane, and a block that reproduces it live.
2. **ACR (Adaptive Context Routing) — the middle tier.** When the DLM can't answer (e.g. the shape doesn't match), the in-browser template parser generates SQL and ACR decides whether to serve the answer from cached context or run the query live.
3. **Template parser — the fallback.** A keyword-based parser (`studio/utils/nlToSql.ts`) that runs entirely in the browser. It handles shapes neither the DLM nor ACR can build (mainly time-series trends, comparisons, distributions).

All three are deterministic — same question, same answer — and none calls a hosted model.

---

## Three-Tier Execution Flow

```
User types question
       │
       ▼
1. DLM (POST /api/v1/dlm/ask)   ── PRIMARY
   Route → which dataset
   Resolve entity filters from value index
   Match metric (name / synonyms / curated aliases)
   Detect group-by, top-N, year filter
   Answer from precomputed context?
        ├─ yes → INSTANT ANSWER   (in-memory dict hit)    ⚡
        ├─ HLL sketch covers it? → APPROX ANSWER (~1-2%)  ⚡
        └─ no  → assemble ONE live query → execute
   Context hints shown while live query runs
       │
       ▼  (only if DLM returns no result)
2. Template parser + ACR
   Dataset auto-detection (score schemas against query)
   nlToSql(query, schema) → 7 patterns, fuzzy matching, SQL
   ACR decides: answer from profile cache, or run live
       │
       ▼  (only if ACR unavailable or no match)
3. Direct SQL execution
   Execute the template parser's SQL directly
       │
       ▼
InlineChart renders ECharts in the conversation
Route badge + timing shown on every answer
```

---

## DLM Overview

The DLM is a **compiled context artifact** — one per dataset — that encodes everything Kaveon knows about that dataset: structure, value inventory, statistics, metrics, and usage patterns. It is an *encode* step, not a training step: no model weights, no embeddings, no hosted LLM.

Storage tables (self-migrating, in the metadata DB):

| Table | Purpose |
|-------|---------|
| `dlm_artifact` | Manifest (columns, joins, metrics, synonyms), stats rollup, source hash, status |
| `dlm_value_index` | Every distinct value of every low-cardinality dimension, normalized for matching |
| `dlm_router` | Per-dataset summary + keyword bag for cross-dataset routing |
| `dlm_answers` | Precomputed answer rows (totals, breakdowns, 2-dim combos) |
| `dlm_sketch` | HyperLogLog register vectors for approximate COUNT(DISTINCT) |

Generation (`generate_dlm()`) runs these steps:

1. **Fingerprint** the dataset definition for cheap change-detection
2. **ANALYZE** the tables, then build context snapshots via `context_profiler`
3. **Value inventory** — bounded `GROUP BY` scan per low-cardinality dimension (cap: 1,000 distinct values). Falls back to `pg_stats.most_common_vals` when unavailable
4. **Usage rollup** — how often each table has been queried (from `query_history`)
5. **Stats rollup** — cardinalities, row counts, date range (metric-coverage-bounded)
6. **Manifest** — the deterministic assembler's map: columns, joins, metrics, synonyms, context spec
7. **Persist** artifact + value index + router summary
8. **Precompute answers** — every metric's grand total, per-dimension breakdowns, 2-dim combos, and HLL sketch cuboids

---

## How Questions Are Routed

### Dataset routing (`route()`)

When a question arrives at `ask()`, the DLM first determines which dataset it targets. Each compiled artifact's manifest is scored against the question tokens:

```
+4  dataset name match (stemmed)
+3  metric name/expression/synonym match
+2  indexed value match (e.g. "Japan" exists in this dataset)
+1  column name match (capped at 3 to prevent broad datasets dominating)
```

A floor of 2 prevents a single generic word from routing. Ties break toward narrower datasets (fewer columns = more focused).

### Entity/value resolution (`_resolve_entity_filters()`)

Next, the DLM extracts entity filters from the question by scanning n-grams (3-word, 2-word, then 1-word) against the value index:

- **Exact normalized match** — "Japan" → `country = 'Japan'`
- **Alias expansion** — "USA" / "America" → `country = 'United States'`
- **Fuzzy match** (edit distance) — "Paskistan" → `country = 'Pakistan'`

One filter per column; longest match wins. Stopwords ("in", "for", "by") are excluded so "in" doesn't match "India".

### Metric matching (`_match_metric()`)

The question tokens (minus generic quantifiers like "total", "count", "average") are compared against each metric's name, expression, and curated aliases. The best overlap wins. Falls back to the curated default metric, then the first defined metric.

### Group-by detection (`_match_group_by()`)

Phrases like "by country", "per region", "across segments" are parsed and resolved against the dataset's dimensions, including curated aliases and synonym expansion. Plural/singular normalization and 4-char prefix matching handle typos and inflections.

### Top-N detection

"Top 10 countries by consumption" sets `top_n = 10` and infers the group-by dimension from the question. "Top models" without a number defaults to 10.

### Year filter (`_extract_year()`)

A 4-digit year (1900–2099) in the question becomes a date filter. Smart handling:

- If the year spans the dataset's entire date range (e.g. a 2026-only dataset asked "in 2026"), the filter is dropped — enabling a context hit instead of a full scan
- If the year is beyond where the metric has data, the latest available year is used and a note explains the substitution

---

## What it can answer

Every question the DLM answers is one of a **named class**. A class has a
name, one SQL shape and one answer sentence; the list lives in
`api/dlm/classes.py` and is what `python -m dlm.coverage` reports per class,
so the list here and the list the harness measures cannot drift apart. The
measured numbers are in
[docs/qualification/dlm/](../qualification/dlm/coverage-2026-09-22.md).

**Base classes** — one statement over the slots the router resolved.

| Class | Example | SQL shape |
|---|---|---|
| `total` | "total queries run" | `SELECT <agg> FROM t` |
| `breakdown` | "queries run by region" | `… GROUP BY d` |
| `filter` | "queries run in Europe" | `… WHERE d = v` |
| `filter_breakdown` | "queries run by country in Europe" | `… WHERE d1 = v GROUP BY d2` |
| `two_filters` | "queries run in Europe Enterprise" | `… WHERE d1 = v1 AND d2 = v2` |
| `top_n` | "top 5 countries by queries run" | `… GROUP BY d ORDER BY 2 DESC LIMIT n` |
| `distinct_total` | "distinct users" | `SELECT APPROX_COUNT_DISTINCT(c) FROM t` — labelled approximate with the sketch's stated error |
| `distinct_breakdown` | "distinct users by region" | the same, grouped |
| `time_slice` | "queries run in 2026", "queries run in July 2026" | `… WHERE date >= lo AND date < hi` |
| `trend` | "queries run over time", "errors by month" | `… GROUP BY date ORDER BY date`, at the grain the time dimension actually has |

**Derived classes** — composed in Python from two or three base results, so a
comparison is two windows of one statement rather than a second SQL dialect
to maintain. Each part keeps its own evidence, and the composed answer
carries them under `evidence.composed_of`.

| Class | Example | Built from |
|---|---|---|
| `comparison_period` | "queries run vs last month" | two `time_slice` statements; reports the level, the change and the percent change |
| `year_over_year` | "queries run year over year" | two `time_slice` statements a year apart |
| `share_of_total` | "what share of queries run is Europe", "percentage of queries run by region" | one slice (or one breakdown) and the grand total |
| `ratio` | "errors per query" | one statement per measure, divided |
| `top_n_within` | "top 3 countries by queries run in each region" | one two-dimension grouping, ranked inside each outer group |
| `existence` | "how many countries have more than 1000 users" | one breakdown, counted against the threshold, with the matches named |
| `vague_default` | "what is current usage", "how are we doing" | the spec's headline measure at the dataset's latest period — and the answer **says** which defaults it used |

**"Current" is the data's word, not the clock's.** A period a question does
not name comes from the spec's time dimension — the maximum date the
statistics record for the column — so a dataset that stops in August is
current as of August. A previous period the data does not reach is stated
("The data holds no 2025 to compare it with"), never answered as zero.

**Refusal classes** — when the DLM will not answer. Nothing here produces a
number.

| Class | When | What the user gets |
|---|---|---|
| `clarify_value` | a word resolves to no indexed value, or to a value in two columns | the nearest indexed values as options, plus an explicit "leave it out" |
| `clarify_metric` | two measures read the question equally well, or a word is close to a measure name | the measures as options |
| `clarify_dimension` | two dimensions read the breakdown equally well, or the "by" phrase names a column that is not a dimension | the dimensions as options |
| `unanswerable` | a word is close to nothing the dataset holds | why, plus the three closest questions the spec *can* answer, plus a few values it knows |
| `out_of_scope` | the question is about nothing the platform holds | the datasets that exist |

A clarification is resumed by re-posting the original question with the slot
pinned (`choices`); the DLM never picks for the user, and never answers a
question the user did not ask by dropping the word it could not place.

---

## Answer-from-Context

When the DLM resolves a question to a shape that was precomputed at generation time, it serves the answer from an **in-memory dict** — zero database trip, microsecond latency. This is the `_serve_from_context()` path.

### What is precomputed

**Grand totals** — every metric's aggregate across the full dataset (one scan for all metrics at generation time).

**Single-dimension breakdowns** — every metric grouped by each precompute-enabled dimension, ordered by the first metric descending, limited to the curated depth (default 500 rows). One scan per dimension at generation time.

**2-dim combos** — for every pair of low-cardinality dimensions whose cross-product is under the cell cap (5,000 cells, max 12 pairs), all metrics grouped by `(dim1, dim2)`. This means questions like "consumption in Asia for Enterprise" serve from context.

### Serving logic

| Question shape | Context lookup |
|---|---|
| "Total revenue" | Grand total for that metric |
| "Revenue by country" | Single-dim breakdown |
| "Revenue in Japan" | Row from the by-country breakdown |
| "Revenue in Japan by segment" | 2-dim combo, filter on country, return segment breakdown |
| "Revenue in Japan, Enterprise" | 2-dim combo cell lookup |
| "Revenue in Japan, Enterprise by product" | Not covered — falls to live query |

Top-N slicing ("top 10 countries") is applied after retrieval by truncating the breakdown rows.

---

## HLL Sketch Cuboids

For **non-additive COUNT(DISTINCT)** metrics (e.g. "Unique Users"), exact precomputation of every filter combination is impractical. Instead, the DLM builds a **HyperLogLog sketch cuboid** at generation time.

### How it works

1. At generation, one SQL scan hashes every row's distinct-column value using `hashtextextended`, extracts the register index (top P bits) and rho (leading-zero run), and `MAX(rho)` per `(cell, register)` — pure SQL, no Postgres `hll` extension required
2. The resulting register vectors are stored per cell in `dlm_sketch` as sparse JSON
3. At query time, `_serve_sketch()` filters the matching cells and **unions the register vectors in Python** using `api/dlm/hll.py`
4. The configured sketch has approximately 2.3% theoretical relative standard error and requires **no live scan** when the requested slice is covered

### Cuboid dimensions

The cuboid's axes are the low-cardinality precomputed dimensions (< 500 distinct values), greedily selected smallest-first until the cell product hits 8,000 or 6 dimensions. This covers any sub-combo of those dims — including 3+ filter subsets that the exact 2-dim combos above don't materialize.

Answers from sketches identify the result as an estimate and state when it was served without a source scan.

---

## Over Engine tables

A dataset bound to a table of the Engine's durable catalog — `source: {kind:
"engine", table_id}` on the dataset, or a dataset over a registered native
catalog whose table id resolves — is answered on the Engine with the same
guarantees as a warehouse dataset, and the answer-from-context above is
replaced by the Engine's own knowing path
([The learning engine](../engine/learning-engine.md), [Declared shape and the
cube](../engine/storage-and-catalogs.md#declared-shape-and-the-cube)).

**Binding.** The dataset's catalog, schema and table names and its column
list come from `GET /v1/catalog/tables/{id}`; when the table declares a
shape (`ALTER TABLE … SET SHAPE (dimensions = …, measures = …, time = …)`),
the shape is the DLM's semantics: every declared dimension is a breakdown
dimension, every measure a metric under its aggregates — `sum`, `count`,
`min`, `max` additive; `count_distinct` non-additive and marked
`approximate` in the context spec — and the time column is the date column.
`COUNT(*)` is always a metric (`Rows`). Without a shape the column types
decide (text and boolean columns are dimensions, numeric non-identifier
columns are summed, the first date or timestamp column is the date column)
and questions take the Engine's row path until a shape is declared. The
context editor curates the spec as for any dataset — aliases, hidden
elements, the default metric — plus `approximate` per metric and the
dataset's `freshness_policy` (`cached`, the default, or `live`).

**Generation** reads the table definition and `GET
/v1/catalog/tables/{id}/version`, indexes each declared dimension's values
with one cube-shaped statement (`SELECT dim, COUNT(*) … GROUP BY dim`,
answered from the cube's cells without a scan when the cube is built), and
records the table, its shape and the source version in the artifact. **No
warehouse cell is written**: `dlm_answers` and `dlm_sketch` hold nothing for
an Engine-backed dataset. On the local stack this turns a thirty-second
generation into under a second.

**Answering.** The question resolves to its slots exactly as before — the
router is shared — and the statement is written in the Engine's dialect
(`api/dlm/engine_dialect.py`: bare relation names, a quoted column only
when it is not a plain identifier or is a reserved word, `DATE` literals
for date columns, `EXTRACT` for timestamps, resolved bounds instead of
`CURRENT_DATE` arithmetic). A breakdown over declared dimensions is written
**without `ORDER BY`/`LIMIT`** — the shape the cube answers, bounded by the
dimensions' caps — and the DLM ranks and slices the cells itself, so "top 3
countries by users" is a context answer. The DLM executes the statement
through the Engine bridge with the settings it chooses:

| Setting | Value | Why |
|---|---|---|
| `result_cache` | `true` under `freshness_policy: cached`, `false` under `live` | The Engine's cache is keyed by the catalog snapshot and cleared on every publish; a dataset that must read every time says so in its spec |
| `approximate` | `true` only when the question's metric is marked `approximate` (a `count_distinct` measure, by default) | The cube holds distinct counts as sketches; the Engine states the error on the record; an exact `COUNT(DISTINCT)` is never taken from the cube |
| `use_statistics` | `true` | The knowing path is the point; the `reproduce` block turns it off |

The lane and the label come from the record's `execution.mode`, never from
the DLM's own scoring: `context` — the cube (`detail: cube at <version>`) or
the statistics (`statistics at <version>`) answered without reading the
rows — is badged **From context · no scan**; `cache` (the result cache) is
**From cache**; `distributed`/`coordinator` is **Live query · Xs**. An
estimate is labelled from `execution.approximate` with the sketch and error
the Engine states. The Engine only answers from a cube or statistics that are
current for the statement's pinned source version, so a context answer over
an Engine table is exact at the version its evidence names.

**Freshness.** The scorer's change signal is the table's source version from
`GET /v1/catalog/tables/{id}/version` (the Delta log's tail, the Iceberg
pointer, a listing digest or a file's identity — a metadata read): a version
equal to the one the artifact recorded is no change; a moved version is a
change of at least the half fraction, the same per-element rule a re-analyzed
warehouse table takes, and the sweep rebuilds the value index. The
PostgreSQL counter path is unchanged for warehouse datasets.

**Evidence and reproduction.** Every answer's `evidence` names the statement,
the dataset and its source (table id and catalog names), the source version
it reflects, the lane, the Engine's `execution` object verbatim, the elapsed
time and rows, and a `reproduce` block — the same statement with
`{use_statistics: false, result_cache: false}` — that `POST /api/v1/dlm/reproduce`
runs so the live number sits beside the context one
([API reference](../reference/api.md#dlm-answers-and-their-evidence)). Studio's
answer card shows this under **Evidence**, with **Run live** for Analysts and
above.

**Coverage.** `python -m dlm.coverage --dataset <id>` (in `api/`) builds a
question corpus from the dataset's own spec — totals, breakdowns, filters,
filter-and-breakdown, two filters, rankings, non-additive totals and
breakdowns, years and trends when there is a date column, an unknown value
and an out-of-scope question — asks each through `/dlm/ask`, and prints per
class how many were answered, clarified or refused and, of the answered, how
many came from context, the cache or a live read, every count taken from the
evidence. The run on the local stack's dataset is recorded in
[coverage-2026-09-19](../qualification/dlm/coverage-2026-09-19.md); the
contract corpus for the AKS telemetry dataset stays with
`scripts/qualify-dlm-questions.py`.

---

## `serve_chart()` and `serve_chart_multi()`

Dashboard charts use a dedicated serving path that maps a `(metric_column, aggregation, group_by, filters)` tuple to precomputed context — no chat-style NL parsing needed.

**`serve_chart()`** resolves the metric column + aggregation to a named metric (e.g. `SUM(primary_energy_consumption)` → "Total Energy"), validates filters (equality only) and group-by against known dimensions, then delegates to `_serve_from_context()`. Falls back to HLL sketches for COUNT(DISTINCT) metrics.

**`serve_chart_multi()`** handles multi-metric charts (stacked bar, combo). It resolves each metric independently, retrieves each from context, and merges the rows by group key. Returns `served=true` only when ALL metrics are answered from context.

Both return a freshness score alongside the data so the dashboard can show staleness indicators.

**API endpoint:** `POST /api/v1/dlm/serve-chart` — accepts single or multi-metric payloads.

---

## `filter_values()`

Dashboard filter dropdowns call `GET /api/v1/dlm/filter-values?dataset_id=N&column=X` to populate their options. This reads the distinct values from precomputed breakdown rows — no live SQL.

The values are extracted from `dlm_answers`: for any metric that has a single-dimension breakdown on the requested column, the group keys are the dimension's distinct values. Sorted alphabetically, capped at the requested limit (default 200).

---

## Freshness Scoring and Auto-Rebuild

`check_freshness()` computes how current a dataset's DLM context is by combining two signals:

1. **Time decay** — exponential decay from the artifact's `built_at` timestamp
2. **Data-change signal** — row modifications since the last ANALYZE, read from `pg_stat_user_tables`; for an Engine-backed dataset, the table's source version from `GET /v1/catalog/tables/{id}/version` compared with the version the artifact recorded ([Over Engine tables](#over-engine-tables))

The product yields a score in [0, 1]:

| Score range | Recommendation |
|---|---|
| >= 0.7 | `use_context` — answers are current |
| >= 0.5 | `rebuild` — context is usable but aging |
| < 0.5 | `no_context` — too stale to trust |

When the ask/serve path hits a stale dataset, `maybe_auto_rebuild()` spawns a background thread to regenerate the DLM. A 5-minute cooldown per dataset prevents rebuild storms.

---

## Context Hints

When a question falls through to a live query (e.g. a 2-filter combo not in the precomputed set), the DLM still surfaces **context hints** — partial answers it already knows — so the user sees something instantly while the exact figure is being fetched.

`_context_hints()` returns:

- The metric's **grand total** (from the precomputed total)
- The metric's value **for each single-dimension filter** (from the per-dim breakdown)

The frontend shows these as inline context while a live-query timer ticks. When the live result arrives, it replaces the hints.

---

## Follow-Up Detection

The frontend detects conversational follow-ups and rewrites them using the previous query's context before sending to the DLM.

**Trigger patterns:** "What about Japan", "How about India", "And France", "Same for Germany", or a short 1-3 word entity-only message (e.g. just "Brazil").

**Rewrite logic:** the previous query is cleaned (existing entity names removed), and the new entity is injected. Example:

```
Previous: "Total energy consumption in China"
Follow-up: "What about Japan"
Rewritten: "Total energy consumption in Japan"
```

This rewritten question is then sent to the DLM as a fresh `ask()` call.

---

## Template Parser (Fallback)

The in-browser template parser (`nlToSql.ts`) handles query shapes the DLM does not yet cover. It runs entirely client-side, generates SQL from keyword patterns, and is tried only after the DLM returns no result.

### Dataset auto-detection

Each loaded schema is scored against the query: +3 for dataset name words, +2 for metric names, +1 for column names. Highest score wins.

### Pattern matching (priority order)

| Pattern | Triggers on | Example | Chart |
|---|---|---|---|
| Aggregate only | `total`, `sum`, `count`, `average`, `min`, `max` | "total revenue" | `kpi` |
| Top N | `top <N> <group> by <metric>` | "top 10 countries by deaths" | `bar` |
| Trend | `over time`, `trend`, `by month/year/week/day` | "revenue over time" | `line` |
| Grouped | `by`, `per`, `for each` | "revenue by region" | `bar`/`line` |
| Compare | `compare X vs Y` | "compare North vs South" | `line`/`bar` |
| Distribution | `distribution`, `breakdown`, `spread` | "breakdown of regions" | `pie`/`bar` |
| Fallback scan | (no pattern matched) | best-effort from tokens | varies |

### Fuzzy column matching

`findColumn()` resolves tokens to schema columns via: direct name match → description match → forward alias → reverse alias. Built-in aliases cover common synonyms (revenue/sales, quantity/qty, etc.).

---

## Code Locations

| File | Role |
|------|------|
| `api/dlm/engine.py` | DLM runtime: compilation, deterministic resolution, and answer serving; the Engine-backed path (`_answer_on_engine`), evidence and `reproduce` |
| `api/dlm/engine_dialect.py` | One statement assembler, two dialects (PostgreSQL, Engine) |
| `api/dlm/classes.py` | The named question classes: the registry, the derived-class detection, and the composition from base results |
| `api/dlm/curation.py` | Auto-curation: a dataset's context spec derived from the Engine's statistics and its declared shape, with per-element evidence |
| `api/dlm/coverage.py` | `python -m dlm.coverage`: question-class coverage of one or more datasets, with `--check` for wrong answers against an independent statement |
| `api/services/engine_datasets.py` | Binding a dataset to an Engine table: names, columns and semantics from the definition and its shape |
| `api/routers/dlm.py` | API endpoints: /dlm/ask, /dlm/serve-chart, /dlm/filter-values, /dlm/route, /datasets/{id}/dlm/generate, /datasets/{id}/freshness |
| `api/dlm/hll.py` | HyperLogLog implementation |
| `api/dlm/profiler.py` | Statistics substrate and context profiling |
| `api/dlm/validity.py` | Time/change decay factors used by freshness scoring |
| `api/dlm/router.py` | Question-to-context route selection |
| `studio/app/page.tsx` | Frontend chat flow: three-tier execution (DLM → ACR → template parser), follow-up detection, context hints display |
| `studio/utils/nlToSql.ts` | In-browser template parser: patterns, fuzzy matching, SQL builder |
| `studio/components/chat/InlineChart.tsx` | Chat-embedded chart renderer (ECharts) |
| `studio/components/chat/EvidencePanel.tsx` | The answer card's Evidence disclosure: statement, source and version, lane, Run live |
| `studio/components/ContextBanner.tsx` | Homepage banner showing compiled context coverage per dataset |
