# DLM Curation at Scale: How Kaveon Precomputes 375 Answers Across 10 Million Rows

**Pruthvi Prodduturi**
August 2026

> **Implementation and evidence boundary.** Counts and timings below are a dated
> case study for the named datasets, dashboards, database size, and cache state;
> they are not universal product guarantees. HLL results are approximate, and
> compiled coverage depends on connector capabilities and the selected curation
> budget. See [`STATUS.md`](../STATUS.md) for the current capability ledger.

---

## Abstract

Dashboard-driven analytics products hit the database for every chart render. On a
resource-constrained instance, this means 6--10 concurrent GROUP BY scans against
a multi-million-row fact table, each competing for the same 2 GB of memory. The
result is 500 timeouts, 2--7 minute load times, and an unusable product.

This paper describes how Kaveon's **Data Language Model (DLM)** eliminates this
problem through **precomputed curation**: a bounded set of scans at build time
that produces a complete answer store — grand totals, per-dimension breakdowns,
two-dimensional combos, and HyperLogLog sketch cuboids — from which 94.4% of
dashboard charts and the majority of natural-language questions are served with
**zero database queries**. The system stores 375 precomputed answers for a single
10.1-million-row dataset, serves them as microsecond dict lookups from an
in-memory cache, and keeps them current through a freshness algorithm that reads
the database's own change counters rather than re-scanning the data.

This paper provides the exact SQL templates, the constants that govern the
precomputation budget, the storage schema, the incremental refresh mechanism, and
the production numbers across 10 datasets, 9 dashboards, and 71 charts.

---

## 1. The Problem

Consider a dashboard with 10 charts over a 10.1-million-row fact table on a B1ms
PostgreSQL instance (1 vCore, 2 GB RAM). Each chart issues a GROUP BY query. Ten
concurrent scans against the same table:

- Each scan reads ~10M rows.
- Total I/O: ~100M row reads per dashboard load.
- On a B1ms instance, this exceeds the buffer pool. Pages evict and re-read.
- Individual charts take 2--7 minutes. Some timeout at the 30-second proxy limit.

The standard mitigation is a result cache with a TTL. But a TTL cache:

- Expires answers on a clock unrelated to whether the data changed.
- Requires each chart to "warm" the cache on first load (still a 10M-row scan).
- Invalidates the entire cache atomically — no partial refresh.
- Cannot answer a question that wasn't previously asked.

The DLM replaces this with **precomputed curation**: the common question shapes
are computed at build time and served without any database query.

---

## 2. The Curation Model

The DLM precomputes answers in four tiers, each progressively more selective.
Every tier runs during `generate_dlm()` — a single compilation pass that
typically completes in 30--120 seconds depending on dataset size.

### 2.1 What Gets Precomputed

For a dataset with **M** metrics and **D** dimension columns:

| Tier | Shape | Count | Scan Cost |
|---|---|---|---|
| 1. Grand totals | All metrics, no grouping | M | 1 scan |
| 2. Per-dimension breakdowns | Each metric × each dimension | M × D | D scans |
| 3. Two-dim combos | Each metric × qualifying dim pairs | M × P (P ≤ 12) | P scans |
| 4. HLL sketch cuboids | COUNT DISTINCT metrics × low-card dims | 1 base cuboid | 1 scan per metric |

### 2.2 What Does Not Get Precomputed

- Time-window slices ("revenue in Q3 2025") — the time-slice matrix is unbounded.
- Three-or-more-filter intersections (unless dashboard-curated, §7).
- High-cardinality dimension breakdowns (more than 500 distinct values).
- Scatter/bubble charts (no metric aggregation — direct x/y column mapping).

---

## 3. Tier 1: Grand Totals

**One scan, all metrics at once.**

```sql
SELECT SUM(total_queries) AS m0,
       SUM(active_users)  AS m1,
       AVG(response_time) AS m2,
       COUNT(DISTINCT user_id) AS m3
FROM public.kaveon_product_analytics
```

Each metric value is stored as a separate row in `dlm_answers` with
`group_col = ''` (empty string = ungrouped). For the Kaveon Product Usage
dataset (dataset 142), this produces **15 answer rows** (one per metric).

