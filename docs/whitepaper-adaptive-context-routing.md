# Adaptive Context-Based Query Routing Using Data Staleness Scoring

**Pruthvi Prodduturi**
August 2026

---

## Abstract

Conversational analytics systems answer natural-language questions by generating
and executing a database query for every question. This is correct but wasteful:
a large fraction of questions are repeats, near-repeats, or answerable from
statistics the database already maintains about itself. Caching helps, but the
industry-standard cache is a flat time-to-live (TTL) — it expires answers on a
clock that has no relationship to whether the underlying data actually changed.
A TTL of five minutes re-queries unchanged data twelve times an hour and serves
stale data the instant the table churns at second one.

This paper presents **adaptive context-based query routing**: a method that
builds a statistical *context representation* of a dataset without a large
language model, assigns each element of that context a **validity score** derived
from how much the underlying data has actually changed, and routes each incoming
question to either an instant context-based answer or a live query — deciding
**per context element relevant to the question, not per dataset**. The load-bearing
idea is that data change can be *measured from the database's own metadata
counters* rather than by re-querying the data, so staleness is detected at
effectively zero cost. On the Kaveon platform the router is a drop-in replacement
for the TTL result cache; it answers a meaningful class of questions with no
query execution at all, and it re-queries exactly when — and only the parts that
— the data has moved.

---

## 1. Introduction

Kaveon's conversational layer turns natural-language questions into charts. The
companion paper *Template-Based Natural Language to SQL* describes how a question
becomes SQL deterministically, with no LLM. That work removed the cost, latency,
and non-determinism of the *translation* step. This paper addresses the step
after it: **execution**.

Every question, once translated, hits the database. For a dashboard that reloads,
a team asking the same "how are we doing this quarter" question forty times a day,
or an exploration session that revisits the same slice repeatedly, the system
executes the same query against the same unchanged data over and over. The
standard mitigation is a result cache with a TTL. TTLs are a blunt instrument:

- **Too short** and you re-query unchanged data constantly — you pay for freshness
  you already had.
- **Too long** and you serve stale answers after the data has changed — you trade
  correctness for cost.
- **Either way**, the expiry clock is disconnected from the one thing that matters:
  *did the data change?*

The insight behind this work is that a modern analytical database already knows
how much its tables have changed, and already maintains a statistical summary of
its columns — and both are readable in constant time without scanning a single
row. PostgreSQL's `pg_stat_user_tables` exposes, per table, the number of row
modifications since the last statistics refresh (`n_mod_since_analyze`), the live
row count (`n_live_tup`), and when statistics were last computed (`last_analyze`).
PostgreSQL's `pg_stats` exposes, per column, the distinct-value estimate, null
fraction, most-common values, and a distribution histogram. These are byproducts
of the query planner that the database keeps current for its own purposes.

If we treat those catalogs as a **context representation** of the data, and treat
their change counters as a **freshness signal**, we can do something a TTL cache
cannot: decide whether a cached answer is still valid based on *how much the
relevant data actually moved*, and re-query only when it did — and only the parts
that did.

---

## 2. Method Overview

The method has six steps. Steps 1–2 run periodically (or on demand) to build and
maintain the context. Steps 3–6 run per question.

```
   ┌─────────────────────────────  build / maintain  ──────────────────────────┐
   │  1. Profile the dataset (no LLM):                                          │
   │       column distributions  <- pg_stats                                    │
   │       table relationships   <- pg_constraint + name/type heuristics        │
   │       query pattern freq.    <- query_history                              │
   │  2. Capture change counters (n_live_tup, n_mod_since_analyze, last_analyze)│
   │       per element and store the snapshot.                                  │
   └────────────────────────────────────────────────────────────────────────────┘
                                        │
   ┌─────────────────────────────  per question  ──────────────────────────────┐
   │  3. Map the question -> the specific context ELEMENTS it depends on        │
   │       (deterministic token/alias matching, no LLM).                        │
   │  4. Score ONLY those elements for validity:                                │
   │       score = f(time-decay, change-magnitude, usage-weighted half-life)    │
   │     Route:  all valid -> CONTEXT   some stale -> HYBRID   all stale -> QUERY│
   │  5. If a live query runs: execute, cache the result against the question    │
   │     signature + its element dependencies, and REFRESH those elements.      │
   │  6. If answering from context: return with NO query execution — from the    │
   │     profile directly, or from a dependency-valid cached result.            │
   └────────────────────────────────────────────────────────────────────────────┘
```

