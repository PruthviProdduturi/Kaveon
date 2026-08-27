# NL→SQL: How Natural Language Queries Work

Kaveon's homepage lets you ask questions in plain English and get back charts, with **no LLM dependency**. Three engines sit behind it, tried in order:

1. **DLM (Data Language Model) — the primary path.** A per-dataset compiled context artifact in the API. It resolves the question deterministically and, for common cases, **answers from precomputed context with no database scan at all** — returning a result badged **"From context · no DB scan"**. Only novel slices fall through to a single live query, badged **"Live query · Xs"**.
2. **ACR (Adaptive Context Routing) — the middle tier.** When the DLM can't answer (e.g. the shape doesn't match), the in-browser template parser generates SQL and ACR decides whether to serve the answer from cached context or run the query live.
3. **Template parser — the fallback.** A keyword-based parser (`apps/kaveon-web/utils/nlToSql.ts`) that runs entirely in the browser. It handles shapes neither the DLM nor ACR can build (mainly time-series trends, comparisons, distributions).

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
3. At query time, `_serve_sketch()` filters the matching cells and **unions the register vectors in Python** using the `services/hll` module
4. The estimate has ~1-2% error and requires **no live scan**

### Cuboid dimensions

The cuboid's axes are the low-cardinality precomputed dimensions (< 500 distinct values), greedily selected smallest-first until the cell product hits 8,000 or 6 dimensions. This covers any sub-combo of those dims — including 3+ filter subsets that the exact 2-dim combos above don't materialize.

Answers from sketches are badged with the note: "Estimated from a HyperLogLog sketch (~1-2% error) · no DB scan".

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
2. **Data-change signal** — row modifications since the last ANALYZE, read from `pg_stat_user_tables`

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
| `apps/kaveon-api/services/dlm.py` | DLM engine: generate, ask, serve_chart, serve_chart_multi, filter_values, check_freshness, route, resolve_value, HLL sketches |
| `apps/kaveon-api/routers/dlm.py` | API endpoints: /dlm/ask, /dlm/serve-chart, /dlm/filter-values, /dlm/route, /datasets/{id}/dlm/generate, /datasets/{id}/freshness |
| `apps/kaveon-api/services/hll.py` | HyperLogLog implementation: empty, union_estimate, to_sparse |
| `apps/kaveon-api/services/context_profiler.py` | Zero-scan statistics substrate (pg_stats snapshots) |
| `apps/kaveon-api/services/context_validity.py` | Time/change decay factors used by freshness scoring |
| `apps/kaveon-web/app/page.tsx` | Frontend chat flow: three-tier execution (DLM → ACR → template parser), follow-up detection, context hints display |
| `apps/kaveon-web/utils/nlToSql.ts` | In-browser template parser: patterns, fuzzy matching, SQL builder |
| `apps/kaveon-web/components/chat/InlineChart.tsx` | Chat-embedded chart renderer (ECharts) |
| `apps/kaveon-web/components/ContextBanner.tsx` | Homepage banner showing compiled context coverage per dataset |
