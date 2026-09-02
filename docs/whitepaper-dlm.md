# The Data Language Model: A Self-Compiling Semantic Layer for Deterministic Natural Language Analytics

**Pruthvi Prodduturi**
August 2026

---

## Abstract

Large language models have become the default engine behind "talk to your data"
products: a user asks a question, the system sends the schema and the question to
an LLM, the LLM generates SQL, and the database executes it. This works, but it
is expensive ($0.01--0.10 per query), slow (2--5 seconds per round-trip),
non-deterministic, and requires sending schema metadata to a third-party API.

This paper presents the **Data Language Model (DLM)**: a per-dataset compiled
context artifact that answers natural-language questions **with no hosted LLM**
and, for the common cases, **with no database scan at all**. The DLM is compiled
once per dataset from the database's own statistics and a bounded set of
precomputation scans. At query time, it resolves user intent through stem-based
token matching and a seed synonym lexicon, serves answers from a precomputed
answer store in microseconds, and re-queries the warehouse only when the question
is genuinely novel. A freshness algorithm built on the database's own change
counters determines when each element of the context is still valid, triggering
targeted rebuilds only when -- and only where -- the data has actually moved.

On the Kaveon platform, the DLM serves as the **primary** query path. Over a
10.1-million-row synthetic usage dataset, a SUM across all rows drops from ~15
seconds (live scan) to ~1.5 seconds (context lookup, with the residual being
routing and resolution). Across 9 production dashboards (71 charts), 67 charts
(94.4%) are served entirely from DLM context with zero database queries. The
system answers 375 precomputed question shapes for a single dataset, uses zero
LLM calls per question, and leaks zero data to external APIs.

---

## 1. Introduction

Every "talk to your data" product faces the same pipeline: translate the question
into SQL, execute the SQL, visualize the result. The industry has converged on
LLMs for step one and result caches (TTL-based) for step two. Both are blunt
instruments.

The LLM is overkill for the 80% case. "Total revenue." "Sales by region." "Top
10 customers." These are pattern-matching problems, not open-ended language
understanding. An LLM that costs $0.03 and takes 3 seconds to answer "total
revenue" is doing unnecessary work.

The TTL cache is blind to reality. A 5-minute TTL re-queries unchanged data 12
times an hour and serves stale data the instant the table churns at second one.
The expiry clock has no relationship to whether the underlying data actually
changed.

The DLM replaces both with a single compiled artifact:

1. **No LLM for translation.** Intent is resolved through deterministic token
   matching with stem expansion and a seed synonym lexicon. The same question
   always produces the same answer.

2. **No scan for the common case.** Grand totals, per-dimension breakdowns, and
   filtered slices are precomputed at build time and served from an in-memory
   answer store. The warehouse is touched only when the question is genuinely
   novel.

3. **No clock for freshness.** A composite validity score built from the
   database's own change counters (`pg_stat_user_tables.n_mod_since_analyze`)
   determines whether each element of the context is still valid. The system
   re-queries exactly when -- and only the parts that -- the data has moved.

---

## 2. What a DLM Is

A DLM is a **per-dataset compiled context artifact** stored in a metadata
database, separate from the data warehouse. It contains:

| Component | Purpose | Storage |
|---|---|---|
| **Manifest** | Column types, join graph, metrics, dimensions, synonyms, context spec | `dlm_artifact.manifest` (JSON) |
| **Value index** | Normalized value → column/key mapping for entity resolution | `dlm_value_index` |
| **Router summary** | Cross-dataset keyword bag for multi-dataset question routing | `dlm_router` |
| **Precomputed answers** | Grand totals, per-dimension breakdowns, 2-dim combos | `dlm_answers` |
| **HLL sketch cuboids** | Sparse HyperLogLog registers for approximate COUNT DISTINCT | `dlm_sketch` |
| **Stats rollup** | Row count, date range, cardinalities, watermark for incremental refresh | `dlm_artifact.stats_rollup` (JSON) |
| **Curation overlay** | Human editorial overrides (aliases, hidden dims, default metrics) | `dlm_artifact.curation` (JSON) |

The DLM is **compiled, not trained.** There is no model, no gradient, no training
data. The compilation reads the database's own catalogs (`pg_stats`,
`pg_stat_user_tables`, `pg_constraint`) for structure and statistics, runs a
bounded set of GROUP BY scans for the answer store, and persists the result. The
artifact is fully deterministic: the same schema and data produce the same DLM.

