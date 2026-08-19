import { PageHeader, Callout, Pager } from "../../../components/docs/prose";

export const metadata = { title: "Core concepts" };

export default function ConceptsDocs() {
  return (
    <div className="docs-prose">
      <PageHeader
        eyebrow="Getting Started"
        title="Core concepts"
        lead="Five ideas explain how everything in Kaveon fits together. Once these click, every page in these docs is just detail."
      />

      <h2>Two kinds of database</h2>
      <p>Kaveon always talks to two very different things — keeping them straight avoids most confusion:</p>
      <table>
        <thead><tr><th></th><th>Metadata database</th><th>Data sources</th></tr></thead>
        <tbody>
          <tr><td><strong>Holds</strong></td><td>Kaveon&rsquo;s own state — datasets, charts, dashboards, history, themes, roles</td><td>Your actual data — warehouses and databases you query</td></tr>
          <tr><td><strong>Configured via</strong></td><td>Setup wizard / <code>Settings → Metadata Server</code></td><td><code>Data Sources</code> page (1 to N)</td></tr>
          <tr><td><strong>Count</strong></td><td>Exactly one</td><td>As many as you register</td></tr>
        </tbody>
      </table>

      <h2>Data source → dataset → chart → dashboard</h2>
      <p>Content builds in a chain, each layer reusable by the next:</p>
      <ul>
        <li><strong>Data source</strong> — a database connection (<a href="/docs/data-sources">docs</a>).</li>
        <li><strong>Dataset</strong> — a semantic layer over tables: named dimensions and metrics (<a href="/docs/datasets">docs</a>).</li>
        <li><strong>Chart</strong> — a visualization bound to a dataset (<a href="/docs/charts">docs</a>).</li>
        <li><strong>Dashboard</strong> — a canvas of charts with shared filters (<a href="/docs/dashboards">docs</a>).</li>
      </ul>
      <Callout type="tip">
        You never write JOINs or GROUP BYs for charts — the dataset encodes intent once and Kaveon generates the SQL. Drop
        into <a href="/docs/sql-lab">SQL Lab</a> only when you want raw control.
      </Callout>

      <h2>Roles and visibility</h2>
      <p>
        Two independent axes govern access. <strong>Roles</strong> — <code>Viewer → Analyst → Editor → Admin</code> — gate
        what you can <em>do</em> (run SQL, create content, publish, administer). <strong>Visibility</strong> —
        <code> private / internal / published</code> — gates who can <em>see</em> a given dataset, chart, or dashboard. Full
        model in <a href="/docs/auth">Auth &amp; RBAC</a>.
      </p>
      <p>
        The full four-role ladder lives in the API&rsquo;s authorization layer. Through the NextAuth sign-in, a user
        resolves to just two of those roles: <strong>Admin</strong> (email listed in <code>AUTH_ADMIN_EMAILS</code>) or
        <strong> Viewer</strong>.
      </p>

      <h2>Ask, don&rsquo;t query</h2>
      <p>
        The home page turns plain-English questions into charts with <strong>no hosted LLM</strong>. The primary engine
        is the <strong>DLM (Data Language Model)</strong> — a compiled per-dataset context artifact that answers the
        common questions from precomputed context with <strong>no database scan</strong> (badged &ldquo;From
        context&rdquo; vs &ldquo;Live query&rdquo;); a deterministic in-browser parser is the fallback. See{" "}
        <a href="/docs/nl-to-sql">AI · NL→SQL</a>.
      </p>

      <h2>Everything is live and UI-driven</h2>
      <p>
        No stale cache — queries hit the source every time, with connection pools warmed at startup. And nearly all
        configuration (data sources, auth provider, metadata server, AI keys) is done from the UI, no{" "}
        <code>.env</code> edits or restarts.
      </p>

      <Pager
        prev={{ href: "/docs/quickstart", title: "Quickstart" }}
        next={{ href: "/docs/sql-lab", title: "SQL Lab" }}
      />
    </div>
  );
}
