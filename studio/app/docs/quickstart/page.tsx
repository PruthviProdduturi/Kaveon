import { PageHeader, Callout, Code, Pager } from "../../../components/docs/prose";

export const metadata = { title: "Quickstart" };

export default function Quickstart() {
  return (
    <div className="docs-prose">
      <PageHeader
        eyebrow="Getting Started"
        title="Quickstart"
        lead="From sign-in to your first chart in a few minutes. This walks the golden path; every step has a dedicated deep-dive linked at the end."
      />

      <h2>1 · Sign in</h2>
      <p>
        Open Kaveon and sign in. Authentication is provider-based with no local passwords — GitHub, Google,
        or Microsoft Entra ID light up automatically when their credentials are present. The API&rsquo;s authorization
        layer defines a four-role ladder (<code>Viewer → Analyst → Editor → Admin</code>), but the NextAuth sign-in
        resolves you to <strong>Admin</strong> (if your email is in <code>AUTH_ADMIN_EMAILS</code>) or{" "}
        <strong>Viewer</strong>.
      </p>
      <Callout type="note">
        Running it yourself? See <a href="/docs/auth">Auth &amp; RBAC</a> for wiring GitHub/Google/Microsoft and the
        role model. The rest of this page assumes you&rsquo;re signed in.
      </Callout>

      <h2>2 · Connect a data source</h2>
      <p>
        Go to <strong>Data Sources → + Add Data Source</strong>. Kaveon speaks to Microsoft Fabric SQL, Azure SQL,
        PostgreSQL, MySQL, and StarRocks. Fill in the connection, click <strong>Test</strong>, and save — no
        <code>.env</code> edits, no restart.
      </p>
      <Callout type="tip">
        No warehouse handy? Skip this step. Kaveon ships with demo datasets (COVID &amp; NYC taxi) in its metadata
        database, so every feature below works out of the box.
      </Callout>

      <h2>3 · Run a query</h2>
      <p>
        Open <strong>Lab</strong>, pick your database in the toolbar, and run something. The schema browser on the left
        loads tables and columns to power autocomplete.
      </p>
      <Code lang="sql">{`SELECT region, SUM(total) AS revenue
FROM   orders
GROUP  BY region
ORDER  BY revenue DESC;`}</Code>
      <p>Press <code>Ctrl/Cmd + Enter</code> to run. Full editor reference: <a href="/docs/sql-lab">SQL Lab</a>.</p>

      <h2>4 · Define a dataset</h2>
      <p>
        A <strong>semantic dataset</strong> names your dimensions, metrics, and filter columns once so charts can be
        built without re-writing SQL. Go to <strong>Datasets → + New Dataset</strong>, pick a table (or join a few),
        and mark which columns are dimensions vs. metrics.
      </p>

      <h2>5 · Build a chart</h2>
      <p>
        <strong>Charts → + New Chart</strong>. Choose the dataset, pick a chart type (37, from bars to a 3D globe),
        and drag dimensions and metrics into place. Kaveon generates the JOINs and aggregation for you and renders live.
      </p>

      <h2>6 · Assemble a dashboard</h2>
      <p>
        <strong>Dashboards → + New Dashboard</strong> gives you a drag-and-drop canvas. Drop charts into resizable
        tiles, add markdown and section headers, and wire up <strong>cross-chart filtering</strong> — click a bar to
        filter every related chart at once.
      </p>

      <h2>7 · Just ask</h2>
      <p>
        On the home page, type a question in plain English — <em>&ldquo;top regions by revenue this quarter&rdquo;</em> —
        and Kaveon detects the right dataset, builds the SQL, runs it, and renders a chart inline. How that works:{" "}
        <a href="/docs/nl-to-sql">AI · NL→SQL</a>.
      </p>

      <Pager
        prev={{ href: "/docs", title: "Introduction" }}
        next={{ href: "/docs/concepts", title: "Core Concepts" }}
      />
    </div>
  );
}