---

## 3. Compilation Pipeline

`generate_dlm(dataset_id)` runs a 10-step pipeline:

**Step 1 — Fingerprint.** SHA-256 of the dataset definition (name, schema, table,
columns, metrics). If the hash matches an existing artifact with status `ready`,
the compilation short-circuits. `force=True` overrides.

**Step 2 — Statistics substrate.** Runs `ANALYZE` on all relevant tables, then
calls the context profiler to read `pg_stats` (column distributions, distinct
counts, null fractions, histograms) and `pg_stat_user_tables` (row counts,
modification counters). This is O(1) per table — a 100-million-row table costs
the same as a 100-row one.

**Step 3 — Value inventory.** For each low-cardinality dimension column (up to
1,000 distinct values), scans `SELECT col, COUNT(*) GROUP BY col LIMIT
1001`. Falls back to `pg_stats.most_common_vals` when the scan fails. Produces
normalized (value, frequency) pairs for entity resolution.

**Step 4 — Usage rollup.** Reads `query_history.tables_used` from the metadata
database to compute per-table query frequency. This drives the usage-weighted
decay factor in the freshness algorithm.

**Step 5 — Stats rollup.** Aggregates cardinalities, row counts, date range
(min/max of the date column, bounded to where defined metrics have non-null
data), and a freshness timestamp.

**Step 6 — Manifest assembly.** Deterministic map of: columns (with table, type,
semantic type, synonyms), join graph (dimension tables, fact keys, join keys),
metrics (name, expression, type, format, synonyms), and a `context_spec`
(auto-suggested aliases, additivity flags, precompute configuration).

**Step 7 — Persist.** Atomically upserts `dlm_artifact`, `dlm_value_index`, and
`dlm_router` in the metadata database.

**Step 8 — Precompute answers.** Four tiers of precomputation (detailed in
Section 5 and in the companion paper *DLM Curation at Scale*).

**Step 9 — Dashboard curation.** Finds dashboards referencing this dataset and
precomputes N-dimensional combos for their filter × chart definitions (up to 4
dimensions, capped at 5,000 cells per combo).

**Step 10 — Watermark.** Records `row_count` and `max_date` for incremental
refresh tracking.

---

## 4. Intent Resolution (No LLM)

When a user asks "what are the carbon outputs by country?", the DLM must resolve
this to a specific metric, a specific dimension, and zero or more entity filters
-- without calling an LLM. The resolution pipeline:

### 4.1 Tokenization and Expansion

The question is tokenized, stopwords are removed, and each token is expanded
through two layers:

**Suffix stemmer.** A lightweight rule-based stemmer with 11 suffix rules handles
common English inflections with no external dependencies. Representative rules:
- `-ies` → `-y` (countries → country)
- `-tion`/`-sion` → `-e` or strip (emission → emiss → emit)
- `-ing` with doubled consonant (selling → sell)
- `-ed`, `-er`, `-es`, `-s` with length guards

**Seed synonym lexicon.** 56 synonym families spanning finance, health, energy,
transport, geography, time, and technology domains:
```
revenue: [sales, turnover, income, sell, sold, earned]
emission: [emissions, co2, carbon, ghg, greenhouse, pollution]
deaths: [mortality, fatality, died, killed, deceased]
```

`_expand_tokens` stems every token, looks up its synonym group (falling back to
the stemmed form), and produces the expanded set. The expansion is **symmetric**:
both the question tokens and the metric/dimension candidate tokens are expanded,
so "carbon outputs" matches a metric named `total_emissions` via the
emission↔carbon synonym link.

### 4.2 Metric Matching

`_match_metric` scores each defined metric against the expanded question tokens.
Generic quantifier words ("total", "sum", "count") are excluded from matching to
prevent false positives. The fallback chain:
1. Curated default metric (if set in the context spec)
2. Simplest additive metric (COUNT(*) or SUM)
3. First non-AVG, non-DISTINCT metric
4. First metric

### 4.3 Group-By Resolution

`_match_group_by` detects grouping keywords ("by", "per", "across", "for each")
via regex, then matches the trailing tokens against dimension names and curated
aliases using the same stem+synonym expansion.

### 4.4 Entity Filter Resolution

`_resolve_entity_filters` scans the question for entity mentions using the value
index. It tests n-grams in descending length (3-word, 2-word, 1-word), resolves
each against `dlm_value_index` with fuzzy matching (cutoff 0.84), and produces
equality filters. One filter per column; stopwords are excluded.