The architecture is deliberately parallel to the NL-to-SQL engine's philosophy:
**measure, don't guess.** The router does not predict whether data changed; it
reads a counter that says so.

---

## 3. The Context Representation (Step 1 — no LLM)

A *context element* is the unit of granularity: either a **table** or a single
**column**. The context representation is a set of elements, each with a profile.
It is built entirely from the database's own catalogs — no model, no data scan.

### 3.1 Column value distributions — `pg_stats`

For every column the profiler reads:

| `pg_stats` field | Meaning | Used for |
|---|---|---|
| `n_distinct` | distinct-value estimate (absolute if ≥0, negative ratio of rows if <0) | approximate `COUNT(DISTINCT)` answers |
| `null_frac` | fraction of NULLs | completeness / null-rate answers |
| `most_common_vals` / `most_common_freqs` | top values and their frequencies | "most common X", categorical share |
| `histogram_bounds` | value distribution buckets | range / percentile shape |
| `avg_width` | average byte width | row-size estimation |

These are maintained by `ANALYZE` and autovacuum. Reading them is O(1) — profiling
a 100-million-row table costs the same as profiling a 100-row one. This is the
sense in which the context is built "by statistically profiling column value
distributions" without ever touching the data.

### 3.2 Inferred relationships — `pg_constraint` + heuristics

Declared foreign keys come from `information_schema`. On top of that, a
name/type heuristic links a `<t>_id` column to a `<t>` primary key even when no
FK is declared — common in analytical warehouses where referential constraints
are dropped for load performance. The relationship graph is what lets the router
understand that a question spanning two tables depends on both.

### 3.3 Query pattern frequency — `query_history`

The platform already records every executed query and the tables it touched
(`query_history.tables_used`). Aggregating this gives a per-table **usage weight**:
how often each element has actually been relied upon for prior answers. This feeds
factor (c) of the validity score (§4.3).

---

## 4. The Validity Score (Step 2 & 4)

Each element carries a **validity score** in `[0, 1]`. `1.0` means "trust the
context, no query needed"; `0.0` means "the context is worthless, go live." The
score is a composite of three factors, all computed from cheap metadata.

### 4.1 Factor (a) — Time

Elapsed time since the element was last refreshed, run through an exponential
half-life decay:

```
time_factor = exp( -ln(2) · age / half_life )
```

Age alone is a weak signal — old data that never changes is perfectly valid — so
time is combined multiplicatively with change (§4.4) rather than trusted on its
own. Its role is to bound staleness even when the change signal is unavailable.

### 4.2 Factor (b) — Change magnitude (the load-bearing factor)

This is the factor that distinguishes the method from a TTL cache, and the one a
naïve reading gets wrong. To know whether a column's statistics changed, the
obvious approach is to re-compute them — but that requires querying the data,
which defeats the entire purpose. **The method never does this.** Instead it reads
the database's own change counter:

```
delta      = n_mod_since_analyze(now) - n_mod_since_analyze(at_capture)
change_frac = delta / row_count_at_capture
change_factor = exp( -ln(2) · change_frac / CHANGE_HALF_FRACTION )
```

