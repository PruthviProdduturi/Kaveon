import { PageHeader, Callout, Code, Pager } from "../../../components/docs/prose";

export const metadata = { title: "Data Language Model (DLM)" };

export default function DlmDocs() {
  return (
    <div className="docs-prose">
      <PageHeader
        eyebrow="Core Engine"
        title="Data Language Model (DLM)"
        lead="The DLM is Kaveon's core engine — a self-compiling semantic layer that turns your schema into a deterministic question-answering machine. No training, no fine-tuning, no LLM. Register a dataset and the DLM compiles itself: indexing every column value, mapping synonyms, precomputing metric rollups across every dimension, and building HLL sketches for non-additive metrics. Questions resolve to SQL through deterministic pattern matching, not token prediction."
      />

      <Callout type="note">
        There is no model call in the DLM path. Every answer is deterministic — the same question always produces the
        same SQL and the same result. The DLM is the <strong>primary</strong> NL→SQL engine; the browser-side template
        parser (<a href="/docs/nl-to-sql">NL→SQL</a>) is the fallback for shapes not yet precomputed.
      </Callout>

      <h2>What is a DLM?</h2>
      <p>
        A DLM is a per-dataset <strong>compiled context artifact</strong> stored in the <code>kaveonmeta</code> database.
        It is not a machine learning model — it is a deterministic index of your schema&apos;s metrics, dimensions, column
        values, and precomputed answers. When a user asks a question, the DLM resolves it to a specific metric, optional
        grouping dimension, and optional entity filters — then either serves the answer from precomputed context in
        microseconds, or assembles a single live SQL query.
      </p>

      <h2>The compilation pipeline</h2>
      <p>
        Generating a DLM is a one-time <strong>encode</strong> step triggered via the dataset page or the API. It reads
        your warehouse&apos;s statistics and precomputes answers — it does not train a model. The pipeline has five stages:
      </p>

      <h3>1. Register</h3>
      <p>
        Define your dataset&apos;s fact table, metrics (with SQL expressions like <code>SUM(revenue)</code> or{" "}
        <code>COUNT(DISTINCT user_id)</code>), dimensions (grouping columns like <code>country</code>,{" "}
        <code>platform</code>), and optionally a date column for time-scoped queries. The registration also specifies
        which database connection to use.
      </p>

      <h3>2. Value indexing</h3>
      <p>
        The compiler queries <code>SELECT DISTINCT</code> for every dimension column and builds a <strong>value
        index</strong> — a lookup table mapping every real value (e.g. &ldquo;United States&rdquo;, &ldquo;iOS&rdquo;,
        &ldquo;Enterprise&rdquo;) to its column. This is what lets the DLM resolve entity filters from natural language:
        when a user says &ldquo;in the US&rdquo;, the value index matches &ldquo;US&rdquo; to the <code>country</code>
        column without any fuzzy model inference.
      </p>
      <Code lang="text">{`Value Index (excerpt):
  "United States"    → country
  "US"               → country  (alias)
  "iOS"              → platform
  "Enterprise"       → segment
  "Q4 2024"          → quarter`}</Code>

      <h3>3. Synonym maps</h3>
      <p>
        A built-in synonym dictionary maps common terms to their canonical forms. For example, &ldquo;actions&rdquo; maps
        to &ldquo;event&rdquo;, &ldquo;area&rdquo; maps to &ldquo;region&rdquo;, &ldquo;nation&rdquo; maps to
        &ldquo;country&rdquo;. Users can also define custom aliases through the context editor (curation overrides).
      </p>
      <Code lang="text">{`Synonym Map (built-in):
  "country"  → ["nation", "state", "territory", "land"]
  "event"    → ["action", "activity", "occurrence", "log", "record", "entry"]
  "revenue"  → ["sales", "income", "earnings", "turnover"]
  "user"     → ["customer", "client", "account", "member", "subscriber"]`}</Code>

      <h3>4. Precomputed answers</h3>
      <p>
        For every metric × dimension combination, the compiler runs the aggregation query once and stores the result in
        the <code>dlm_answers</code> table. This includes:
      </p>
      <ul>
        <li><strong>Metric totals</strong> — the overall value for each metric (e.g. &ldquo;Total Actions: 504,291,882&rdquo;)</li>
        <li><strong>Per-dimension breakdowns</strong> — the metric grouped by each dimension (e.g. Total Actions by country, by platform, by segment)</li>
        <li><strong>Filtered slices</strong> — single-dimension-filter answers (e.g. Total Actions where country = &lsquo;US&rsquo;)</li>
      </ul>
      <p>
        At runtime, these answers are loaded into the in-memory DLM context (<code>_ANSWER_CACHE</code> dict) and served in
        microseconds — no database query at all.
      </p>

      <h3>5. HLL sketches</h3>
      <p>
        For non-additive metrics like <code>COUNT(DISTINCT user_id)</code>, simple precomputed totals cannot be
        combined across dimensions (you can&apos;t sum distinct counts). The DLM uses{" "}
        <strong>HyperLogLog (HLL) sketches</strong> — probabilistic data structures that approximate distinct counts
        with ~2% error and can be merged across partitions. This means &ldquo;how many active users in the US&rdquo;
        can be answered from precomputed sketches without scanning the 504M-row fact table.
      </p>
      <Code lang="text">{`Metric: Active Users = COUNT(DISTINCT user_id)
  ├── HLL sketch (overall)        → ~12,847 users
  ├── HLL sketch (country=US)     → ~4,291 users
  ├── HLL sketch (platform=iOS)   → ~3,102 users
  └── HLL sketch (segment=Ent.)   → ~1,856 users

Merge: HLL(country=US) ∪ HLL(platform=iOS) → ~5,012 unique users
  (no full scan needed)`}</Code>

      <h2>Multi-dataset routing</h2>
      <p>
        When a question arrives at <code>POST /dlm/ask</code>, the router scores it against every compiled DLM to find
        the best dataset. The scoring uses <strong>weighted lexical matching</strong>:
      </p>
      <Code lang="text">{`score = 3 × metric_hits      (metric names that appear in the question)
      + 2 × value_hits       (indexed values that appear in the question)
      + 4 × name_hits        (dataset name words that appear)
      + min(col_hits, 3)     (column names, capped at 3 to prevent generic matches)

Floor: score must be ≥ 2 to prevent stray single-word matches.
Tie-break: narrowest dataset (fewest columns) wins.`}</Code>
      <p>
        The routing is deterministic — the same question always routes to the same dataset. The routed dataset ID is
        returned in the response so the frontend can display it.
      </p>

      <h2>Question resolution</h2>
      <p>
        Once routed, the DLM resolves the question into structured components through a deterministic pipeline:
      </p>

      <h3>Metric matching</h3>
      <p>
        The question&apos;s tokens are matched against metric names and aliases. When no metric name explicitly matches,
        a smart fallback kicks in:
      </p>
      <ul>
        <li>Prefer <code>COUNT(*)</code> or <code>SUM()</code> metrics over <code>COUNT(DISTINCT)</code> for generic count questions</li>
        <li>Avoid <code>AVG</code> metrics as defaults (averages need explicit intent)</li>
        <li>Fall back to the first metric only as a last resort</li>
      </ul>
      <Callout type="tip">
        This prevents the common failure mode where &ldquo;how many events&rdquo; accidentally routes to a{" "}
        <code>COUNT(DISTINCT country)</code> metric instead of <code>SUM(actions)</code>. The DLM prefers additive
        metrics for generic &ldquo;how many&rdquo; questions.
      </Callout>

      <h3>Entity filter resolution</h3>
      <p>
        The value index resolves entity references in the question to column filters. &ldquo;in the US&rdquo; becomes{" "}
        <code>WHERE country = &apos;United States&apos;</code>. &ldquo;on iOS&rdquo; becomes{" "}
        <code>WHERE platform = &apos;iOS&apos;</code>. Multiple filters can be resolved simultaneously.
      </p>

      <h3>Relative time parsing</h3>
      <p>
        The DLM recognizes temporal expressions and converts them to SQL date filters using the dataset&apos;s configured
        date column:
      </p>
      <Code lang="text">{`"last 7 days"       → WHERE event_date >= CURRENT_DATE - INTERVAL '7 days'
"last 3 months"     → WHERE event_date >= CURRENT_DATE - INTERVAL '3 months'
"this week"         → WHERE event_date >= CURRENT_DATE - INTERVAL '7 days'
"this month"        → WHERE event_date >= CURRENT_DATE - INTERVAL '1 month'
"this quarter"      → WHERE event_date >= CURRENT_DATE - INTERVAL '3 months'
"today"             → WHERE event_date >= CURRENT_DATE
"yesterday"         → WHERE event_date >= CURRENT_DATE - INTERVAL '1 day'`}</Code>
      <p>
        When a relative time filter is present, precomputed context is bypassed in favor of a live query — stale
        precomputed totals cannot answer &ldquo;last 7 days&rdquo; correctly.
      </p>

      <h3>Superlative and top-N detection</h3>
      <p>
        Questions with superlative patterns are automatically converted to ranked queries:
      </p>
      <Code lang="text">{`"which country has the most users"    → top_n=1, ORDER BY DESC
"top 5 platforms by revenue"          → top_n=5, ORDER BY DESC
"lowest 3 regions by error rate"      → top_n=3, ORDER BY ASC
"what has the least sessions"         → top_n=1, ORDER BY ASC`}</Code>
      <p>
        Superlative keywords (<code>most, highest, largest, biggest, greatest</code>) trigger descending sort.
        Bottom keywords (<code>lowest, least, fewest, smallest, bottom</code>) trigger ascending sort.
      </p>

      <h3>Year filter extraction</h3>
      <p>
        Standalone four-digit years in the question (2020–2039 range) are extracted as date filters:{" "}
        <code>WHERE EXTRACT(YEAR FROM date_col) = 2024</code>. This lets users say &ldquo;revenue in 2024&rdquo;
        without needing a full date range.
      </p>

      <h2>Answer serving</h2>
      <p>
        The DLM has two answer paths, and every response is honestly labelled so the user knows which path served them:
      </p>

      <h3>Context path — microseconds</h3>
      <p>
        When the question maps to a precomputed shape (metric total, per-dimension breakdown, or single-dimension filter),
        the answer is served from the in-memory DLM context (<code>_ANSWER_CACHE</code> dict) with no database query.
        The response is tagged <strong>⚡ From context · no DB scan</strong>.
      </p>
      <Code lang="text">{`Precomputed shapes that serve from context:
  ├── "how many total actions"         → metric total
  ├── "actions by country"             → per-dimension breakdown
  ├── "actions in the US"              → filtered metric total
  ├── "top 5 countries by actions"     → sorted breakdown slice
  └── "which country has the most"     → top-1 from breakdown`}</Code>

      <h3>Live query path — sub-second</h3>
      <p>
        Novel combinations (multi-filter, time-scoped, cross-dimension) assemble a single SQL query against the
        warehouse. The query is built deterministically from the resolved components — metric expression, group-by
        column, WHERE clauses, ORDER BY, LIMIT. The response is tagged <strong>Live query · Xs</strong> with the
        actual execution time.
      </p>

      <h3>Context hints</h3>
      <p>
        While a live query is running, the DLM serves <strong>context hints</strong> — the precomputed metric total
        and per-filter breakdowns — as an instant preview. The user sees approximate data immediately, and the live
        query result replaces it when ready.
      </p>

      <h2>Dashboard integration</h2>
      <p>
        The DLM powers dashboard charts through a dedicated endpoint that serves aggregations from precomputed context:
      </p>

      <h3>serve-chart</h3>
      <p>
        <code>POST /dlm/serve-chart</code> takes a metric, optional group-by, and optional filters, and returns the
        result from DLM context. Dashboard charts load instantly from context instead of running live SQL against the
        warehouse. When context cannot answer (e.g. a novel filter combination), the response includes{" "}
        <code>served: false</code> and the frontend falls back to <code>/sql/execute</code>.
      </p>

      <h3>Dashboard curation</h3>
      <p>
        <code>POST /dashboards/{"{dashboard_id}"}/dlm/curate</code> precomputes the N-dimensional answer combinations
        for a dashboard&apos;s filter×chart definitions. After curation, even complex multi-filter dashboard
        interactions serve instantly from context.
      </p>

      <h3>Filter values</h3>
      <p>
        <code>GET /dlm/filter-values</code> returns distinct values for a dimension column from DLM context — no SQL
        query needed. Dashboard filter dropdowns populate instantly.
      </p>

      <h2>API reference</h2>
      <table>
        <thead><tr><th>Endpoint</th><th>Method</th><th>Purpose</th></tr></thead>
        <tbody>
          <tr><td><code>/dlm/ask</code></td><td>POST</td><td>NL→SQL: route question, resolve terms, return SQL + context answer</td></tr>
          <tr><td><code>/datasets/{"{id}"}/dlm/generate</code></td><td>POST</td><td>Compile the DLM artifact (idempotent unless <code>force=true</code>)</td></tr>
          <tr><td><code>/datasets/{"{id}"}/dlm</code></td><td>GET</td><td>Artifact status — manifest, rollups, generation time</td></tr>
          <tr><td><code>/datasets/{"{id}"}/dlm/context</code></td><td>GET</td><td>Context spec for the context editor</td></tr>
          <tr><td><code>/datasets/{"{id}"}/dlm/context</code></td><td>PUT</td><td>Save human curation overrides (aliases, breakdowns)</td></tr>
          <tr><td><code>/datasets/{"{id}"}/dlm/resolve</code></td><td>GET</td><td>Resolve a term to column + filter (retrieval probe)</td></tr>
          <tr><td><code>/datasets/{"{id}"}/dlm/refresh</code></td><td>POST</td><td>Incremental refresh — delta-merge for additive metrics</td></tr>
          <tr><td><code>/dlm/route</code></td><td>GET</td><td>Cross-dataset routing: score question against all DLMs</td></tr>
          <tr><td><code>/dlm/serve-chart</code></td><td>POST</td><td>Dashboard chart from precomputed context</td></tr>
          <tr><td><code>/dlm/filter-values</code></td><td>GET</td><td>Distinct dimension values from DLM context</td></tr>
          <tr><td><code>/dlm/coverage</code></td><td>GET</td><td>What context is compiled — datasets, date ranges, value coverage</td></tr>
          <tr><td><code>/dlm/cache/invalidate</code></td><td>POST</td><td>Clear in-memory DLM answer context (Admin only)</td></tr>
          <tr><td><code>/dlm/sweep</code></td><td>POST</td><td>Manual freshness sweep trigger (Admin only)</td></tr>
          <tr><td><code>/dlm/notify-data-change</code></td><td>POST</td><td>Pipeline webhook — invalidate + rebuild after ETL</td></tr>
          <tr><td><code>/dashboards/{"{id}"}/dlm/curate</code></td><td>POST</td><td>Precompute multi-filter dashboard combinations</td></tr>
          <tr><td><code>/datasets/{"{id}"}/freshness</code></td><td>GET</td><td>Freshness score + rebuild recommendation</td></tr>
        </tbody>
      </table>

      <Pager
        prev={{ href: "/docs/nl-to-sql", title: "AI · NL→SQL" }}
        next={{ href: "/docs/freshness", title: "Freshness Algorithm" }}
      />
    </div>
  );
}
