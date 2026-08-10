# Template-Based Natural Language to SQL: A Deterministic Approach to Conversational Data Querying

**Pruthvi Prodduturi**
August 2026

---

## Abstract

This paper presents a template-based natural language to SQL (NL-to-SQL) engine built for the Kaveon data platform. Rather than relying on large language models, the system uses deterministic keyword pattern matching, fuzzy column resolution, and schema-aware scoring to translate natural language questions into executable SQL. The engine processes queries in under 1ms, requires no API keys or network calls, works fully offline, and produces identical output for identical input. On the class of analytical questions that constitute the majority of real-world data exploration -- aggregations, groupings, rankings, trends, and comparisons -- the system achieves reliable accuracy with zero inference cost.

---

## 1. Introduction

The promise of "talk to your data" has been a product vision across the analytics industry for years. The dominant approach today routes user questions through a large language model (LLM) -- typically GPT-4, Claude, or a fine-tuned text-to-SQL model -- which generates SQL from the natural language input. This works, but it comes with significant trade-offs:

- **Cost.** Every query costs $0.01-0.10 in API usage. At scale, this adds up fast.
- **Latency.** LLM round-trips take 2-5 seconds. Users notice.
- **Non-determinism.** The same question asked twice may produce different SQL. This makes debugging, testing, and trust difficult.
- **Hallucination.** LLMs generate syntactically valid SQL that references columns or tables that don't exist.
- **Dependency.** Requires network connectivity, API keys, and a third-party service that can change pricing or behavior at any time.
- **Privacy.** Schema metadata and user queries are sent to external servers.

The key insight behind Kaveon's approach is that the vast majority of analytical questions follow a small number of predictable patterns. "Total revenue." "Sales by region." "Top 10 customers by spend." "Revenue trend over time." "Compare US vs UK." These aren't open-ended natural language understanding problems -- they're pattern matching problems.

Kaveon's NL-to-SQL engine handles this 80% case with a deterministic, template-based parser that runs entirely client-side. No model. No API key. No network call. For the remaining 20% -- complex joins, multi-step reasoning, ambiguous references -- the architecture supports an optional LLM fallback that can be added without changing the core pipeline.

---

## 2. Architecture

The system is structured as a linear pipeline with seven stages:

```
User Query
    |
    v
[1. Tokenization & Normalization]
    |
    v
[2. Pattern Detection]
    |
    v
[3. Fuzzy Column/Metric Resolution]
    |
    v
[4. Schema Binding & SQL Generation]
    |
    v
[5. Multi-Dataset Auto-Detection]
    |
    v
[6. Query Execution]
    |
    v
[7. Chart Selection + Insight Generation]
    |
    v
Response (chart + narrative + SQL)
```

Each stage is a pure function (or close to it). The pipeline is synchronous through stage 5, with stage 6 being the only async operation (database round-trip). The entire parse-and-generate path completes in sub-millisecond time on modern hardware.

The core design decision: **parse the query, don't understand it.** The system doesn't build a semantic representation of the user's intent. It matches surface-level patterns against known templates and resolves ambiguous references using the dataset schema as a constraint. This is deliberately less powerful than semantic parsing, but it's predictable, testable, and fast.

---

## 3. Pattern Matching Engine

The engine recognizes seven query patterns, evaluated in priority order. The first match wins.

### Pattern 1: Aggregate-Only

Matches queries requesting a single aggregate value with no grouping dimension.

```
Input:  "total revenue"
Regex:  /^(?:what(?:'s| is) (?:the )?)?(?:show |get )?(total|sum|count|average|avg|mean|min|max|minimum|maximum)\s+(?:of\s+)?(.+?)$/i
Guard:  No grouping keywords (by, per, for each, over time, trend)
Output: SELECT SUM(revenue) FROM sales.orders LIMIT 1
Chart:  KPI (big number)
```

The aggregate function keyword maps through a lookup table:

```typescript
const AGGREGATE_ALIASES: Record<string, string> = {
  total: "SUM", sum: "SUM", count: "COUNT",
  average: "AVG", avg: "AVG", mean: "AVG",
  min: "MIN", minimum: "MIN", max: "MAX", maximum: "MAX",
};
```

