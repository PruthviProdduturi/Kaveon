import { PageHeader, Callout, Pager } from "../../components/docs/prose";

export const metadata = { title: "Introduction" };

export default function DocsIntro() {
  return (
    <div className="docs-prose">
      <PageHeader
        eyebrow="Introduction"
        title="Kaveon documentation"
        lead="Kaveon is a unified data intelligence platform: a columnar query engine, a deterministic Data Language Model, and a complete analytics studio — built as one system, over data that stays in storage you own."
      />

      <h2>The idea</h2>
      <p>
        Most analytics stacks are assembled. You run a BI tool, point it at someone else&rsquo;s query engine,
        bolt an LLM onto the front for natural language, and move data between all three. Every seam is a
        place where cost, latency, and correctness leak.
      </p>
      <p>
        Kaveon is built as <strong>one system with three pillars</strong>. The engine is ours, so query
        execution is not rented. The language layer is deterministic, so a question resolves through
        inspectable rules rather than a model&rsquo;s guess. The studio is the surface over both, not the
        product boundary.
      </p>

      <h2>Three pillars</h2>
      <table>
        <thead>
          <tr><th>Pillar</th><th>What it is</th><th>Maturity</th></tr>
        </thead>
        <tbody>
          <tr>
            <td><strong><a href="/docs/engine">Kaveon Engine</a></strong></td>
            <td>A vectorized columnar query engine in Rust, over Arrow. Its own SQL parser, planner, optimizer, and distributed runtime — no DuckDB, Trino, or Spark embedded inside it.</td>
            <td>Alpha</td>
          </tr>
          <tr>
            <td><strong><a href="/docs/dlm">Kaveon DLM</a></strong></td>
            <td>The Data Language Model: compiles per-dataset context, then answers questions by resolving them against that context. No hosted LLM on this path.</td>
            <td>Shipping</td>
          </tr>
          <tr>
            <td><strong>Kaveon Studio</strong></td>
            <td>SQL Lab, semantic datasets, 37 chart types, dashboards with cross-filtering, and administration.</td>
            <td>Shipping</td>
          </tr>
        </tbody>
      </table>

      <h2>What makes it different</h2>
      <p>
        <strong>Deterministic language, not a model call.</strong> Ask &ldquo;active users by plan last
        quarter&rdquo; and the DLM routes it to a dataset, resolves the entities, and either answers from
        precomputed context or assembles SQL. The same question yields the same query every time, at no
        token cost and with no model latency. Generative approaches cover broader phrasing; this one is
        reproducible and auditable. See <a href="/docs/nl-to-sql">DLM · NL→SQL</a>.
      </p>
      <p>
        <strong>Answers without a scan.</strong> The DLM precomputes metric totals, per-dimension
        breakdowns, and low-cardinality dimension pairs when it compiles a dataset. Common questions are
        served from that context with no database round trip; only uncovered shapes fall through to a live
        query. The UI always tells you which path answered you. See <a href="/docs/freshness">Freshness</a>.
      </p>
      <p>
        <strong>Your data stays where it is.</strong> The direction is the Live Lake Path — Engine reads
        Parquet and Delta in your storage without a mandatory import step. Today that means local
        filesystem; ADLS Gen2, S3, and Iceberg are target work, tracked honestly in{" "}
        <a href="/docs/engine/storage">Storage &amp; Catalogs</a>.
      </p>

      <Callout type="note">
        Kaveon is open source under MIT and self-hosted. Nothing phones home, and the deterministic
        question path requires no AI provider at all. Studio&rsquo;s optional inline SQL assistant can use a
        hosted model if you configure one; that is separate from DLM routing.
      </Callout>

      <h2>Where the project actually is</h2>
      <p>
        These docs separate what runs today from what is designed but unbuilt, and they say which is which
        on every page. The two things worth knowing before you read further:
      </p>
      <ul>
        <li>
          <strong>Studio, the API, and the DLM are the shipping product.</strong> They run in production and
          query your registered SQL sources directly.
        </li>
        <li>
          <strong>Engine is alpha and not yet wired into Studio.</strong> It runs standalone through its CLI
          and HTTP API. Studio queries do not route through it yet. It has no auth or TLS of its own, so keep
          it on a trusted network.
        </li>
      </ul>

      <Callout type="tip">
        Want it running locally? The <a href="/docs/quickstart">Quickstart</a> brings up the whole stack —
        Studio, API, PostgreSQL, and a two-worker Engine cluster — with one command.
      </Callout>

      <h2>Choose your path</h2>
      <table>
        <thead><tr><th>If you want to…</th><th>Start here</th></tr></thead>
        <tbody>
          <tr><td>Run Kaveon locally and try it</td><td><a href="/docs/quickstart">Quickstart</a></td></tr>
          <tr><td>Understand the model before committing</td><td><a href="/docs/concepts">Core concepts</a></td></tr>
          <tr><td>See how the pieces fit</td><td><a href="/docs/architecture">Architecture</a></td></tr>
          <tr><td>Evaluate or develop the query engine</td><td><a href="/docs/engine">Kaveon Engine</a></td></tr>
          <tr><td>Understand deterministic NL→SQL</td><td><a href="/docs/dlm">Data Language Model</a></td></tr>
          <tr><td>Integrate programmatically</td><td><a href="/docs/api-reference">API reference</a></td></tr>
          <tr><td>Deploy and operate it</td><td><a href="/docs/deployment">Deployment</a> · <a href="/docs/operations">Operations</a></td></tr>
          <tr><td>Read the underlying research</td><td><a href="/docs/research">Papers &amp; patents</a></td></tr>
        </tbody>
      </table>

      <h2>How these docs are organized</h2>
      <ul>
        <li><strong>Getting Started</strong> — what Kaveon is, a runnable quickstart, and the core concepts reused everywhere.</li>
        <li><strong>Platform</strong> — architecture, the API surface, and the connector matrix.</li>
        <li><strong>Studio</strong> — SQL Lab, charts, dashboards, semantic datasets, and data sources.</li>
        <li><strong>Intelligence</strong> — the Data Language Model, NL→SQL, and freshness-based routing.</li>
        <li><strong>Engine</strong> — the Rust engine manual: architecture, SQL, distributed runtime, storage, and memory.</li>
        <li><strong>Deploy &amp; Operate</strong> — deployment, auth and RBAC, operations, troubleshooting, upgrades, and releases.</li>
        <li><strong>Research</strong> — technical papers, the patent disclosure, and comparisons with other engines.</li>
      </ul>
      <p>
        Every page carries a status badge and a verification date. Desktop pages have an outline on the
        right; the sidebar filter searches titles, descriptions, keywords, and body text.
      </p>

      <Pager next={{ href: "/docs/quickstart", title: "Quickstart" }} />
    </div>
  );
}
