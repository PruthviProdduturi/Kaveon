# NL→SQL: How Natural Language Queries Work

Kaveon's homepage lets you ask questions in plain English and get back charts, with **no LLM dependency**. Two engines sit behind it:

1. **DLM (Data Language Model) — the primary path.** A per-dataset compiled context artifact in the API (`apps/kaveon-api/services/dlm.py`). It resolves the question deterministically and, for the common cases, **answers from precomputed context with no database scan at all** — returning a result badged **"⚡ From context · no DB scan"**. Only novel slices fall through to a single live query, badged **"Live query · Xs"**.
2. **The in-browser template parser — the fallback.** A template-based keyword parser (`apps/kaveon-web/utils/nlToSql.ts`) that runs entirely in the browser. It handles shapes the DLM can't yet build (mainly time-series trends) and is documented in detail below.

Both are deterministic — same question, same answer — and neither calls a hosted model. See `docs/dlm-positioning.md` and `docs/whitepaper-adaptive-context-routing.md` for the DLM design.

---

## How It Works End to End

```
User types question
       │
       ▼
DLM route (POST /api/v1/dlm/ask)   ── PRIMARY
  Route → which dataset(s)
  Resolve NL terms → columns/values (value index)
  Match a precomputed answer shape?
       ├─ yes → ANSWER FROM CONTEXT   (in-memory dict hit, no DB scan)  ⚡
       └─ no  → assemble ONE live query → execute → cache
       │
       ▼  (only if DLM can't build the shape — e.g. trends)
Template fallback
  Dataset auto-detection: score each loaded schema against the query
  nlToSql(query, schema): 7 patterns, fuzzy resolution, SQL string
  POST /api/v1/lab/query → execute
       │
       ▼
InlineChart renders ECharts in the conversation
  Route badge (context vs live) + timing shown on every answer
  Chart type picked automatically based on result shape
```

The rest of this guide documents the **template fallback** in detail (patterns, fuzzy matching, chart selection). For the DLM primary path — the value index, precomputed answers, and answer-from-context routing — see the whitepaper and positioning doc referenced above.

---

## Dataset Auto-Detection

On page load the homepage fetches schemas for every dataset the user has access to. When a question is submitted, each schema is scored:

```
score += 3   if dataset name words appear in the query
score += 1   for each column name that appears in the query
score += 2   for each metric name that appears in the query
```

The schema with the highest score wins. If two schemas tie, the first one wins. The selected dataset is shown in the UI so the user can override it manually.

---

## Pattern Matching

The parser tries patterns in priority order and returns on the first match.

### Pattern 1 — Aggregate only

Triggers on: `total`, `sum`, `count`, `average`, `avg`, `mean`, `min`, `max` with no grouping keywords.

```
"total revenue"
"what is the average order amount?"
"count of customers"
```

Result: `SELECT SUM(revenue) FROM orders LIMIT 1`, chart type `kpi`.

### Pattern 2 — Top N

Triggers on: `top <number> <group> by <metric>`

```
"top 10 countries by total deaths"
"top 5 products by revenue"
```

Result: `SELECT country, SUM(deaths) FROM ... GROUP BY country ORDER BY SUM(deaths) DESC LIMIT 10`, chart type `bar`.

### Pattern 3 — Trend over time

Triggers on: `over time`, `trend`, `by month`, `by year`, `by week`, `by day`, `monthly`, `yearly`, `weekly`, `daily`.

```
"show revenue over time"
"trend of new cases monthly"
```

The engine finds the first `date`-typed column in the schema. The metric is extracted from remaining tokens; if none match, the first defined metric is used as a fallback.

Result: `SELECT date_col, SUM(metric) FROM ... GROUP BY date_col ORDER BY date_col LIMIT 1000`, chart type `line`.

### Pattern 4 — Grouped by dimension

Triggers on: `by`, `per`, `for each`, `grouped by`, `group by`

```
"revenue by region"
"orders per category"
"show sales for each product"
```

Result: `SELECT region, SUM(revenue) FROM ... GROUP BY region ORDER BY SUM(revenue) DESC LIMIT 1000`.

Chart type selection: if the group column is a `date`, renders as `line`; otherwise `bar`.

### Pattern 5 — Compare X vs Y

Triggers on: `compare ... vs ...`, `compare ... versus ...`, `compare ... and ...`, `compare ... against ...`

```
"compare North vs South by sales"
"compare 'Widget A' versus 'Widget B' over time"
```

Extracts the two literal values, finds a string column to filter on, and a metric. If a date column exists the chart is a multi-series line; otherwise a grouped bar.

### Pattern 6 — Distribution / breakdown

Triggers on: `distribution`, `breakdown`, `spread`

```
"distribution of order status"
"breakdown of regions"
```

Result: `SELECT col, COUNT(*) as count FROM ... GROUP BY col ORDER BY count DESC LIMIT 1000`.

Chart type: `pie` when the column has fewer than 8 distinct values (estimated); `bar` otherwise.