Non-additive metrics (AVG, COUNT DISTINCT) are computed here independently — not
derived from Tier 2 breakdowns. This guarantees exactness: "average response
time" is the true AVG over all rows, not the average of per-group averages.

---

## 4. Tier 2: Per-Dimension Breakdowns

**One scan per dimension.**

```sql
SELECT subscription_plan AS grp,
       SUM(total_queries) AS m0,
       SUM(active_users)  AS m1,
       AVG(response_time) AS m2
FROM public.kaveon_product_analytics
WHERE subscription_plan IS NOT NULL
GROUP BY subscription_plan
ORDER BY m0 DESC
LIMIT 500
```

For a dataset with 15 metrics and 7 dimensions, this produces up to **15 × 7 =
105 answer rows** (fewer when some dimensions have low cardinality). Each row is
stored with `group_col = 'subscription_plan'` (the dimension name).

**Depth control.** The default `LIMIT` is 500 rows per dimension, configurable
per dimension via the curation overlay (e.g., a "country" dimension might be set
to `top_n: 250`). Dimensions marked `precompute: false` or `hidden: true` in the
curation spec are skipped.

**What this enables at query time:**

- "Total queries by plan" → direct lookup of `(metric='total_queries', group_col='subscription_plan')`.
- "Queries for Enterprise" → select the row where `grp = 'Enterprise'` from the
  per-plan breakdown. **Still no database query** — the filter is applied to the
  precomputed breakdown in memory.
- "Top 5 countries by active users" → read the first 5 rows of the sorted
  per-country breakdown. Instant.

---

## 5. Tier 3: Two-Dimensional Combos

**Qualifying pairs only — bounded by cell product.**

For dimension pairs where **both** have fewer than `HIGH_CARD = 500` distinct
values and the cell product (`card(d1) × card(d2)`) is under `CELL_CAP = 5,000`:

```sql
SELECT subscription_plan AS g1,
       user_role          AS g2,
       SUM(total_queries) AS m0,
       SUM(active_users)  AS m1
FROM public.kaveon_product_analytics
WHERE subscription_plan IS NOT NULL
  AND user_role IS NOT NULL
GROUP BY subscription_plan, user_role
ORDER BY m0 DESC
LIMIT 5000
```

The pair is stored with `group_col = 'subscription_plan|user_role'`
(pipe-delimited, lexicographically ordered). Maximum **12 pairs** per dataset
(`MAX_PAIRS = 12`).

**What this enables at query time:**

- "Queries in Asia for Enterprise" → two filters on different dimensions. The
  engine finds the precomputed combo covering both dimensions, filters in memory,
  and returns the result. **No database query.**
- "Active users by role where plan = Enterprise" → one filter + one group-by on
  different dimensions. The engine reads the `plan|role` combo, filters to
  `plan = Enterprise`, and returns the role breakdown. Instant.

**Why the constraints matter:**

| Constraint | Value | Rationale |
|---|---|---|
| `HIGH_CARD` | 500 | A 500 × 500 = 250K cell combo would produce a massive GROUP BY — unacceptable build time and storage |
| `CELL_CAP` | 5,000 | Upper bound on rows per combo, even for qualifying pairs |
| `MAX_PAIRS` | 12 | Prevents combinatorial explosion when a dataset has many low-cardinality dimensions |

For the Kaveon Product Usage dataset: 7 dimensions, of which 5 qualify (< 500
cardinality). That gives `C(5,2) = 10` potential pairs, all under the cell cap.
Each pair × 15 metrics = up to **150 answer rows** from this tier.

---

## 6. Tier 4: HLL Sketch Cuboids

Non-additive `COUNT(DISTINCT col)` metrics cannot be answered by summing
precomputed breakdowns. "Distinct users for Enterprise" is not the sum of
"distinct users for Enterprise in Asia" + "... in Europe" — users may span
regions. The DLM solves this with **HyperLogLog sketch cuboids**.

### 6.1 Build-Time: One Scan