### 4.5 Additional Extractors

- **Top-N:** regex `\btop\s+(\d+)\b`; bare "top" defaults to 10. Superlatives
  ("which country has the most") resolve to top-1.
- **Year filter:** regex `\b(19|20)\d{2}\b`. Smart: if the year spans the entire
  dataset range, it's dropped (no-op filter → context hit). If beyond data range,
  falls back to latest available year with a note.
- **Relative time:** "last N days/weeks/months/years", "this week/month/quarter/year",
  "today", "yesterday" → `CURRENT_DATE - INTERVAL '...'`.
- **Trend detection:** "over time", "by year", "monthly", "trend" → line chart
  with GROUP BY date column.

### 4.6 Multi-Dataset Routing

When a workspace has multiple datasets, `route()` scores each compiled DLM:

```
score = 4 × name_hits + 3 × metric_hits + 2 × value_hits + min(col_hits, 3)
```

The column hit cap at 3 prevents broad datasets (many columns) from dominating.
A floor of 2 rejects single stray generic-word matches. Tiebreaker: narrower
datasets are preferred (fewer columns = more focused).

---

## 5. Precomputed Answer Store

The DLM precomputes answers at build time so the common case requires no database
scan at query time. Four tiers:

**Tier 1 — Grand totals.** One scan computes every metric simultaneously:
`SELECT expr0, expr1, ... FROM fact`. Each metric's total is stored as a separate
answer row with `group_col = ''`.

**Tier 2 — Per-dimension breakdowns.** One scan per dimension:
`SELECT dim, expr0, expr1, ... FROM fact WHERE dim IS NOT NULL GROUP BY dim
ORDER BY m0 DESC LIMIT depth`. Default depth is 500, configurable per dimension
via the curation overlay.

**Tier 3 — Two-dimensional combos.** For dimension pairs where both have up to
500 distinct values and the cell product is under 5,000:
`SELECT d1, d2, expr0, ... FROM fact GROUP BY d1, d2 LIMIT 5000`. Maximum 12
pairs per dataset. Stored with `group_col = "d1|d2"` (pipe-delimited,
lexicographic). Enables two-filter questions ("queries in Asia for Enterprise")
to serve from context.

**Tier 4 — HLL sketch cuboids.** For non-additive `COUNT(DISTINCT col)` metrics.
HyperLogLog registers (p=11, 2,048 registers, ~2.3% standard error) are computed
in one SQL scan using `hashtextextended()` — pure SQL, no extension required. At
query time, register vectors merge in Python to produce approximate NDV at
arbitrary dimension slices. Maximum 6 dimensions, 8,000 cells.

Non-additive metrics (COUNT DISTINCT, AVG) are computed **independently per
shape** at build time — never derived by summing a breakdown. "Active Users for
Enterprise" is exact, not an illegal roll-up.

### Storage

All precomputed answers live in `dlm_answers`, keyed by `(dataset_id,
metric_name, group_col)` with an ON CONFLICT upsert. The table sits in the
metadata database (`kaveonmeta`), physically separate from the data warehouse.
On first access, answers are loaded into an in-memory dict (`_ANSWER_CACHE`) and
served as microsecond dict lookups until invalidated by a rebuild.

---

## 6. The Freshness Algorithm

A precomputed answer is only useful if you know it's still correct. The DLM's
freshness algorithm replaces TTL-based expiry with a measurement-based validity
score that reads the database's own change counters — detecting staleness at
effectively zero cost.

### 6.1 Composite Score

```
score = time_decay(age, effective_half_life(usage)) × change_factor(delta_rows, row_count)
```

The score is in [0, 1]. Multiplicative: either factor collapsing to zero
invalidates the element.

### 6.2 Factor 1 — Time Decay

```
time_factor = e^(-ln2 × age_seconds / effective_half_life)
```

Base half-life: **6 hours** (`BASE_HALF_LIFE_SECONDS = 21,600`). An unused,
unchanged element reaches 0.5 validity after 6 hours.

| Age | Score |
|-----|-------|
| 0h | 1.000 |
| 1h | 0.891 |
| 3h | 0.707 |
| 6h | 0.500 |
| 12h | 0.250 |
| 24h | 0.063 |

### 6.3 Factor 2 — Change Detection (Zero-Scan)

