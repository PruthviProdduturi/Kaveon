import { PageHeader, Callout, Code, Pager } from "../../../components/docs/prose";

export const metadata = { title: "Quickstart" };

export default function Quickstart() {
  return (
    <div className="docs-prose">
      <PageHeader
        eyebrow="Getting Started"
        title="Quickstart"
        lead="Bring up the full stack locally — Studio, the API, PostgreSQL, and a two-worker Engine cluster — then run your first query, ask your first question, and query a Parquet file with the Engine CLI."
      />

      <h2>Prerequisites</h2>
      <table>
        <thead><tr><th>Requirement</th><th>Why</th></tr></thead>
        <tbody>
          <tr><td>Docker with Compose v2</td><td>Runs the whole stack. <code>docker compose version</code> should print v2.x.</td></tr>
          <tr><td>~4 GB free RAM</td><td>Six containers: Studio, API, PostgreSQL, one Engine coordinator, two Engine workers.</td></tr>
          <tr><td>Ports 3000, 8080, 8081, 5433</td><td>All bound to <code>127.0.0.1</code> only.</td></tr>
          <tr><td>Git</td><td>To clone the repository.</td></tr>
        </tbody>
      </table>
      <p>
        You do <strong>not</strong> need an OAuth application, a cloud account, an API key, or an existing
        warehouse to complete this page.
      </p>

      <h2>1 · Start the stack</h2>
      <Code lang="bash">{`git clone https://github.com/PruthviProdduturi/Kaveon.git
cd Kaveon
docker compose up -d --build`}</Code>
      <p>
        The first build compiles the Rust Engine and takes several minutes; subsequent starts are fast.
        Watch the containers become healthy:
      </p>
      <Code lang="bash">{`docker compose ps`}</Code>
      <Code lang="text">{`NAME                        STATUS
kaveon-postgres             Up (healthy)
kaveon-engine-coordinator   Up (healthy)
kaveon-engine-worker-1      Up
kaveon-engine-worker-2      Up
kaveon-api                  Up (healthy)
kaveon-studio               Up (healthy)`}</Code>

      <Callout type="note">
        The stack binds only to loopback and enables <code>KAVEON_LOCAL_MODE</code>, which signs you in as a
        local Admin so you can skip OAuth entirely. That mode is for localhost only — see{" "}
        <a href="/docs/auth">Auth &amp; RBAC</a> before exposing Kaveon to a network.
      </Callout>

      <h2>2 · Verify</h2>
      <p>Check each tier independently before opening the UI:</p>
      <Code lang="bash">{`curl -s localhost:8080/api/health     # platform API
curl -s localhost:8081/health         # Engine coordinator`}</Code>
      <p>
        Then open <a href="http://localhost:3000">http://localhost:3000</a>. You land in Studio, already
        signed in as <strong>Local Developer</strong> with the Admin role.
      </p>

      <h2>3 · Run your first query</h2>
      <p>
        Compose creates two databases: <code>kaveonmeta</code> for Kaveon&rsquo;s own state, and an empty{" "}
        <code>kaveon</code> warehouse for your data. Open <strong>Lab</strong>, select the{" "}
        <code>kaveon</code> database in the toolbar, and create something to query:
      </p>
      <Code lang="sql">{`CREATE TABLE orders (
  id       SERIAL PRIMARY KEY,
  region   TEXT   NOT NULL,
  plan     TEXT   NOT NULL,
  total    NUMERIC(10,2) NOT NULL,
  ordered  DATE   NOT NULL
);

INSERT INTO orders (region, plan, total, ordered) VALUES
  ('North America', 'Enterprise', 1200.00, '2026-08-02'),
  ('North America', 'Team',        340.00, '2026-08-11'),
  ('Europe',        'Enterprise',  980.00, '2026-08-14'),
  ('Europe',        'Team',        210.00, '2026-08-21'),
  ('Asia',          'Enterprise',  760.00, '2026-09-01'),
  ('Asia',          'Team',        150.00, '2026-09-02');`}</Code>
      <p>Press <code>Ctrl/Cmd + Enter</code> to run, then query it:</p>
      <Code lang="sql">{`SELECT region, SUM(total) AS revenue
FROM   orders
GROUP  BY region
ORDER  BY revenue DESC;`}</Code>
      <Code lang="text">{`region          revenue
North America   1540.00
Europe          1190.00
Asia             910.00`}</Code>
      <p>
        Results are cached by SHA of the query text, so re-running is instant. Full editor reference:{" "}
        <a href="/docs/sql-lab">SQL Lab</a>.
      </p>

      <h2>4 · Define a semantic dataset</h2>
      <p>
        A dataset names your dimensions and metrics once so charts and questions can be built without
        rewriting SQL. Go to <strong>Datasets → New Dataset</strong>, choose the <code>orders</code> table,
        and mark:
      </p>
      <ul>
        <li><strong>Dimensions</strong> — <code>region</code>, <code>plan</code></li>
        <li><strong>Metrics</strong> — <code>SUM(total)</code> as <em>Revenue</em></li>
        <li><strong>Time column</strong> — <code>ordered</code></li>
      </ul>
      <p>
        This is the input the DLM compiles against. Details: <a href="/docs/datasets">Semantic Datasets</a>.
      </p>

      <h2>5 · Compile the DLM context</h2>
      <p>
        On the dataset page, click <strong>Generate</strong>. Kaveon scans the table once and precomputes
        each metric&rsquo;s grand total, its breakdown per dimension, and low-cardinality dimension pairs.
        This is what lets common questions be answered with no database round trip.
      </p>
      <Callout type="tip">
        Generation cost scales with table size — seconds here, minutes on tens of millions of rows. It is a
        one-time step per dataset, repeated only when you regenerate.
      </Callout>

      <h2>6 · Ask a question</h2>
      <p>On the home page, type:</p>
      <Code lang="text">{`revenue by region`}</Code>
      <p>
        Kaveon routes the question to the dataset, matches the metric, groups by the dimension, and renders
        the result. Look at the badge above the answer:
      </p>
      <table>
        <thead><tr><th>Badge</th><th>Meaning</th></tr></thead>
        <tbody>
          <tr><td><strong>From context · no DB scan</strong></td><td>Served from precomputed context. No query ran.</td></tr>
          <tr><td><strong>From sketch · ≈ estimate</strong></td><td>Approximate distinct count from a HyperLogLog sketch.</td></tr>
          <tr><td><strong>Live query · Xs</strong></td><td>The shape was not precomputed, so SQL was assembled and run.</td></tr>
        </tbody>
      </table>
      <p>
        No hosted model is involved on any of those paths. How the routing decides:{" "}
        <a href="/docs/nl-to-sql">DLM · NL→SQL</a> and <a href="/docs/freshness">Freshness</a>.
      </p>

      <h2>7 · Query a Parquet file with the Engine</h2>
      <p>
        The Engine is a separate runtime with its own SQL. It is <strong>not</strong> wired into Studio yet —
        the query above went through the platform API, not through the Engine. To use it, point the stack at
        a directory of Parquet files and restart:
      </p>
      <Code lang="bash">{`KAVEON_DATA_PATH=/path/to/parquet docker compose up -d`}</Code>
      <p>Install the CLI and connect it to the running coordinator:</p>
      <Code lang="bash">{`curl -sSf https://raw.githubusercontent.com/PruthviProdduturi/Kaveon/dev/scripts/install.sh | sh

kaveon --server http://localhost:8081`}</Code>
      <p>
        Tables are auto-discovered from <code>.parquet</code> files in that directory, under the{" "}
        <code>kaveon</code> catalog:
      </p>
      <Code lang="sql">{`SHOW CATALOGS;
SHOW TABLES;
SELECT region, count(*) FROM orders GROUP BY region ORDER BY 2 DESC LIMIT 5;`}</Code>
      <p>Or run a single statement without entering the shell:</p>
      <Code lang="bash">{`kaveon --server http://localhost:8081 -e "SELECT count(*) FROM orders"`}</Code>
      <Callout type="warn">
        The Engine has no authentication or TLS of its own. Keep the coordinator on a trusted network — the
        Compose stack binds it to loopback for exactly this reason.
      </Callout>

      <h2>8 · Shut down</h2>
      <Code lang="bash">{`docker compose down          # stop, keep data
docker compose down -v       # stop and delete the PostgreSQL volume`}</Code>

      <h2>Where to go next</h2>
      <table>
        <thead><tr><th>To…</th><th>Read</th></tr></thead>
        <tbody>
          <tr><td>Understand datasets, charts, and questions properly</td><td><a href="/docs/concepts">Core concepts</a></td></tr>
          <tr><td>Connect a real warehouse instead of the local one</td><td><a href="/docs/data-sources">Data Sources</a> · <a href="/docs/connectors">Connector matrix</a></td></tr>
          <tr><td>Build charts and dashboards</td><td><a href="/docs/charts">Chart Builder</a> · <a href="/docs/dashboards">Dashboards</a></td></tr>
          <tr><td>Go deeper on the Engine</td><td><a href="/docs/engine">Kaveon Engine</a></td></tr>
          <tr><td>Deploy beyond localhost</td><td><a href="/docs/deployment">Deployment</a></td></tr>
        </tbody>
      </table>

      <Pager
        prev={{ href: "/docs", title: "Introduction" }}
        next={{ href: "/docs/concepts", title: "Core Concepts" }}
      />
    </div>
  );
}