```sql
WITH h AS (
  SELECT subscription_plan, user_role, region,
         hashtextextended(CAST(user_id AS text), 0)::bit(64) AS b
  FROM public.kaveon_product_analytics
  WHERE user_id IS NOT NULL
),
e AS (
  SELECT subscription_plan, user_role, region,
         substring(b from 1 for 11)::text  AS reg,
         CASE WHEN position('1' in substring(b from 12 for 53)::text) = 0
              THEN 54
              ELSE position('1' in substring(b from 12 for 53)::text)
         END AS rho
  FROM h
)
SELECT subscription_plan, user_role, region,
       reg, MAX(rho) AS rho
FROM e
GROUP BY subscription_plan, user_role, region, reg
```

This produces one **base cuboid** over the low-cardinality dimensions. Each cell
(e.g., `plan=Enterprise, role=Admin, region=Asia`) stores a sparse register
vector (only non-zero registers, JSON-serialized).

**Constants:**

| Parameter | Value | Effect |
|---|---|---|
| `P` (precision bits) | 11 | 2^11 = 2,048 registers per cell |
| `RBITS` | 53 (= 64 - 11) | Remaining bits for rho computation |
| Standard error | ~2.3% | Mean error at 20M rows; p95 = 4.1% |
| `SKETCH_MAX_DIMS` | 6 | Max dimensions in a cuboid |
| `SKETCH_MAX_CELLS` | 8,000 | Max cells before the cuboid is too expensive |

### 6.2 Query-Time: Register Merge

Any sub-combination of the cuboid's dimensions is answerable by **unioning
register vectors** — an element-wise max across the relevant cells, followed by
the HLL cardinality estimator. This is pure Python, no database query.

"Distinct users for Enterprise" → union all cells where `plan = Enterprise`,
estimate. ~2.3% error, 15.7× smaller than raw, 20--30× faster than a live
`COUNT(DISTINCT)`.

The sketch cuboid requires at least 2 dimensions (single-dimension distinct
counts are already exact from Tier 2). Pure SQL, no `hll` extension required.

---

## 7. Dashboard-Level Curation

Dashboards often combine filters and chart group-bys in patterns that exceed the
2-dim combos of Tier 3. A dashboard with filters on `region` and `plan`, showing
a chart grouped by `role`, needs a 3-dimensional answer
(`region × plan × role`). The DLM handles this with **dashboard-level curation**.

### 7.1 How It Works

`curate_dashboard(dashboard_id)` derives the required N-dimensional combos from
the dashboard's definition:

1. Read the dashboard's filter definitions → a set of filter dimensions.
2. Read each chart's group-by column → a set of chart dimensions.
3. For each subset of filter dimensions (1 to `MAX_CURATION_DIM = 4`), crossed
   with each chart group-by: if the combined dimensionality ≤ 4, precompute:

```sql
SELECT region AS g0, subscription_plan AS g1, user_role AS g2,
       SUM(total_queries) AS m0, SUM(active_users) AS m1
FROM public.kaveon_product_analytics
WHERE region IS NOT NULL
  AND subscription_plan IS NOT NULL
  AND user_role IS NOT NULL
GROUP BY region, subscription_plan, user_role
ORDER BY m0 DESC
LIMIT 5000
```

Stored with `group_col = 'region|subscription_plan|user_role'`.

### 7.2 Auto-Curation

Dashboard curation runs automatically at the end of `generate_dlm()` for every
dashboard that references the dataset. No manual step required.

### 7.3 Production Numbers

| Metric | Value |
|---|---|
| Dashboards auto-curated | 4 |
| Dashboard-curated N-dim answers | 52 |
| Max dimensionality | 4 (capped at `MAX_CURATION_DIM`) |
| Max rows per combo | 5,000 (capped at `CURATION_CELL_CAP`) |

---

## 8. Incremental Refresh

When new data arrives, the DLM does not always need a full rebuild.
`incremental_refresh(dataset_id)` implements delta processing:

### 8.1 Decision Logic

1. Read the watermark from `stats_rollup`: `(row_count, max_date)`.
2. Get current row count from the database.
3. If no new rows → return `no_new_data`.
4. If new rows < 50% of total, a date column exists, and additive metrics exist →
   **delta merge**.
5. Otherwise → **full rebuild** (`generate_dlm(force=True)`).

### 8.2 Delta Merge (Additive Metrics)