```
delta = max(0, n_mod_since_analyze_now - n_mod_since_analyze_at_capture)
change_frac = delta / row_count_at_capture
change_factor = e^(-ln2 × change_frac / CHANGE_HALF_FRACTION)
```

`CHANGE_HALF_FRACTION = 0.05` (5%). A table that has churned 5% of its rows
since capture has its change factor fall to 0.5; a table that hasn't moved stays
at 1.0.

The change signal is read from `pg_stat_user_tables.n_mod_since_analyze` — a
running count of inserts, updates, and deletes that PostgreSQL maintains for its
own autovacuum scheduling. **Reading this counter is a metadata lookup, not a
table scan.** This is the enablement core: staleness is measured from a counter,
not from a re-query.

| Rows Churned | Score |
|-------------|-------|
| 0% | 1.000 |
| 1% | 0.871 |
| 5% | 0.500 |
| 10% | 0.250 |
| 20% | 0.063 |

**Autovacuum edge case.** When `last_analyze` changes but the counter resets
(autovacuum re-analyzed the table), the algorithm treats this as at least 5%
modification to avoid a false "fresh" signal.

### 6.4 Factor 3 — Usage-Weighted Decay

```
effective_half_life = BASE_HALF_LIFE / (1 + USAGE_GAIN × ln(1 + usage_count))
```

`USAGE_GAIN = 0.35`. Floor: `MIN_HALF_LIFE_SECONDS = 600` (10 minutes).

Hot elements decay faster. The rationale: the answers users rely on most are the
ones where a stale answer does the most damage.

| Usage Count | Effective Half-Life |
|-------------|-------------------|
| 0 | 6h 00m |
| 10 | 3h 16m |
| 100 | 2h 18m |
| 1,000 | 1h 45m |
| 10,000 | 1h 25m |

### 6.5 Routing Decision

Given the validity scores of all context elements relevant to a question and a
threshold (default `DEFAULT_THRESHOLD = 0.70`):

| Condition | Route | Behavior |
|---|---|---|
| All elements ≥ 0.70 | **context** | Answer from precomputed store, no query |
| Some below 0.70 | **hybrid** | Query only the stale elements |
| All below 0.70 | **query** | Full live query |

### 6.6 Self-Healing

When a live query executes because the context was judged stale, the router
refreshes the change counters of the touched elements to a fresh state. The
subsequent identical question is then served from context until the data moves
again. Staleness triggers a refresh; the refresh restores validity. The system
self-heals around change.

### 6.7 Three Rebuild Triggers

**On-ask.** `maybe_auto_rebuild()` checks freshness on every question. If the
score falls below 0.5 (`_STALE_THRESHOLD`), a background thread triggers
`generate_dlm(force=True)` with a 5-minute cooldown between attempts.

**Proactive sweep.** A background daemon runs `freshness_sweep()` every 30
minutes, checking all datasets with status `ready` and triggering rebuilds for
stale ones. The daemon starts at API boot.

**Pipeline webhook.** `POST /dlm/notify-data-change?dataset_id=X` provides
instant invalidation after ETL. It clears the in-memory caches and triggers a
background rebuild.

---

## 7. Answering a Question

`ask(question)` is the full NL→answer pipeline:

1. **Route** to the best dataset (multi-dataset scoring, §4.6).
2. **Resolve intent:** metric, group-by, entity filters, top-N, year, relative
   time, trend (§4.1--4.5).
3. **Try context path.** If no time-group, no year, no relative-time filter:
   attempt `_serve_from_context`. On miss for COUNT DISTINCT, try
   `_serve_sketch`. If context hits, return immediately with source labelled
   "⚡ From context · no DB scan."
4. **Fall to live query.** Assemble SELECT/WHERE/GROUP BY/ORDER BY/LIMIT SQL
   from resolved components. Execute against the warehouse. Cache the result
   with its element dependencies. Refresh stale elements. Return with source
   labelled "Live query · Xs."
5. **Context hints.** Even when the live path runs, the system returns
   precomputed single-dimension slices as instant previews while the query
   executes — the user sees partial data immediately.

### 7.1 Dashboard Chart Serving

`serve_chart(dataset_id, metric_column, aggregation, group_by, filters)` maps a
dashboard chart's configuration directly to a precomputed answer:

1. Resolve `(column, aggregation)` to a named metric via expression matching.
2. Normalize filters (handle IN operators by splitting to individual values).
3. Serve from `_serve_from_context` or `_serve_sketch`.
4. Return `{columns, rows, served, freshness_score}`.