`n_mod_since_analyze` is a running count of inserts/updates/deletes the database
maintains for its own autovacuum scheduling. Reading it is a metadata lookup. A
shift in `last_analyze` (autovacuum re-analyzed the table, resetting the counter)
is treated as an independent change signal. With `CHANGE_HALF_FRACTION = 5%`, a
table that has churned 5% of its rows since capture has its change factor fall to
0.5; a table that hasn't moved stays at 1.0.

This is the enablement core: **staleness is measured from a counter, not from a
scan.** Any database that exposes a table-modification counter (PostgreSQL
`n_mod_since_analyze`, SQL Server `rowmodctr`, a CDC log sequence number, or a
`max(updated_at)` watermark) can supply factor (b).

### 4.3 Factor (c) — Usage-weighted decay

The claim requires a decay "weighted by how frequently each portion of the context
has been relied upon." The direction of that weighting is a design decision the
method must pin down, because both directions are defensible-sounding and only one
is right. This method makes **frequently-used elements decay _faster_**:

```
effective_half_life = base_half_life / (1 + gain · ln(1 + usage_count))
                      (floored at a minimum so hot elements can't demand a
                       refresh on literally every request)
```

The rationale: the answers users lean on most are the ones where a stale answer
does the most damage, so those elements should be held to the highest freshness
bar. A cold element that nobody queries can tolerate a longer half-life; a hot one
cannot. Usage shortens the half-life of factor (a); it does not touch factor (b).

### 4.4 Composite

```
validity = time_factor(age, effective_half_life(usage)) · change_factor(...)
```

Multiplicative, so either signal collapsing to zero invalidates the element — a
table that changed 40% is stale no matter how recently it was profiled, and a
table profiled a week ago is suspect no matter how little the counter moved. The
score is returned with a full per-factor breakdown for observability.