Confidence: 1.0 when the target resolves to a defined metric, 0.9 when it resolves to a numeric column.

### Pattern 2: Top-N

Matches queries requesting ranked results.

```
Input:  "top 10 customers by revenue"
Regex:  /top\s+\d+\s+(.+?)\s+by\s+(.+?)$/i
Output: SELECT customer, SUM(revenue) FROM sales.orders
        GROUP BY customer ORDER BY SUM(revenue) DESC LIMIT 10
Chart:  Bar
```

The N value is extracted via `/\btop\s+(\d+)/i`. The group column is resolved as a string-type column; the metric is resolved as either a defined metric or a numeric column wrapped in `SUM()`.

### Pattern 3: Trend / Over Time

Matches temporal queries.

```
Input:  "revenue over time"
Regex:  /\b(over time|trend|over the|by month|by year|by week|by day|monthly|yearly|weekly|daily)\b/
Output: SELECT order_date, SUM(revenue) FROM sales.orders
        GROUP BY order_date ORDER BY order_date LIMIT 1000
Chart:  Line (always, when date column is on x-axis)
```

The date column is found by type (`col.type === "date"`). If no specific metric is mentioned, the system falls back to the first defined metric in the schema.

### Pattern 4: Compare X vs Y

Matches explicit comparison queries.

```
Input:  "compare US vs UK"
Regex:  /compare\s+(.+?)\s+(?:vs|versus|and|against)\s+(.+?)(?:\s+by|\s+over|\s*$)/i
Output: SELECT order_date, country, SUM(revenue) FROM sales.orders
        WHERE country IN ('US', 'UK')
        GROUP BY order_date, country ORDER BY order_date LIMIT 1000
Chart:  Line (if date column exists), Bar (otherwise)
```

The comparison values are extracted and injected into a `WHERE ... IN (...)` clause. If a date column exists, the query groups by both the date and the comparison column, enabling a multi-series line chart.

### Pattern 5: Distribution

Matches requests for categorical breakdowns.

```
Input:  "distribution of category"
Regex:  /\b(distribution|breakdown|spread)\b/
Output: SELECT category, COUNT(*) as count FROM products
        GROUP BY category ORDER BY count DESC LIMIT 1000
Chart:  Pie (< 8 categories), Bar (>= 8 categories)
```

### Pattern 6: Group-By

The general-purpose grouped aggregation. Catches the common "[metric] by [dimension]" pattern.

```
Input:  "sales by region"
Regex:  /(.+?)\s+(?:by|per|for each|grouped by|group by)\s+(.+?)$/i
Output: SELECT region, SUM(sales) FROM sales.orders
        GROUP BY region ORDER BY SUM(sales) DESC LIMIT 1000
Chart:  Bar (categorical), Line (date dimension)
```

The ordering is descending by metric for categorical dimensions, ascending by column value for date dimensions. If a metric match fails but the group column resolves, the system falls back to `COUNT(*)`.

### Pattern 7: Fallback

When no explicit pattern matches, the engine scans every word in the query against all columns and metrics. It also tries two-word combinations for multi-word metric names. Results are assembled into the best possible query based on what was found:

- Found a group column + metric: grouped query (confidence 0.5)
- Found only a metric: KPI query (confidence 0.5)
- Found only a group column: `COUNT(*)` grouped query (confidence 0.3)
- Found nothing: return `null` (no match)

### Pattern Priority

Patterns are evaluated in a fixed order: aggregate-only, top-N, trend, compare, distribution, group-by, fallback. The first successful match short-circuits evaluation. This ordering is intentional: more specific patterns (aggregate-only requires no grouping keywords) are checked before general patterns (group-by catches almost anything with "by" in it).

---

## 4. Fuzzy Column Matching

Real users don't type exact column names. They type "confirmed cases" when the column is `confirmed`. They type "sales" when the column is `revenue`. The engine handles this through a multi-layer resolution strategy.

### Layer 1: Normalized String Matching

All comparisons operate on normalized strings (lowercased, underscores/hyphens replaced with spaces). The `fuzzyMatch` function checks:

- Exact match
- Substring containment (either direction)
- Prefix/suffix match (minimum 3 characters)
- Word-level overlap: each word in the needle is checked against each word in the haystack

```typescript
// "confirmed cases" matches column "confirmed"
// because word "confirmed" (length >= 3) is found in haystack
fuzzyMatch("confirmed cases", "confirmed") // true
```

### Layer 2: Alias Expansion

A static alias table maps common business terms to their variants:

```typescript
const COLUMN_ALIASES: Record<string, string[]> = {
  sales: ["sale", "revenue", "amount", "order_amount"],
  revenue: ["rev", "total_revenue", "revenue_amount", "sales"],
  date: ["created_at", "order_date", "timestamp", "created", "updated_at"],
  region: ["area", "territory", "location", "country", "state", "city"],
  // ...
};
```

Alias resolution is bidirectional. If the user types "sales," the system checks the column list against `["sale", "revenue", "amount", "order_amount"]`. If the user types "revenue," the system also finds columns that match any alias list containing "revenue."

### Layer 3: Description Matching

Columns with a `description` field in the schema are matched against the user's terms using the same fuzzy matching logic. This allows dataset authors to annotate columns with human-readable descriptions that improve match quality.

### Layer 4: Metric Resolution

Metrics (pre-defined aggregate expressions like `SUM(revenue)`) are resolved through the same fuzzy + alias pipeline. When a metric matches, its `expression` field is used directly in the generated SQL, preserving any custom aggregation logic defined by the dataset author.

### Confidence Scoring

Each resolution path produces an implicit confidence level:

| Resolution path          | Confidence |
|--------------------------|------------|
| Direct metric match      | 1.0        |
| Direct column match      | 0.9        |
| Alias-expanded match     | 0.8        |
| Description match        | 0.7        |
| Fallback word scan       | 0.3 - 0.5 |

The confidence score propagates through the pipeline and is used in multi-dataset auto-detection (Section 5) and in the final response to determine how assertive the system should be.

---

## 5. Multi-Dataset Auto-Detection

A typical Kaveon workspace contains multiple datasets -- potentially dozens -- connected to different databases. When a user types "show cases by country," the system needs to determine which dataset to query without requiring a manual selection.

### Scoring Algorithm

The `findBestSchema` function iterates over all cached dataset schemas and computes a composite score:

```
score = (dataset_name_overlap * 0.3)
      + (column_name_overlap  * 0.2 per match)
      + (metric_name_overlap  * 0.2 per match)
      + (parser_confidence    * 1.0 if nlToSql succeeds)
```

Each overlap check splits names into words (minimum 3 characters), normalizes them, and checks for bidirectional substring containment against the query words.

The parser confidence is the heaviest signal. If `nlToSql()` returns a result with confidence 1.0 for a given schema, that schema almost certainly wins. The name and column overlap scores act as tiebreakers and as a safety net when the parser produces low-confidence results.

### Schema Caching

All dataset schemas are fetched on page load and cached in a React ref (`allSchemasRef`). This provides two benefits:

1. **Instant access.** The scoring function reads from the ref synchronously -- no async calls during query processing.
2. **Closure stability.** The ref avoids stale closure issues that would occur if the scoring function captured a state variable.

### Fallback Behavior

When no dataset scores above a minimum threshold but at least one dataset matched by name (confidence >= 0.3), the system generates a basic `SELECT` query showing a sample of the data. When no dataset matches at all, the system lists all available datasets and suggests reformulated queries.

---

## 6. Chart Type Selection

After query execution, the system selects a visualization type using deterministic heuristics:

```typescript
function pickChartType(pattern: string, xCol: DatasetColumn | null, groupCount?: number): ChartType {
  if (pattern === "kpi") return "kpi";
  if (pattern === "distribution") {
    return groupCount !== undefined && groupCount < 8 ? "pie" : "bar";
  }
  if (xCol?.type === "date") return "line";
  if (pattern === "table") return "table";
  return "bar";
}
```

The rules, in priority order:

| Condition | Chart type | Rationale |
|-----------|------------|-----------|
| Single aggregate value | KPI (big number) | No axes needed |
| Date column on x-axis | Line chart | Temporal data implies continuity |
| Distribution with < 8 categories | Pie chart | Readable part-to-whole |
| Distribution with >= 8 categories | Bar chart | Pie becomes illegible beyond ~7 slices |
| Categorical + numeric | Bar chart | Standard comparison visualization |
| Fallback | Table | Show raw data when visualization is ambiguous |

These heuristics are opinionated by design. An LLM-based chart selector might pick a more creative visualization, but it might also pick an inappropriate one. The deterministic rules guarantee that the chosen chart type is always reasonable, even if it's not always optimal.

The `InlineChart` component renders the selected chart type using ECharts, with theme-aware styling via CSS variables. KPI results render as a large formatted number with a label. Tables render with a 20-row preview. All chart types include a collapsible SQL footer and a link to open the query in SQL Lab for further exploration.

---

## 7. Intelligent Response Generation

The system generates a natural language narrative alongside every chart. This is not LLM-generated text -- it's template-based analysis of the result set.

### Formatting

Large numbers are formatted with K/M/B suffixes:

```typescript
const fmt = (v: number): string => {
  if (Math.abs(v) >= 1_000_000_000) return (v / 1_000_000_000).toFixed(1) + "B";
  if (Math.abs(v) >= 1_000_000) return (v / 1_000_000).toFixed(1) + "M";
  if (Math.abs(v) >= 1_000) return (v / 1_000).toFixed(1) + "K";
  return v.toLocaleString();
};
```

### Response Templates

**KPI results:** "The total **revenue** is **4.2M**."

**Grouped results:** The system sorts rows by the metric column descending, extracts the top 3 entries, and reports:

> Found **47** results for revenue by region. Top 3: **North America** (1.2B), **Europe** (890.4M), **Asia Pacific** (723.1M).
>
> Want me to show just the **top 10** or filter by a specific value?

The follow-up suggestion is generated when the result set exceeds 10 rows, guiding the user toward a more focused query.

**Empty results:** When the query executes successfully but returns no rows, the system displays the generated SQL so the user can debug the query directly.

**No match:** When the parser cannot match the query to any dataset, the system lists all available datasets and provides example query patterns. This teaches the user the system's capabilities without requiring documentation.

---

## 8. Limitations and Future Work

The template-based approach has clear boundaries.

**No joins.** The engine operates on single tables (or views). Queries requiring joins across tables are not supported. In practice, this is mitigated by Kaveon's dataset abstraction -- a dataset can be backed by a view that pre-joins multiple tables.

**No multi-step reasoning.** "Revenue growth rate" requires computing revenue for two time periods and calculating the delta. The current engine can't decompose this into sub-queries. Each query is independent.

**No contextual reference resolution.** "Show me that by month instead" requires understanding what "that" refers to from the conversation history. The current system treats each query independently.

**No complex predicates.** "Revenue where region is North America and year is 2024" would require parsing arbitrary WHERE clause conditions. The engine only supports predicates in the compare pattern (`WHERE column IN (...)`).

**Limited aggregation.** The engine supports SUM, COUNT, AVG, MIN, and MAX. Window functions, HAVING clauses, and nested aggregations are out of scope.

### Planned improvements:

- **Conversation context.** Carry forward the previous query's schema bindings so "now show that by month" resolves correctly.
- **User correction learning.** When a user modifies the generated SQL in SQL Lab and re-runs it, capture the correction as a training signal for pattern refinement.
- **Hybrid LLM fallback.** When the template parser returns `null` or a confidence below 0.3, optionally route to an LLM with the schema as context. The LLM handles the 20% case; the template engine handles the 80% case at zero cost.
- **Additional patterns.** Year-over-year comparison, percentile queries, conditional aggregation.
- **Staleness-scored execution.** Once a question is translated, it still hits the database every time. The companion work *Adaptive Context-Based Query Routing Using Data Staleness Scoring* (`whitepaper-adaptive-context-routing.md`) sits behind this translation layer and routes each translated question to an instant context-based answer or a live query, based on a per-element validity score measured from the database's own change counters — so the system re-queries only when, and only where, the data has actually moved.

---

## 9. Comparison with LLM-Based Approaches