For metrics classified as **additive** (SUM, COUNT without DISTINCT):

**Grand total delta:**
```sql
SELECT SUM(total_queries) AS v
FROM public.kaveon_product_analytics
WHERE event_date > '2026-08-15'   -- prev_date from watermark
```
New total = old total + delta. One scan covers all additive metrics.

**Per-dimension breakdown delta:**
```sql
SELECT subscription_plan, SUM(total_queries) AS v
FROM public.kaveon_product_analytics
WHERE event_date > '2026-08-15'
  AND subscription_plan IS NOT NULL
GROUP BY subscription_plan
```
Merges into existing rows: adds delta to existing cells, appends new cells.

### 8.3 What Triggers a Full Rebuild

- New rows ≥ 50% of total (the delta is too large to be meaningful).
- Non-additive metrics (COUNT DISTINCT, AVG, MIN, MAX) need exact recomputation.
- No date column to bound the delta scan.
- The `force` flag is set.

### 8.4 Metric Classification

`_metric_agg_type(expression)` classifies each metric:

| Expression Pattern | Classification | Delta-Mergeable |
|---|---|---|
| `SUM(...)` | additive | Yes |
| `COUNT(*)` | additive | Yes |
| `COUNT(col)` (no DISTINCT) | additive | Yes |
| `MIN(...)`, `MAX(...)` | semi-additive | No (new min/max could change result) |
| `AVG(...)` | non-additive | No |
| `COUNT(DISTINCT ...)` | non-additive | No |

---

## 9. The Storage Model

### 9.1 Schema

Five tables in the metadata database (`kaveonmeta`):

```
dlm_artifact       -- one row per dataset: manifest, stats_rollup, usage_rollup,
                      curation overlay, source_hash, status
dlm_value_index    -- value → column/key resolution for entity filters
dlm_router         -- cross-dataset routing summaries
dlm_answers        -- precomputed results (this paper's focus)
dlm_sketch         -- HLL registers for COUNT DISTINCT
```

### 9.2 `dlm_answers` Schema

| Column | Type | Description |
|---|---|---|
| `id` | UUID | Primary key |
| `dataset_id` | INT | Foreign key to datasets |
| `metric_name` | TEXT | Metric being answered |
| `group_col` | TEXT | `''` = total, `'dim'` = 1-dim, `'d1\|d2'` = 2-dim |
| `columns` | JSON | Column headers `["grp", "value"]` |
| `rows` | JSON | Row data `[["Enterprise", 12345], ...]` |
| `computed_at` | TIMESTAMP | When this answer was last computed |

**Unique index:** `(dataset_id, metric_name, group_col)` — enables the ON
CONFLICT upsert pattern:

```sql
INSERT INTO dlm_answers (id, dataset_id, metric_name, group_col, columns, rows, computed_at)
VALUES ($1, $2, $3, $4, $5, $6, $7)
ON CONFLICT (dataset_id, metric_name, group_col) DO UPDATE SET
  columns = EXCLUDED.columns,
  rows = EXCLUDED.rows,
  computed_at = EXCLUDED.computed_at
```

### 9.3 In-Memory Cache

On first access, all `dlm_answers` rows for a dataset are loaded into
`_ANSWER_CACHE` — a Python dict keyed by `(dataset_id, metric_name, group_col)`.
Subsequent lookups are microsecond dict hits. The cache is invalidated on:

- `generate_dlm()` completion (full rebuild).
- `incremental_refresh()` completion (delta merge).
- `POST /dlm/cache/invalidate` (manual or webhook).
- API restart.

---

## 10. Serving from Context

### 10.1 Natural Language Path

`_serve_from_context(dataset_id, metric_name, group_col, filters)` handles
progressively complex shapes:

| Shape | Resolution |
|---|---|
| No filters, no group | Grand total lookup |
| No filters, with group | Per-dimension breakdown lookup |
| 1 equality filter, no group | Select matching row from 1-dim breakdown |
| 1 equality filter + group | 2-dim combo lookup, filter one dim, return other |
| 2 equality filters, no group | Cell lookup from 2-dim combo |
| N filters ± group | Dashboard-curated N-dim combo lookup |

