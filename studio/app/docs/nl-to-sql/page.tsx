import { PageHeader, Callout, Code, Diagram, Pager } from "../../../components/docs/prose";

export const metadata = { title: "DLM · NL→SQL" };

export default function NlToSqlDocs() {
  return (
    <div className="docs-prose">
      <PageHeader
        eyebrow="Features"
        title="DLM · NL→SQL"
        lead="The home page turns plain-English questions into charts with no hosted LLM. The primary engine is the DLM — a compiled per-dataset context artifact that answers the common questions from precomputed context with no database scan. A browser-side template parser is the fallback. Both are deterministic, fast, and inspectable."
      />

      <Callout type="note">
        There is no hosted model call in either deterministic path, so there is no model-inference charge. Resolution is reproducible for the same request and context version; source results may change as data changes. The separate <a href="/docs/sql-lab">SQL Lab inline AI</a> is where LLM providers (Claude,
        GPT-4o, GitHub Models) come in for open-ended generation.
      </Callout>

      <Diagram src="/docs/architecture/kaveon-intelligence-loop.svg" alt="Natural-language intelligence and analytical compute paths in Kaveon" caption="The current DLM path resolves against registered SQL sources. Routing its compiled SQL into Kaveon Engine is target architecture, not a shipping integration." />

      <h2>The DLM — primary path</h2>
      <p>
        The primary engine is the <strong>DLM (Data Language Model)</strong>: a per-dataset compiled context artifact in
        the API (<code>api/dlm/</code>). It routes the question to a dataset, resolves the natural-language terms
        to columns and values, and — for the common shapes — returns a{" "}
        <strong>precomputed answer with no database scan at all</strong>. Only novel slices touch the warehouse.
      </p>
      <Code lang="text">{`Question  →  POST /api/v1/dlm/ask
   route → which dataset · resolve terms → columns/values (value index)
        │
        ├─ precomputed shape?  →  ⚡ From context · no DB scan   (in-memory dict hit)
        │      totals · by-dimension · single-dimension filter
        │
        └─ novel slice/combo   →  Live query · Xs   (assemble ONE warehouse query → cache)`}</Code>
      <p>
        Every answer is badged honestly — <strong>⚡ From context · no DB scan</strong> or{" "}
        <strong>Live query · Xs</strong> — with the real timing. Because the <code>kaveonmeta</code> context plane is
        physically separate from the <code>kaveon</code> warehouse, context answers never wait behind a big scan. See{" "}
        <a href="/docs/architecture">Architecture</a> for the plane split.
      </p>

      <h2>The fallback — template parser</h2>
      <p>
        For shapes the DLM does not yet build (mainly time-series trends), the home page falls back to a template-based
        keyword parser that runs entirely in the browser (<code>utils/nlToSql.ts</code>). The rest of this page
        documents that fallback engine.
      </p>
      <Code lang="text">{`Fallback:  User question
      │
      ▼
Dataset auto-detection      → score every loaded schema, pick the best
      │
      ▼
nlToSql(query, schema)      → 7 ordered patterns, fuzzy column/metric resolution
      │
      ▼
POST /api/v1/sql/execute    → kaveon-api runs the SQL against the source
      │
      ▼
InlineChart renders ECharts → chart type chosen from the result shape`}</Code>

      <h2>Dataset auto-detection</h2>
      <p>
        On load, the home page fetches schemas for every dataset you can access. When you submit a question, each
        schema is scored against the text and the highest score wins (ties go to the first). The chosen dataset is
        shown in the UI so you can override it.
      </p>
      <Code lang="text">{`score += 0.3   per dataset-name word that appears in the query
score += 0.2   per column name that appears
score += 0.2   per metric name that appears
score += confidence   from the NL→SQL parser for this schema`}</Code>

      <h2>Pattern matching</h2>
      <p>The parser tries seven patterns in this priority order and returns on the first match.</p>

      <h3>1 · Aggregate only</h3>
      <p>Triggers on <code>total, sum, count, average, avg, mean, min, max</code> with no grouping words.</p>
      <Code lang="text">{`"total revenue"  ·  "average order amount"  ·  "count of customers"
→  SELECT SUM(revenue) FROM orders LIMIT 1        (chart: kpi)`}</Code>

      <h3>2 · Top N</h3>
      <p>Triggers on <code>top &lt;number&gt; &lt;group&gt; by &lt;metric&gt;</code>.</p>
      <Code lang="text">{`"top 10 countries by total deaths"
→  SELECT country, SUM(deaths) FROM ... GROUP BY country
   ORDER BY SUM(deaths) DESC LIMIT 10               (chart: bar)`}</Code>

      <h3>3 · Trend over time</h3>
      <p>Triggers on <code>over time, trend, by month/year/week/day, monthly, yearly…</code></p>
      <p>
        It finds the first <code>date</code>-typed column in the schema; the metric comes from the remaining tokens, or
        falls back to the first defined metric.
      </p>
      <Code lang="text">{`"show revenue over time"  ·  "trend of new cases monthly"
→  SELECT date_col, SUM(metric) FROM ... GROUP BY date_col
   ORDER BY date_col LIMIT 1000                     (chart: line)`}</Code>

      <h3>4 · Compare X vs Y</h3>
      <p>Triggers on <code>compare … vs / versus / and / against …</code></p>
      <p>
        Extracts the two literal values, finds a string column to filter on and a metric. With a date column it renders
        a multi-series line; otherwise a grouped bar.
      </p>

      <h3>5 · Distribution / breakdown</h3>
      <p>Triggers on <code>distribution, breakdown, spread</code>.</p>
      <Code lang="text">{`"distribution of order status"  ·  "breakdown of regions"
→  SELECT col, COUNT(*) AS count FROM ... GROUP BY col
   ORDER BY count DESC LIMIT 1000`}</Code>
      <p>
        Chart type: always <code>bar</code>. (<code>pickChartType</code> supports a
        &ldquo;pie under 8 distinct values&rdquo; rule, but the distribution pattern doesn&rsquo;t pass a group count,
        so that branch is currently inactive.)
      </p>

      <h3>6 · Grouped by dimension</h3>
      <p>Triggers on <code>by, per, for each, grouped by, group by</code>.</p>
      <Code lang="text">{`"revenue by region"  ·  "orders per category"
→  SELECT region, SUM(revenue) FROM ... GROUP BY region
   ORDER BY SUM(revenue) DESC LIMIT 1000`}</Code>
      <p>Chart type: <code>line</code> if the group column is a date, otherwise <code>bar</code>.</p>

      <h3>7 · Fallback word scan</h3>
      <p>When no pattern matches, the engine scans every word against columns and metrics and builds the best it can:</p>
      <ul>
        <li>group column + metric → grouped query (confidence 0.5)</li>
        <li>metric only → single-value KPI</li>
        <li>group column only → <code>COUNT</code> by that column, chart type <code>table</code></li>
        <li>nothing matched → returns <code>null</code>; the assistant asks you to rephrase</li>
      </ul>

      <h2>Fuzzy column matching</h2>
      <p>
        Every query token is resolved to a real schema column by <code>findColumn(token, columns, typeFilter?)</code>,
        which walks a cascade — exact match, then case-insensitive, then singular/plural and underscore/space variants,
        then substring — optionally constrained to a type (e.g. only <code>date</code> columns for trend queries). The
        same idea resolves metrics, so &ldquo;sales&rdquo; can map to a <code>total_sales</code> metric.
      </p>

      <h2>Automatic chart selection</h2>
      <p>
        The chart type is inferred from the query shape and result, not chosen by hand: <code>kpi</code> for single
        values, <code>line</code> for time series, <code>bar</code> for grouped comparisons and distributions, and
        <code>table</code> as a safe fallback. <code>InlineChart</code> renders it with ECharts directly in the
        conversation.
      </p>

      <Pager
        prev={{ href: "/docs/sql-lab", title: "SQL Lab" }}
        next={{ href: "/docs/dlm", title: "Data Language Model" }}
      />
    </div>
  );
}
