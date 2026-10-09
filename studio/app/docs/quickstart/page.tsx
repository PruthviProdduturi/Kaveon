import { PageHeader, Callout, Code, Pager } from "../../../components/docs/prose";

export const metadata = { title: "Quickstart" };

export default function Quickstart() {
  return (
    <div className="docs-prose">
      <PageHeader
        eyebrow="Getting Started"
        title="Quickstart"
        lead="A five-minute local walkthrough for Studio, the API, and the Engine. Use Install and deploy for VM, AKS, or Vercel hosting; use Deployment topology and Operations for production operation."
      />

      <h2>Prerequisites</h2>
      <table>
        <thead><tr><th>Requirement</th><th>Why</th></tr></thead>
        <tbody>
          <tr><td>Docker with Compose v2</td><td>Runs the whole stack. <code>docker compose version</code> should print v2.x.</td></tr>
          <tr><td>~4 GB free RAM</td><td>Five containers: Studio, the API, one Engine coordinator and two Engine workers. There is no database to install.</td></tr>
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
kaveon-engine-coordinator   Up (healthy)
kaveon-engine-worker-1      Up
kaveon-engine-worker-2      Up
kaveon-api                  Up (healthy)
kaveon-studio               Up (healthy)`}</Code>

      <Callout type="note">
        The stack binds only to loopback. Configure Microsoft Entra ID in the ignored <code>.env</code> to
        use personal or work sign-in; without provider credentials, local mode uses a development Admin.
        That mode is for localhost only — see{" "}
        <a href="/docs/auth">Auth &amp; RBAC</a> before exposing Kaveon to a network.
      </Callout>

      <h2>2 · Verify</h2>
      <p>Check each tier independently before opening the UI:</p>
      <Code lang="bash">{`curl -s localhost:8080/api/health     # platform API
curl -s localhost:8081/health         # Engine coordinator`}</Code>
      <p>
        Then open <a href="http://localhost:3000">http://localhost:3000</a>. If Microsoft is configured,
        sign in with your Microsoft account; otherwise Studio uses the local development Admin.
      </p>

      <h2>3 · Run your first read query</h2>
      <p>
        Open <strong>Catalog</strong> to choose a registered catalog and schema, then open <strong>SQL Lab</strong>.
        The local showcase uses read-only Parquet and Delta tables; Kaveon catalog DDL registers existing lake data and
        does not create arbitrary PostgreSQL-style user tables.
      </p>
      <Code lang="sql">{`SELECT *
FROM OpenSource.public.nyc_taxi_borough
LIMIT 25;`}</Code>
      <p>This table is part of the OpenSource showcase manifest. For another lake, replace it with a table shown by <code>SHOW TABLES</code>. Press <code>Ctrl/Cmd + Enter</code> to run the query.</p>
      <p>
        Results are cached by SHA of the query text, so re-running is instant. Full editor reference:{" "}
        <a href="/docs/sql-lab">SQL Lab</a>.
      </p>

      <h2>4 · Define a semantic dataset</h2>
      <p>
        A dataset names your dimensions and metrics once so charts and questions can be built without
        rewriting SQL. Go to <strong>Datasets → New Dataset</strong>, choose the table you just queried,
        and mark:
      </p>
      <ul>
        <li><strong>Dimensions</strong> — choose categorical columns such as borough, country, or status</li>
        <li><strong>Metrics</strong> — choose a numeric column and aggregation such as <code>SUM</code> or <code>COUNT</code></li>
        <li><strong>Time column</strong> — choose a date or timestamp column when the table has one</li>
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
        Studio sends analytical statements through the Engine. For direct Engine access, point the CLI at a directory
        of Parquet or Delta files and restart:
      </p>
      <Code lang="bash">{`KAVEON_DATA_PATH=/path/to/parquet docker compose up -d`}</Code>
      <p>Install the CLI and connect it to the running coordinator:</p>
      <Code lang="bash">{`curl -fsSL https://raw.githubusercontent.com/PruthviProdduturi/Kaveon/dev/scripts/install.sh | bash

kaveon --server http://localhost:8081`}</Code>
      <p>
        Tables are auto-discovered from <code>.parquet</code> files in that directory, under the configured local catalog:
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
      <Code lang="bash">{`docker compose down          # stop, keep everything
docker compose down -v       # stop and delete the volumes — see below`}</Code>
      <Callout type="warn">
        <code>-v</code> deletes the <code>catalog-data</code> volume, and that volume holds the Engine&rsquo;s catalog:
        every catalog, schema and table definition you registered, the planner&rsquo;s statistics, and every cube.
        Table data in your mounted directory is untouched, but the map to it is gone and the cubes have to be rebuilt,
        which is not quick. Use plain <code>docker compose down</code> unless you mean to start over.
      </Callout>

      <h2>Where to go next</h2>
      <table>
        <thead><tr><th>To…</th><th>Read</th></tr></thead>
        <tbody>
          <tr><td>Understand datasets, charts, and questions properly</td><td><a href="/docs/concepts">Core concepts</a></td></tr>
          <tr><td>Connect a real warehouse instead of the local one</td><td><a href="/docs/data-sources">Data Sources</a> · <a href="/docs/connectors">Connector matrix</a></td></tr>
          <tr><td>Build charts and dashboards</td><td><a href="/docs/charts">Chart Builder</a> · <a href="/docs/dashboards">Dashboards</a></td></tr>
          <tr><td>Go deeper on the Engine</td><td><a href="/docs/engine">Kaveon Engine</a></td></tr>
          <tr><td>Deploy beyond localhost</td><td><a href="/docs/installation">Install &amp; deploy</a> · <a href="/docs/deployment">Deployment topology</a></td></tr>
        </tbody>
      </table>

      <Pager prev={{ href: "/docs", title: "Introduction" }} next={{ href: "/docs/concepts", title: "Core Concepts" }} />
    </div>
  );
}
