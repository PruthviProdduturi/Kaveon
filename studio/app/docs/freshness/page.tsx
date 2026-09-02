import { PageHeader, Callout, Code, Pager } from "../../../components/docs/prose";

export const metadata = { title: "Freshness Algorithm" };

export default function FreshnessDocs() {
  return (
    <div className="docs-prose">
      <PageHeader
        eyebrow="Core Engine"
        title="Freshness Algorithm"
        lead="DLM context elements carry a validity score in [0, 1]. For PostgreSQL sources, Kaveon estimates drift from catalog statistics instead of rescanning analytical tables, then routes eligible requests to context, hybrid, or live-query paths."
      />

      <Callout type="note">
        The freshness algorithm is <strong>per-element, not per-dataset</strong>. A single dataset can have some
        elements fresh and others stale — the router can trust part of the context and refresh only the stale part
        (the <em>hybrid</em> route). This is what makes Kaveon efficient on large datasets: it only recomputes what
        changed.
      </Callout>

      <h2>The composite score</h2>
      <p>
        The validity score is a multiplicative composite of two factors — either one going to zero invalidates the
        element, and a third factor (usage) modulates the time factor&apos;s half-life:
      </p>
      <Code lang="text">{`score = time_factor(age, effective_half_life) × change_factor(Δrows, threshold)

where effective_half_life = base_half_life / (1 + usage_gain × ln(1 + usage_count))`}</Code>
      <p>
        The score is computed on-demand from cheap metadata — never by re-querying the underlying data. Computing a
        score costs one lightweight catalog query to read change counters from{" "}
        <code>pg_stat_user_tables</code>.
      </p>

      <h2>Factor 1: Time decay</h2>
      <p>
        Exponential decay by elapsed time since the element was last refreshed. The half-life is the time at which
        an unused, unchanged element&apos;s freshness has fallen to 0.5.
      </p>
      <Code lang="python">{`def _time_factor(age_seconds: float, half_life: float) -> float:
    if age_seconds <= 0:
        return 1.0
    return math.exp(-math.log(2.0) * age_seconds / max(1.0, half_life))`}</Code>
      <table>
        <thead><tr><th>Age</th><th>Score (6h half-life)</th></tr></thead>
        <tbody>
          <tr><td>0 hours</td><td>1.000</td></tr>
          <tr><td>1 hour</td><td>0.891</td></tr>
          <tr><td>3 hours</td><td>0.707</td></tr>
          <tr><td>6 hours</td><td>0.500</td></tr>
          <tr><td>12 hours</td><td>0.250</td></tr>
          <tr><td>24 hours</td><td>0.063</td></tr>
        </tbody>
      </table>
      <p>
        The base half-life is <strong>6 hours</strong> — after 6 hours with no changes and no usage weighting, the
        time factor drops to 0.5. This means purely time-based staleness alone triggers a rebuild within half a day.
      </p>

      <h2>Factor 2: Change detection</h2>
      <p>
        This is the load-bearing trick: the algorithm detects data drift from a counter, not from a scan. PostgreSQL
        maintains <code>n_mod_since_analyze</code> in <code>pg_stat_user_tables</code> — a running count of row
        modifications (inserts, updates, deletes) since the last <code>ANALYZE</code>. The DLM records this counter
        at compilation time; at scoring time, it reads the current value and computes the delta.
      </p>
      <Code lang="python">{`def _change_factor(rows_at_capture, mods_now, mods_at_capture,
                   last_analyze_changed) -> float:
    delta = max(0, mods_now - mods_at_capture)
    if last_analyze_changed and delta == 0:
        # analyze ran but counter reset — treat as at least the half fraction
        delta = max(1, int(0.05 * max(1, rows_at_capture)))
    frac = delta / float(max(1, rows_at_capture))
    return math.exp(-math.log(2.0) * frac / 0.05)`}</Code>
      <p>
        The change sensitivity is set so that <strong>5% of rows modified = change factor of 0.5</strong> (half stale).
        This means:
      </p>
      <table>
        <thead><tr><th>Rows modified</th><th>Change factor</th></tr></thead>
        <tbody>
          <tr><td>0%</td><td>1.000</td></tr>
          <tr><td>1%</td><td>0.871</td></tr>
          <tr><td>5%</td><td>0.500</td></tr>
          <tr><td>10%</td><td>0.250</td></tr>
          <tr><td>20%</td><td>0.063</td></tr>
        </tbody>
      </table>

      <Callout type="tip">
        The <code>pg_stat_user_tables</code> counters are maintained by PostgreSQL for free as part of its autovacuum
        infrastructure. Reading them is a single catalog query — no table scan, no I/O, no locks. This is what makes
        the freshness algorithm &ldquo;zero-scan&rdquo; with respect to analytical tables: it uses a small catalog query rather than rescanning source rows.
      </Callout>

      <h3>Autovacuum edge case</h3>
      <p>
        When PostgreSQL&apos;s autovacuum runs <code>ANALYZE</code>, the <code>n_mod_since_analyze</code> counter
        resets to zero. The algorithm detects this by comparing the <code>last_analyze</code> timestamp: if it changed
        but the counter is zero, the algorithm assumes at least the half-fraction of rows were modified. This prevents
        a false &ldquo;fresh&rdquo; signal after autovacuum.
      </p>

      <h2>Factor 3: Usage weighting</h2>
      <p>
        Hot elements — those frequently relied upon for answers — get a <strong>shorter effective half-life</strong>.
        The answers your team relies on most are kept the freshest. Usage does not lengthen the half-life; it can only
        shorten it.
      </p>
      <Code lang="python">{`def _effective_half_life(usage_count: int) -> float:
    hl = BASE_HALF_LIFE_SECONDS / (1.0 + USAGE_GAIN * math.log1p(max(0, usage_count)))
    return max(MIN_HALF_LIFE_SECONDS, hl)`}</Code>
      <table>
        <thead><tr><th>Usage count</th><th>Effective half-life</th></tr></thead>
        <tbody>
          <tr><td>0 (never used)</td><td>6h 0min (base)</td></tr>
          <tr><td>10</td><td>3h 16min</td></tr>
          <tr><td>100</td><td>2h 18min</td></tr>
          <tr><td>1,000</td><td>1h 45min</td></tr>
          <tr><td>10,000</td><td>1h 25min</td></tr>
        </tbody>
      </table>
      <p>
        A floor of <strong>10 minutes</strong> prevents a hyper-used element from demanding a refresh on literally
        every request.
      </p>

      <h2>Tunables</h2>
      <table>
        <thead><tr><th>Constant</th><th>Value</th><th>Purpose</th></tr></thead>
        <tbody>
          <tr><td><code>BASE_HALF_LIFE_SECONDS</code></td><td>21,600 (6h)</td><td>Time until an unused, unchanged element is half-stale</td></tr>
          <tr><td><code>CHANGE_HALF_FRACTION</code></td><td>0.05 (5%)</td><td>Row modification fraction at which change factor = 0.5</td></tr>
          <tr><td><code>USAGE_GAIN</code></td><td>0.35</td><td>How aggressively usage shortens the half-life</td></tr>
          <tr><td><code>MIN_HALF_LIFE_SECONDS</code></td><td>600 (10min)</td><td>Floor: even the most-used element cannot go below 10 minutes</td></tr>
          <tr><td><code>DEFAULT_THRESHOLD</code></td><td>0.70</td><td>Score below which an element is considered stale</td></tr>
        </tbody>
      </table>

      <h2>Route decision</h2>
      <p>
        Given the validity scores for all elements a question depends on, the router makes a per-element decision:
      </p>
      <Code lang="text">{`All elements ≥ threshold (0.70)
  → Route: CONTEXT — answer entirely from precomputed DLM context. No query.

Some elements below threshold
  → Route: HYBRID — answer fresh elements from context, query only the stale ones.
    This is the efficiency trick: a 504M-row dataset with one stale dimension
    only refreshes that one dimension, not the entire artifact.

All elements below threshold
  → Route: QUERY — full live query against the warehouse.`}</Code>

      <Callout type="note">
        The routing is <strong>per-element</strong>, not per-dataset. A dataset with 10 dimension columns might have
        9 fresh and 1 stale — the router queries only the stale one. This is what makes the DLM efficient on large
        datasets with many dimensions.
      </Callout>

      <h2>Three rebuild triggers</h2>
      <p>
        Stale DLM context can be rebuilt through three independent paths, ensuring data never stays stale for long:
      </p>

      <h3>1. On-ask rebuild</h3>
      <p>
        Every question that routes through the DLM calls <code>maybe_auto_rebuild()</code> after serving the answer.
        If the freshness score is below threshold, a <strong>background thread</strong> triggers a rebuild — the
        user gets their answer immediately (from live query if needed), and the rebuilt context is ready for the
        next question.
      </p>
      <Code lang="text">{`POST /dlm/ask
  │
  ├── Serve answer (context or live query)
  │
  └── maybe_auto_rebuild(dataset_id)
        └── score < threshold?  →  background thread: generate_dlm(force=True)`}</Code>

      <h3>2. Proactive sweep</h3>
      <p>
        A background daemon thread runs <code>freshness_sweep()</code> every <strong>30 minutes</strong>, checking
        every dataset with a compiled DLM. Stale artifacts are rebuilt proactively — before any user asks.
        This means even datasets that nobody is currently querying stay fresh.
      </p>
      <Code lang="text">{`Every 30 minutes (background thread):
  for each dataset with a compiled DLM:
    score = check_freshness(dataset_id)
    if score < threshold:
      generate_dlm(dataset_id, force=True)
      log("rebuilt {dataset_name}: score was {score}")`}</Code>
      <p>
        The sweep starts automatically when the API boots (module-level <code>_start_sweep_loop()</code>). It can
        also be triggered manually via <code>POST /dlm/sweep</code> (Admin only).
      </p>

      <h3>3. Pipeline webhook</h3>
      <p>
        Data pipelines can call <code>POST /dlm/notify-data-change?dataset_id=X</code> after loading new data. This
        immediately invalidates the in-memory DLM context and triggers a background rebuild — no waiting for the next
        sweep or user query.
      </p>
      <Code lang="text">{`ETL pipeline completes
  │
  └── POST /dlm/notify-data-change?dataset_id=144
        ├── invalidate_caches(dataset_id)    — clear in-memory DLM answer context
        └── _trigger_background_rebuild()    — recompile the DLM artifact`}</Code>

      <h2>The zero-scan trick</h2>
      <p>
        The fundamental insight is that <strong>detecting whether data changed does not require reading the data</strong>.
        PostgreSQL already tracks row modification counters for its own autovacuum decisions. The freshness algorithm
        piggybacks on these counters:
      </p>
      <Code lang="sql">{`SELECT relname AS table_name,
       n_live_tup,
       n_mod_since_analyze,
       GREATEST(COALESCE(last_analyze, 'epoch'),
                COALESCE(last_autoanalyze, 'epoch')) AS last_analyze
FROM pg_stat_user_tables
WHERE schemaname = 'public'`}</Code>
      <p>
        This single catalog query returns change signals for every table in the schema — typically in under a
        millisecond on the documented reference path. It avoids analytical table scans and application-level locks; catalog access still has normal database I/O and scheduling costs. The result estimates how much the data
        has drifted since the DLM was last compiled.
      </p>

      <Callout type="tip">
        This approach requires a PostgreSQL-compatible source that exposes the expected statistics semantics (including Azure Database for PostgreSQL, which Kaveon uses). The <code>pg_stat_user_tables</code> view is part of PostgreSQL&apos;s core statistics
        collector and requires no extensions or configuration.
      </Callout>

      <h2>Observability</h2>
      <p>
        The freshness endpoint returns a full breakdown for debugging and monitoring:
      </p>
      <Code lang="json">{`GET /datasets/144/freshness
{
  "score": 0.72,
  "recommendation": "use_context",
  "factors": {
    "time": 0.81,
    "change": 0.89,
    "age_seconds": 5420.3,
    "effective_half_life_seconds": 17280.0,
    "usage_count": 47,
    "row_delta": 12840
  },
  "element_key": "public.kaveon_events_enriched",
  "threshold": 0.70
}`}</Code>
      <p>
        Every factor is exposed — time, change, usage count, effective half-life, row delta. This makes the
        algorithm fully inspectable: you can see exactly why context is trusted or why a rebuild was triggered.
      </p>

      <Pager
        prev={{ href: "/docs/dlm", title: "Data Language Model" }}
        next={{ href: "/docs/charts", title: "Chart Builder" }}
      />
    </div>
  );
}