| Dimension | Kaveon NL-to-SQL | GPT-4 / Claude (text-to-SQL) | Fine-tuned text2sql models |
|---|---|---|---|
| **Latency** | < 1ms (client-side) | 2-5s (API round-trip) | 200-500ms (inference) |
| **Cost per query** | $0 | $0.01-0.10 | $0.001-0.01 (GPU hosting) |
| **Deterministic** | Yes -- identical input always produces identical output | No -- temperature > 0, prompt sensitivity | Mostly -- but model updates change behavior |
| **Offline capable** | Yes | No | Yes (if self-hosted) |
| **Hallucination risk** | None -- only generates SQL using columns that exist in the schema | Moderate -- can reference non-existent columns or tables | Low-moderate -- constrained by training data |
| **Simple queries** (aggregations, groupings, top-N) | High accuracy | High accuracy | High accuracy |
| **Complex queries** (joins, subqueries, window functions) | Not supported | High accuracy | Moderate accuracy |
| **Ambiguity resolution** | Limited -- alias table + fuzzy matching | Strong -- semantic understanding | Moderate -- learned patterns |
| **Multi-step reasoning** | Not supported | Supported (with chain-of-thought) | Limited |
| **Privacy** | Full -- no data leaves the browser | Schema and queries sent to third-party API | Depends on hosting |
| **Setup required** | None | API key, billing, prompt engineering | Training data, GPU infrastructure, MLOps |
| **Dependency risk** | None | API deprecation, pricing changes, rate limits | Model drift, retraining costs |

The comparison reveals a clear trade-off surface. LLMs are strictly more capable on complex queries, ambiguity, and multi-step reasoning. But for the predictable 80% of data exploration queries, the template engine matches LLM accuracy while being orders of magnitude faster, completely free, deterministic, and private.

The pragmatic position is not that template-based parsing replaces LLMs. It's that template-based parsing should be the **first layer** -- the fast, free, deterministic path that handles the common case. An LLM fallback can sit behind it for queries the template engine can't handle, invoked only when needed.

---

## 10. Implementation

The complete NL-to-SQL engine is implemented in three files:

| File | Lines | Responsibility |
|---|---|---|
| `utils/nlToSql.ts` | ~475 | Pattern matching, fuzzy resolution, SQL generation, chart type selection |
| `components/chat/InlineChart.tsx` | ~320 | Chart rendering (ECharts), KPI display, table display, SQL footer |
| `app/page.tsx` | ~600 | Multi-dataset auto-detection, schema caching, insight generation, chat UI |

Total: approximately 1,400 lines of TypeScript across all three files. The core parsing logic in `nlToSql.ts` has zero external dependencies -- no NLP libraries, no ML models, no training data. It's pure TypeScript operating on strings and arrays.

### Core API

The primary function signature:

```typescript
export function nlToSql(
  query: string,
  schema: DatasetSchema
): NlToSqlResult | null;
```

Input: a natural language string and a dataset schema (table name, typed columns, pre-defined metrics). Output: a SQL string, chart type recommendation, axis bindings, title, and confidence score -- or `null` if no pattern matched.

The `DatasetSchema` interface provides the constraint surface:

```typescript
export interface DatasetSchema {
  tableName: string;        // e.g. "sales.orders"
  columns: DatasetColumn[]; // name, type, description
  metrics: DatasetMetric[]; // name, expression, description
}
```

By requiring the schema upfront, the engine guarantees that every column reference in the generated SQL exists in the target table. This is the fundamental mechanism that eliminates hallucination: the system cannot reference what it doesn't know about.

---

## Conclusion

Template-based NL-to-SQL is a viable, production-ready approach for conversational data querying. The Kaveon implementation demonstrates that seven pattern templates, a fuzzy column resolver, and a schema-aware scoring algorithm can handle the majority of analytical questions users actually ask -- instantly, deterministically, and at zero marginal cost.

The approach is not a replacement for LLM-based text-to-SQL. It is a complement. The template engine handles the fast path; an LLM handles the long tail. Together, they provide a system that is responsive by default and capable when needed.

"Talk to your data" doesn't require a $20/month API key. For most questions, it requires about 475 lines of TypeScript and a schema.