**Worked example** (from the implementation's own tests):

| Element state | time | change | validity |
|---|---|---|---|
| fresh, unused, no change | 1.00 | 1.00 | **1.00** |
| just profiled, 10% of rows churned | 1.00 | 0.25 | **0.25** |
| 2h old, usage 0 | 0.79 | 1.00 | **0.79** |
| 2h old, usage 500 (hot → shorter half-life) | 0.48 | 1.00 | **0.48** |

The last two rows show factor (c): identical age and change, but the hot element
scores lower because heavy reliance shortened its half-life.

---

## 5. Question → Element Mapping (Step 3 — no LLM)

Routing per-element requires knowing *which* elements a question depends on. This
is done deterministically, reusing the philosophy of the NL-to-SQL fuzzy resolver:

1. Tokenize the question; drop stopwords and sub-3-character tokens.
2. Expand tokens with stems and synonyms (`_expand_tokens`) — "carbon" expands to
   include "emission", "selling" stems to "sell" which expands to "revenue", etc.
3. Match expanded tokens against every element's name (table or column), by word
   overlap and substring containment (so "cases" matches `new_cases`).
4. Include a **table** element whenever its name matches or any of its columns
   match; include a **column** element whenever its name matches.

For "deaths by country" against a COVID dataset, the mapping returns exactly
`{covid_daily, covid_daily.country, covid_daily.deaths}` — and crucially does
**not** pull in an unrelated `nyc_taxi` table. Only those three elements are
scored for validity; the freshness of the other 25 tables in the database is
irrelevant to this question. This is what "using the validity score of the
specific context elements relevant to the question (not the dataset as a whole)"
means in practice.

---

## 6. Routing and Answering (Steps 4–6)

Given the validity scores of the relevant elements and a threshold (default 0.7):

| Condition | Route | Behavior |
|---|---|---|
| all relevant elements ≥ threshold | **context** | answer with no query (§6.1) |
| some below threshold | **hybrid** | refresh/query only the stale elements |
| all below threshold (or none captured) | **query** | full live query |

### 6.1 Answering from context with no query at all

Two context paths return an answer without executing any query:

**Profile-synthesised answers.** A class of questions is answerable from the
profile directly. "How many distinct countries?" → `pg_stats.n_distinct` (195).
"Null rate of deaths?" → `pg_stats.null_frac` (0.12). "How many rows?" →
`n_live_tup` (50,000). These never touch the data — the answer *is* the statistic.
Each is returned with an explanation naming its source and, where the statistic is
an estimate (`n_distinct`), an explicit approximate flag — the method's answer to
returning confidence alongside context-derived results.

**Dependency-valid cached results.** For questions that were previously answered
by a live query, the result is cached against the question signature together with
the list of element dependencies it relied on. The cache entry is served **only
while every dependency's validity score is still above the threshold** — the exact
point of departure from a TTL cache. When any dependency drifts, the entry is not
served; the router falls through to a live query.

### 6.2 The live path and the feedback loop (Step 5)

When a query runs, the router (a) executes it, (b) caches the result with its
dependencies, and (c) **refreshes the change counters of the touched elements** to
a fresh state. Step (c) closes the loop: the very act of going live because the
context was judged stale re-captures the counters, so the freshly-answered
question is now served from context until the data moves again. Staleness triggers
a refresh, and the refresh restores validity — the system self-heals around change.

---

## 7. Comparison with Prior Approaches

| Dimension | TTL result cache | Materialized view + incremental maintenance | LLM RAG over a vector store | **Adaptive context routing** |
|---|---|---|---|---|
| Invalidation signal | wall clock | view-defined, engine-driven | embedding similarity / manual | **measured data change per element** |
| Granularity | whole cached entry | whole view | document chunk | **per table/column element** |
| Detects "data unchanged" | no | yes (but heavyweight) | no | **yes, from a counter** |
| Re-computes only what moved | no | partially | n/a | **yes (hybrid route)** |
| Answers with no query | yes (until TTL) | reads the view | LLM call | **yes (profile or valid cache)** |
| Cost to check freshness | free but blind | maintenance overhead | embedding + search | **one metadata read** |
| Uses an LLM to build context | no | no | yes | **no** |

The method's novelty is not any single ingredient — result caches, view staleness,
and statistics catalogs all predate it — but their **combination**: a per-element
validity score whose change term is measured from a metadata counter, used to route
a natural-language question between a no-query context answer and a live query, with
the context itself built without an LLM. A TTL cache expires on a clock; a
materialized view re-maintains an entire view definition; this method invalidates
exactly the elements a specific question depends on, exactly when their data moves.

---

## 8. Implementation

The engine is three services plus a router, in the Kaveon API:

| File | Responsibility |
|---|---|
| `services/context_profiler.py` | Build/refresh the context representation from `pg_stats`, `pg_constraint`, `query_history`; capture change counters; persist snapshots. |
| `services/context_validity.py` | The three-factor validity score, per-element scoring, and the route decision. |
| `services/context_router.py` | Question→element mapping, profile-synthesised answers, dependency-valid result cache, live path + refresh. |
| `routers/context.py` | `POST /context/build`, `GET /context/validity`, `POST /context/ask`. |

Two metadata tables back it: `context_snapshots` (one row per element, holding the
profile and the captured change counters) and `context_answer_cache` (results keyed
by question signature with their element dependencies). Both self-migrate on first
use. Statistics are read from the warehouse connection pool; snapshots are stored
in the platform metadata store. The context path contains no LLM and, for
profile-synthesised answers, no data-plane query whatsoever.

### API sketch

```
POST /api/v1/context/build     { database, schema_name }         -> profile summary
GET  /api/v1/context/validity  ?database=...                     -> per-element scores
POST /api/v1/context/ask       { question, database, sql? }      -> { route, source,
                                                                       validity, answer|result }
```

`ask` returns the route it took (`context` / `hybrid` / `query`), the per-element
validity that drove the decision, and either a profile/cache answer (no query) or a
live result (query executed, elements refreshed).

---

## 9. Limitations and Future Work

**Change-counter fidelity.** `n_mod_since_analyze` counts modifications, not net
change, and resets on `ANALYZE`; the method compensates by treating an `ANALYZE`
as a change event, but a table under constant autovacuum needs a watermark
(`max(updated_at)` or a CDC LSN) for a tighter signal. The factor-(b) interface is
deliberately source-agnostic to allow this.

**Cross-engine support.** The current profiler is PostgreSQL-specific. SQL Server
(`sys.dm_db_stats_properties`, `rowmodctr`), MySQL (`information_schema.STATISTICS`,
`innodb_metrics`), and DuckDB expose analogous catalogs; the method transfers, the
catalog names do not.

**Profile-synthesised answer coverage.** *(Largely shipped — see the Addendum.)*
The profile directly answers row counts, approximate distinct counts, and null
rates. The DLM productization goes further: at build time it **precomputes every
metric's grand total and each per-dimension breakdown** into a dedicated answer
store, so "total X", "X by dimension", and single-dimension filters are served from
context with no scan — not just the statistic-derived answers.

**Threshold policy.** A single global threshold (0.7) drives the route today. A
cost/accuracy policy — cheaper to serve a slightly-stale answer for an exploratory
question than for a board report — would make the threshold context-dependent.

**Hybrid decomposition.** *(Shipped.)* The hybrid route re-runs the query when any
dependency is stale. The DLM answer store now realizes **full two-dimensional
decomposition**: equality and IN filters across any pair of dimensions are served
from precomputed 2-dim answers — context-only, no scan. Dashboard-level curation
(`curate_dashboard`) automatically identifies the filter × groupby matrix from a
dashboard's charts and precomputes all required N-dim combos. Only time-window
slices and 3+-dimension intersections still fall to one live query.

**Incremental refresh.** *(Shipped.)* `incremental_refresh()` performs a delta-merge
for additive metrics (SUM, COUNT) without recomputing from scratch — it reads new
rows since `last_refreshed`, computes the delta, and merges it into the existing
answer. Non-additive metrics (AVG, COUNT DISTINCT) trigger a full recompute of the
affected answers only.

**Auto-rebuild on staleness.** *(Shipped.)* `maybe_auto_rebuild()` checks whether
the DLM's staleness score has drifted below the threshold and, if so, triggers a
rebuild automatically on the next request — the system self-heals without operator
intervention.

**Curatable context spec.** *(Shipped.)* The DLM auto-suggests a context spec
(dimensions, metrics, aliases) from the schema, but operators can overlay **human
curation** — custom display names, value aliases, hidden dimensions, default metric
selection — stored in a separate `curation` column on `dlm_artifact`. The effective
spec merges both (`_merge_spec`), giving the auto-generated profile a human editorial
layer without losing the ability to regenerate the base.

**Robust intent resolution.** *(Shipped.)* The token-based matchers that map a
question to a metric, group-by dimension, and filter value now use **symmetric
stem+synonym expansion**. A 56-concept seed synonym lexicon (finance, health, energy,
transport, geography, time, tech) provides cross-domain vocabulary coverage. A
lightweight suffix stemmer handles common English inflections ("selling" → "sell",
"exported" → "export", "hospitalization" → "hospital") with no external NLP
dependencies. `_expand_tokens` stems every token, looks up its synonym group (falling
back to the stemmed form), and produces the expanded set — applied symmetrically to
both the question tokens and the metric/dimension candidate tokens. This closes the
gap between how users phrase questions ("what are the carbon outputs?") and how
metrics are stored (`total_emissions`), without adding a model dependency. The
`route()` dataset selector also benefits from the enhanced stemmer, as both stemmer
definitions share the same runtime function.

---

## 10. Conclusion

"Talk to your data" spends most of its cost not on understanding the question but
on re-executing it against data that often hasn't changed. Adaptive context-based
query routing replaces the clock-based cache with a measurement-based one: it
profiles the data from the database's own statistics without a model, scores each
element's validity from a change counter rather than a re-query, and routes each
question — element by element — to an instant answer or a live query. The result is
a system that answers from context by default and reaches for the database only
when, and only where, the data has actually moved.

The companion NL-to-SQL engine made the *translation* free. This makes the
*execution* conditional. Together they close the loop: a conversational analytics
system that is fast because it doesn't translate with a model, and cheap because it
doesn't query when it doesn't have to.

---

## Addendum — Productization: the DLM (Data Language Model)

The method above is the research core. In production it ships as the **DLM**: a
per-dataset compiled context artifact that is now the **primary** query path on the
Kaveon homepage (the template translator of the companion paper became the
fallback). Three additions turned the method into a product, all live in prod today.

### A.1 Precomputed answer store (compute-once, answer-many)

Rather than only synthesising answers from `pg_stats`, the DLM **precomputes** the
answerable shapes at build time. `generate_dlm()` runs one scan for all metric grand
totals and one scan per dimension for its breakdown, and persists them to a
`dlm_answers` table (metric × group-column → columns/rows). At query time,
`ask()` serves totals, per-dimension breakdowns, **and** single-dimension equality
filters straight from an **in-memory answer cache** (`_ANSWER_CACHE`, warmed once per
dataset) — a microsecond dict hit with **no fact-table scan**. Only year-slices and
multi-filter combinations fall through to one live query. Non-additive metrics
(`COUNT(DISTINCT …)`, `AVG`) are exact because each shape is computed independently
at build time — never derived by summing a breakdown.

**Measured:** over a 10.1M-row synthetic usage dataset, "what is current usage?"
(a `SUM` across all rows) drops from **~15 s live to ~1.5 s from context**, the
residual being routing/resolution rather than the eliminated scan.

### A.2 Physical separation of control/context from the warehouse

The method distinguishes a metadata plane (the context representation + counters +
caches) from the data plane (the rows). Production now separates them **physically**:
a small **control+context database** (`kaveonmeta` — holds `dlm_*`, `context_*`, and
the platform tables) is distinct from the **data warehouse** (`kaveon` — the actual
rows), on the same server but reached over independent pools. This makes the
economic claim literal: context answers are served from a tiny, fast store that
never contends with a multi-million-row scan, so the common case is answered from a
"$20/month box" while the warehouse scales on its own.

### A.3 Honest source labelling

Every answer is returned with the route it took and its timing, surfaced in the UI as
**"⚡ From context · no DB scan"** versus **"Live query · Xs"**. The system never
hides which plane served an answer or what it cost — the transparency the method's
per-factor validity breakdown was designed for, made visible to the end user.

### A.4 Context-powered dashboard charts (serve-chart)

The DLM's precomputed answers extend beyond the NL→SQL path to power **dashboard
charts directly**. `serve_chart()` (single metric) and `serve_chart_multi()` (stacked
bars, combo charts) resolve the chart's metric/group-by/filter specification against
precomputed answers and return `{columns, rows}` instantly — the warehouse is never
touched. The frontend tries DLM first (`POST /dlm/serve-chart`) and falls through to
SQL only when the context can't answer. On a B1ms instance (1 vCore, 2 GB), this
eliminates the 2-7 minute VIEW scans that caused 500 timeouts, replacing them with
~5 ms context lookups.

Multi-metric support merges per-metric answers by normalized group key, producing a
unified result only when ALL requested metrics can be served from context — no partial
results that would produce misleading charts.

**Filter support.** `serve_chart` accepts equality (`=`) and set-membership (`IN`)
filters. For single-value equality filters, the engine selects matching rows from the
precomputed group-by answer. For `IN` filters, it splits the comma-separated value
list and unions the matching rows. Only non-equality operators (`>`, `LIKE`, etc.)
fall through to SQL. This enables dashboard cross-filtering entirely from context:
selecting Region=Asia filters every chart to Asian countries with no warehouse scan.

**N-dimensional filter×groupby combos.** Dashboard charts often filter on one
dimension while grouping by another (e.g., "exports by country WHERE region = Asia").
The DLM precomputes **two-dimensional** answers (every pair of dimensions × every
metric) and stores them with a pipe-joined `group_col` key (e.g., `"country|region"`).
At serve time, the engine finds the answer whose dimensions cover both the group-by
and filter columns, then filters in memory — still no scan. For a dataset with 7
dimensions and 15 metrics, this produces 21 × 15 = 315 precomputed answers that
cover the full filter×group-by matrix.

**Measured coverage:** across 9 production dashboards (249 chart × filter
combinations), 245 (98.4%) are served entirely from DLM context with zero database
queries. The 4 misses are MIN/AVG metrics on a small dataset that intentionally falls
back to SQL.

### A.5 DLM filter values

Dashboard filter dropdowns also serve from context. `filter_values()` extracts
distinct dimension values from any precomputed GROUP BY answer for that dimension —
the same answer cache that powers `serve_chart`. This eliminates the
`SELECT DISTINCT` scan on the fact table, which is critical on resource-constrained
instances where a 10M-row distinct scan would timeout.

### A.6 HLL sketch cuboids

Non-additive `COUNT(DISTINCT …)` metrics cannot be derived by summing precomputed
breakdowns. The DLM addresses this with **HyperLogLog sketch cuboids**: at build time,
each fact-table row is hashed into per-cell sparse registers stored in `dlm_sketch`.
At query time, registers merge in-memory to produce approximate NDV at arbitrary
dimension slices — no fact-table scan. Typical relative error is ~2.3% at the default
precision (p=11, 2048 registers). This is the only approximate answer path; all
other DLM answers are exact.

### A.7 Where it lives

`services/dlm.py` (engine: value index, router, precompute, answer cache, serve-chart,
filter-values, HLL sketches, incremental refresh, dashboard curation, curatable
context spec, live assembler) and `routers/dlm.py`:

| Endpoint | Verb | Purpose |
|---|---|---|
| `/datasets/{id}/dlm/generate` | POST | Full DLM build (profile → precompute → answer store) |
| `/datasets/{id}/dlm` | GET | DLM status / manifest |
| `/datasets/{id}/dlm/context` | GET/PUT | Read / save human curation overrides |
| `/datasets/{id}/dlm/resolve` | GET | Resolve a question via the DLM |
| `/datasets/{id}/dlm/refresh` | POST | Incremental refresh (delta-merge for additive metrics) |
| `/datasets/{id}/freshness` | GET | Per-element staleness scores |
| `/dashboards/{id}/dlm/curate` | POST | Precompute N-dim combos from dashboard filters × chart groupbys |
| `/dlm/ask` | POST | Full question → answer pipeline |
| `/dlm/route` | GET | Route decision without execution |
| `/dlm/serve-chart` | POST | Context-first chart serving (single/multi-metric, filters) |
| `/dlm/filter-values` | GET | Distinct dimension values from context |
| `/dlm/coverage` | GET | Coverage report across all datasets |
| `/dlm/sweep` | POST | Manual freshness sweep trigger (Admin only) |
| `/dlm/notify-data-change` | POST | Pipeline webhook: invalidate + rebuild after ETL |
| `/dlm/cache/invalidate` | POST | Invalidate in-memory answer/sketch/spec caches |

The self-migrating tables `dlm_artifact` (with `manifest` and `curation` columns),
`dlm_value_index`, `dlm_router`, `dlm_answers`, and `dlm_sketch` sit alongside the
`context_snapshots` store this paper describes — the DLM is built **on top of** the
context engine, not instead of it.

---

*See also: `patent-adaptive-context-routing.md` for the filing-ready claim set,
`whitepaper-nl-to-sql.md` for the deterministic translation layer this router sits
behind, `whitepaper-dlm.md` for the standalone DLM deep dive, and
`whitepaper-dlm-curation.md` for curation at scale with production numbers.*