**Proportional multi-filter fallback.** When exact N-dim combos are not
precomputed, the system approximates by scaling single-dim breakdowns with each
filter's proportional weight: `scaled = grand_total × w1 × w2 × ...` (for
additive metrics only; intensive metrics like AVG skip scaling). Marked
`approx = True` in the response.

### 10.2 Dashboard Chart Path

`serve_chart(dataset_id, metric_column, aggregation, group_by, filters)` maps a
dashboard chart's config to a precomputed answer:

1. **Metric resolution.** Maps `(column, aggregation)` to a named metric by
   matching the metric's expression against the chart's column + aggregation
   function.
2. **Filter normalization.** Handles IN operators by splitting to individual
   values and unioning matching rows.
3. **Context lookup.** Delegates to `_serve_from_context` or `_serve_sketch`.
4. **Freshness annotation.** Attaches the composite freshness score so the
   frontend knows the answer's validity.

`serve_chart_multi` handles stacked bar and combo charts with multiple metrics.
Returns `served = True` only when **all** requested metrics can be answered from
context — no partial results that would produce misleading charts.

---

## 11. Production Numbers

### 11.1 Per-Dataset Answer Counts

| ID | Dataset | Rows | Metrics | Dimensions | Precomputed Answers |
|----|---------|------|---------|------------|---------------------|
| 132 | Global Energy Consumption | 1,200 | ~10 | ~7 | 72 |
| 133 | Global Temperature Anomaly | 19,000 | ~7 | ~7 | 50 |
| 134 | Climate x Energy | 1,200 | ~3 | ~7 | 21 |
| 135 | AI Model Leaderboard | 34 | ~10 | ~7 | 77 |
| 138 | AI Model Pricing | 27 | ~4 | ~2 | 8 |
| 139 | COVID-19 Global | 51,000 | ~5 | ~6 | 30 |
| **142** | **Kaveon Product Usage (Synth)** | **10,100,000** | **15** | **7** | **375** |

*7 of 10 registered datasets have DLM compiled; the remaining 3 are registered
but do not yet have precomputed answers.*

### 11.2 Dataset 142 Breakdown (375 Answers)

The Kaveon Product Usage dataset — 10.1M rows, 15 metrics, 7 dimensions — is the
system's scale proof point.

| Tier | Answers | Build Cost |
|---|---|---|
| Grand totals (15 metrics × 1) | 15 | 1 scan |
| Per-dimension breakdowns (15 × 7) | 105 | 7 scans |
| 2-dim combos (15 × ~10 pairs) | ~150 | 10 scans |
| Dashboard-curated N-dim | ~52 | 4--8 scans |
| HLL sketch cuboids | ~53 | 1--3 scans |
| **Total** | **375** | **~25 scans** |

**Build time:** ~60--90 seconds for the full compilation. The 25 scans run
sequentially but each is a single GROUP BY over the fact table — PostgreSQL's
sequential scan is efficient when the entire table is read.

**Storage:** ~375 rows in `dlm_answers` (JSON columns + rows), plus sparse HLL
register vectors in `dlm_sketch`. Total metadata footprint: < 5 MB.

### 11.3 Dashboard Coverage

| Dashboard | Charts | DLM Served | SQL Path | Coverage |
|-----------|--------|------------|----------|----------|
| Kaveon Product Usage | 8 | 8 | 0 | 100% |
| Kaveon Adoption & Engagement | 8 | 8 | 0 | 100% |
| Kaveon Platform & Growth | 10 | 10 | 0 | 100% |
| AI Model Arena | 10 | 8 | 2 (scatter) | 80% |
| Global Energy | 8 | 8 | 0 | 100% |
| Global Climate | 7 | 7 | 0 | 100% |
| Climate x Energy Impact | 8 | 6 | 2 (scatter) | 75% |
| COVID-19 Global Overview | 6 | 6 | 0 | 100% |
| NYC Yellow Taxi | 6 | 6 | 0 | 100% |
| **Total** | **71** | **67** | **4** | **94.4%** |

The 4 charts on the SQL path are scatter/bubble charts that plot raw x/y columns
with no metric aggregation — they don't produce a GROUP BY that the answer store
can serve. This is by design, not a gap.