The frontend tries DLM first (`POST /dlm/serve-chart`) and falls through to SQL
only when the context can't answer. On a B1ms instance (1 vCore, 2 GB), this
eliminates the 2--7 minute VIEW scans that caused 500 timeouts, replacing them
with ~5 ms context lookups.

---

## 8. Implementation

The DLM is implemented in pure Python with no ML dependencies:

| File | Responsibility |
|---|---|
| `api/dlm/engine.py` | Compilation, intent resolution, and answer serving |
| `api/dlm/hll.py` | HyperLogLog implementation |
| `api/dlm/validity.py` | Validity scoring and freshness decisions |
| `api/dlm/profiler.py` | Context building and change-counter capture |
| `api/dlm/router.py` | Question-to-element mapping and route selection |
| `api/routers/dlm.py` | REST API |

The intent resolution uses a 56-family seed synonym lexicon
and a lightweight suffix stemmer, both hand-written. No NLP library, no ML model,
no training data. The deterministic DLM route makes no external model API calls.

---

## 9. Results

### 9.1 Production Dataset (10.1M rows)

| Question | Path | Latency | DB Scans |
|---|---|---|---|
| "What is current Kaveon usage?" (SUM over 10.1M rows) | Context | ~1.5s (was 15s live) | 0 |
| "queries by plan" | Context | <10ms | 0 |
| "active users by role" | Context | <10ms | 0 |
| "queries for Enterprise" (single-dim filter) | Context | <10ms | 0 |
| "queries by plan in 2026" (year slice, not precomputed) | Live | ~3s | 1 |

375 precomputed answers for this single dataset.

### 9.2 Dashboard Coverage

| Metric | Value |
|---|---|
| Published dashboards | 9 |
| Total charts | 71 |
| Charts served from DLM context | 67 (94.4%) |
| Charts on SQL path (by design) | 4 (scatter/bubble, no metric aggregation) |
| Dashboard-curated N-dim answers | 52 across 4 dashboards |

### 9.3 NL→SQL Accuracy

| Test Suite | Accuracy |
|---|---|
| 105-question standard suite | 95%+ |
| 55-question adversarial suite | 92.4% |

### 9.4 Cost Comparison

| Dimension | DLM | LLM-based NL→SQL |
|---|---|---|
| Latency (common case) | <10ms | 2--5s |
| Cost per query | $0 | $0.01--0.10 |
| LLM calls per question | 0 | 1+ |
| Data egress | 0 | Schema sent to API |
| Deterministic | Yes | No |
| Hallucination risk | 0% | Moderate |

---

## 10. Limitations

**No open-ended reasoning.** "Why did revenue drop?" requires causal analysis
that a deterministic system cannot provide. The DLM owns the 80% "fetch the
right data" question; open-ended reasoning is a different problem.

**Single-table scope.** The DLM operates on single tables (or views). Cross-table
joins are not supported. Mitigated by Kaveon's dataset abstraction — a dataset
can be backed by a view that pre-joins multiple tables.

**Time-series trends fall to live query.** "Revenue over time" requires a GROUP
BY date with ordering — a shape the precomputed store does not cover because the
time-slice matrix is unbounded. These fall to the live path.

**PostgreSQL-specific change counters.** The freshness algorithm reads
`n_mod_since_analyze` from `pg_stat_user_tables`. SQL Server
(`sys.dm_db_stats_properties.modification_counter`), MySQL
(`information_schema.STATISTICS`), and DuckDB expose analogous catalogs; the
method transfers, the catalog names do not.

---

## 11. Conclusion

The Data Language Model is a compiled, deterministic alternative to LLM-based
natural language analytics. It replaces the LLM with stem-based token matching
and a seed synonym lexicon. It replaces the TTL cache with a validity score built
from the database's own change counters. And it replaces the per-query scan with
a precomputed answer store that serves the common case in microseconds.

The result is a system that answers from context by default and reaches for the
database only when — and only where — the data has actually moved. No model. No
API key. No data egress. No hallucination. Just a schema and a compiler.

---

*See also: `whitepaper-dlm-curation.md` for how Kaveon curates 375 answers across
10 million rows, `whitepaper-adaptive-context-routing.md` for the per-element
staleness routing method, `whitepaper-nl-to-sql.md` for the template-based
translation fallback, and `patent-adaptive-context-routing.md` for the
21-claim patent draft.*