### Pattern 7 — Fallback word scan

No pattern matched. The engine scans all words against columns and metrics and builds the best available query:

- Found group column + metric → grouped query (confidence 0.5)
- Found metric only → single-value KPI
- Found group column only → COUNT by that column, chart type `table`
- Nothing matched → returns `null` (no chart rendered, assistant asks for clarification)

---

## Fuzzy Column Matching

`findColumn(token, columns, typeFilter?)` resolves a query token to a schema column using this cascade:

1. **Direct name match** — `normalize()` strips underscores, hyphens, and lowercases. Substring and prefix/suffix matches (min 3 chars) are accepted.
2. **Description match** — matches the optional `description` field on the column.
3. **Forward alias** — looks up the token in `COLUMN_ALIASES` and tries all alias values against column names.
4. **Reverse alias** — checks if the token matches any alias value and infers the canonical column name.

Built-in aliases:

| Canonical | Aliases |
|-----------|---------|
| `sales` | sale, revenue, amount, order_amount |
| `revenue` | rev, total_revenue, revenue_amount, sales |
| `profit` | margin, net_profit, gross_profit |
| `quantity` | qty, units, count, volume |
| `price` | unit_price, cost, rate, amount |
| `date` | created_at, order_date, timestamp, created, updated_at |
| `name` | title, label, description |
| `category` | type, group, segment, class |
| `region` | area, territory, location, country, state, city |
| `customer` | client, account, buyer, user |

Metrics go through the same cascade via `findMetric()`.

---

## Chart Type Selection

After SQL is generated, `pickChartType()` determines the visualization:

| Condition | Chart type |
|-----------|------------|
| Pattern is `kpi` or result is a single value | `kpi` |
| X-axis column is a `date` | `line` |
| Pattern is `distribution`, fewer than 8 groups | `pie` |
| Pattern is `distribution`, 8+ groups | `bar` |
| Pattern is `table` or fallback with only group column | `table` |
| Everything else | `bar` |

---

## Result Rendering

The SQL result flows to `InlineChart` (`components/chat/InlineChart.tsx`), which renders inside the chat message:

- **`kpi`** — large centered number with metric label
- **`bar`** — ECharts bar chart, 280 px tall, brand color palette
- **`line`** — ECharts line chart with smooth curves and 12% opacity area fill
- **`pie`** — ECharts donut (35%–65% radius)
- **`table`** — scrollable HTML table, max 20 rows displayed

Every chart shows an expandable SQL footer with a link to open the query in SQL Lab.

---

## Confidence Scores

Each result carries a `confidence` value (0–1):

| Situation | Score |
|-----------|-------|
| Exact metric match via defined metric expression | 1.0 |
| Top N with all tokens resolved | 1.0 |
| Grouped query with both metric and column resolved | 1.0 |
| Distribution with column resolved | 1.0 |
| Trend with metric (no date fallback) | 1.0 |
| Trend with numeric column only (no metric) | 0.7 |
| Compare pattern | 0.8 |
| Aggregate with numeric column (no metric) | 0.9 |
| Fallback scan with metric + group column | 0.5 |
| Fallback scan, metric or group only | 0.5 |
| Fallback, group column only | 0.3 |

Confidence is currently informational; it is logged to the console but not surfaced to the user.

---

## Intelligent Responses

The assistant message shown alongside the chart is generated from the result data:

- Shows the top 3 rows as text (e.g. "Top result: United States — 1,234,567")
- Appends a suggestion prompt to continue the conversation
- If the SQL returns no rows, responds with "No data found for that query"
- If `nlToSql` returns `null`, responds asking the user to rephrase

---

## Code Locations

| File | Role |
|------|------|
| `apps/kaveon-web/utils/nlToSql.ts` | Core parser: all patterns, fuzzy match, SQL builder |
| `apps/kaveon-web/components/chat/InlineChart.tsx` | Chat-embedded chart renderer |
| `apps/kaveon-web/app/page.tsx` | Homepage: dataset auto-detection, message loop, schema loading |

---

## How to Extend

### Add a new query pattern

Add a new regex or keyword check inside `nlToSql()` before the fallback (Pattern 7). Return an `NlToSqlResult` with `sql`, `chartType`, `xAxis`, `yAxis`, `title`, and `confidence`.

### Add column aliases

Extend the `COLUMN_ALIASES` record at the top of `nlToSql.ts`:

```typescript
const COLUMN_ALIASES: Record<string, string[]> = {
  // existing entries...
  spend: ["cost", "expenditure", "budget", "ad_spend"],
};
```

### Add a new aggregate keyword

Extend `AGGREGATE_ALIASES`:

```typescript
const AGGREGATE_ALIASES: Record<string, string> = {
  // existing entries...
  median: "PERCENTILE_CONT(0.5) WITHIN GROUP (ORDER BY",
};
```

### Support a new chart type in inline chat

Extend the `ChartType` union in `nlToSql.ts` and add a new render branch in `InlineChart.tsx`.