### 11.4 Performance Impact

| Metric | Before DLM | After DLM |
|---|---|---|
| Dashboard load (10 charts, 10.1M rows, B1ms) | 2--7 min, frequent 500 timeouts | <500ms, zero timeouts |
| "What is current usage?" (SUM over 10.1M rows) | ~15 seconds | ~1.5 seconds |
| Per-dimension breakdown | ~8 seconds | <10ms |
| Single-dimension filter | ~8 seconds | <10ms |
| Chart serve (serve-chart endpoint) | ~5--30 seconds | ~5ms |

### 11.5 Infrastructure

| Component | Spec | Cost |
|---|---|---|
| Metadata DB (`kaveonmeta`) | Azure PG B1ms, 1 vCore, 2 GB | ~$15/month |
| Data warehouse (`kaveon`) | Azure PG B1ms, 1 vCore, 2 GB | ~$15/month |
| API (Azure Container Apps) | 0.5 vCPU, 1 GB | ~$10/month |
| Frontend (Vercel) | Hobby plan | $0 |

375 precomputed answers serving 67 dashboard charts and natural-language queries
over 10 million rows, on $40/month of infrastructure.

---

## 12. Constants Reference

| Constant | Value | File | Purpose |
|---|---|---|---|
| `_MAX_CARDINALITY_FOR_VALUES` | 1,000 | dlm.py | Max distinct values to index per dimension |
| `HIGH_CARD` | 500 | dlm.py | Max cardinality for 2-dim combo eligibility |
| `CELL_CAP` | 5,000 | dlm.py | Max cells per 2-dim combo |
| `MAX_PAIRS` | 12 | dlm.py | Max 2-dim combos per dataset |
| `SKETCH_MAX_DIMS` | 6 | dlm.py | Max dimensions in HLL cuboid |
| `SKETCH_MAX_CELLS` | 8,000 | dlm.py | Max cells in HLL cuboid |
| `MAX_CURATION_DIM` | 4 | dlm.py | Max dimensions in dashboard-curated combo |
| `CURATION_CELL_CAP` | 5,000 | dlm.py | Max rows per dashboard-curated combo |
| `P` (HLL precision) | 11 | hll.py | 2^11 = 2,048 registers, ~2.3% error |
| `BASE_HALF_LIFE_SECONDS` | 21,600 (6h) | context_validity.py | Time-decay base |
| `CHANGE_HALF_FRACTION` | 0.05 (5%) | context_validity.py | Change-factor half point |
| `USAGE_GAIN` | 0.35 | context_validity.py | Usage-weighted decay gain |
| `MIN_HALF_LIFE_SECONDS` | 600 (10min) | context_validity.py | Effective half-life floor |
| `DEFAULT_THRESHOLD` | 0.70 | context_validity.py | Context-vs-live routing threshold |
| `_STALE_THRESHOLD` | 0.50 | dlm.py | Auto-rebuild trigger |
| `_REBUILD_COOLDOWN_SECONDS` | 300 (5min) | dlm.py | Min gap between rebuild attempts |
| `_SWEEP_INTERVAL_SECONDS` | 1,800 (30min) | dlm.py | Background freshness sweep interval |

---

## 13. Conclusion

DLM curation is a bounded precomputation strategy that trades ~25 build-time
scans for the elimination of per-request scans on the common case. The four tiers
— grand totals, per-dimension breakdowns, two-dimensional combos, and HLL sketch
cuboids — cover progressively complex question shapes while staying within strict
cell-product and pair-count budgets. Dashboard-level curation extends coverage to
N-dimensional filter × group-by patterns driven by actual dashboard definitions.
Incremental refresh keeps additive metrics current without a full rebuild.

The result: 375 precomputed answers for a 10.1-million-row dataset, serving 67
of 71 dashboard charts with zero database queries, on $40/month of
infrastructure. The 4 misses are scatter plots that don't aggregate — a design
boundary, not a bug.

---

*See also: `whitepaper-dlm.md` for the full DLM engine (compilation, intent
resolution, freshness), `whitepaper-adaptive-context-routing.md` for the
per-element staleness routing method, and `patent-adaptive-context-routing.md`
for the 21-claim patent draft.*
