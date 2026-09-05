import { Callout, Code, Diagram, PageHeader, Pager } from "../../../components/docs/prose";

export const metadata = { title: "Kaveon Engine" };

export default function EngineDocs() {
  return <div className="docs-prose">
    <PageHeader
      eyebrow="Engine Manual"
      title="Kaveon Engine"
      lead="A vectorized columnar query engine in Rust: its own SQL parser, planner, optimizer, catalog, and distributed runtime, reading Parquet and Delta directly over Arrow."
    />

    <Callout type="warn">
      <strong>Alpha.</strong> Engine is not yet Studio&rsquo;s execution backend — queries you run in the UI do
      not go through it. It also has no end-user authentication or TLS, so keep the server on a trusted
      network. Cloud object storage, Iceberg, admission control, and aggregate/join spill are open gates.
    </Callout>

    <Diagram
      src="/docs/architecture/kaveon-engine-pipeline.svg"
      alt="Kaveon Engine coordinator, distributed vectorized execution, exchange, catalog, and lake-read pipeline"
      caption="The coordinator plans versioned fragments; workers execute Arrow batches and exchange partitions. Cloud storage and advanced optimizer capabilities remain targets."
    />

    <h2>Why it exists</h2>
    <p>
      Most analytics products wrap an engine someone else wrote — DuckDB embedded, Trino deployed alongside,
      or SQL pushed down to whatever the customer already runs. Kaveon Engine is first-party so that
      execution, storage access, and the semantic layer can be designed against each other rather than
      negotiated across a boundary. It embeds no other engine.
    </p>

    <h2>Install</h2>
    <Code lang="bash">{`# macOS / Linux — installs to ~/.local/bin
curl -sSf https://raw.githubusercontent.com/PruthviProdduturi/Kaveon/dev/scripts/install.sh | sh

# Windows (PowerShell)
irm https://raw.githubusercontent.com/PruthviProdduturi/Kaveon/dev/scripts/install.ps1 | iex

# From source
cd engine && cargo install --path crates/cli`}</Code>

    <h2>Define a catalog</h2>
    <p>
      A catalog points the Engine at storage. Drop one TOML file per catalog into{" "}
      <code>~/.kaveon/catalogs/</code> — the filename becomes the catalog name — or declare them inline in{" "}
      <code>~/.kaveon/config.toml</code>.
    </p>
    <Code lang="toml">{`# ~/.kaveon/catalogs/warehouse.toml
type      = "local"
base_path = "/data/warehouse"`}</Code>
    <p>
      Tables are auto-discovered from the <code>.parquet</code> files in <code>base_path</code>, and Delta
      tables from directories carrying a complete JSON commit log. Register tables explicitly when you want
      to control the name, schema, or format:
    </p>
    <Code lang="toml">{`[[table]]
name     = "orders"
schema   = "default"
location = "orders/"
format   = "delta"       # parquet | delta | iceberg
access   = "shortcut"    # read in place, no rewrite`}</Code>

    <h2>Run a query</h2>
    <p>The CLI runs embedded with <code>--local</code>, which needs no server:</p>
    <Code lang="bash">{`kaveon --local --data-dir /data/warehouse`}</Code>
    <Code lang="sql">{`SHOW CATALOGS;
SHOW TABLES;
SELECT region, count(*) AS n
FROM   orders
GROUP  BY region
ORDER  BY n DESC
LIMIT  5;`}</Code>
    <Callout type="note">
      <code>SHOW CATALOGS</code>, <code>SHOW SCHEMAS</code>, <code>SHOW TABLES</code>,{" "}
      <code>DESCRIBE</code> and <code>USE catalog.schema</code> are resolved by the CLI against the catalog,
      not by the SQL engine. They work in the shell but are not statements you can POST to{" "}
      <code>/v1/statement</code>.
    </Callout>
    <p>Or execute one statement and exit — useful in scripts:</p>
    <Code lang="bash">{`kaveon --local --data-dir /data/warehouse -e "SELECT count(*) FROM orders"`}</Code>

    <h2>Run the cluster</h2>
    <p>
      For distributed execution, start a coordinator and one or more workers, then point the CLI at the
      coordinator. Without <code>--local</code> the CLI is a thin remote client and defaults to{" "}
      <code>http://localhost:8080</code>.
    </p>
    <Code lang="bash">{`cargo run -p kaveon-server -- /etc/kaveon/coordinator.toml
kaveon --server http://localhost:8080`}</Code>
    <p>
      The Compose stack in the <a href="/docs/quickstart">Quickstart</a> does this for you — a coordinator
      plus two workers, with the coordinator published on port <code>8081</code>.
    </p>
    <table>
      <thead><tr><th>Setting</th><th>TOML</th><th>Environment</th></tr></thead>
      <tbody>
        <tr><td>Node identity</td><td><code>node.environment</code></td><td><code>KAVEON_NODE_ID</code>, <code>KAVEON_ENVIRONMENT</code></td></tr>
        <tr><td>Coordinator role</td><td><code>node.coordinator</code></td><td><code>KAVEON_COORDINATOR</code></td></tr>
        <tr><td>HTTP port</td><td><code>http.port</code></td><td><code>KAVEON_HTTP_PORT</code></td></tr>
        <tr><td>Worker discovery</td><td>—</td><td><code>KAVEON_DISCOVERY_URI</code>, <code>KAVEON_ADVERTISED_URI</code></td></tr>
        <tr><td>Data and catalog dirs</td><td><code>storage.data_dir</code>, <code>storage.catalog_dir</code></td><td><code>KAVEON_DATA_DIR</code></td></tr>
      </tbody>
    </table>
    <p>Environment variables override TOML. Operational endpoints: <code>/ui</code> for the console, <code>/health</code> and <code>/ready</code> for probes.</p>

    <h2>What SQL runs today</h2>
    <table>
      <thead><tr><th>Supported</th><th>Not executable yet</th></tr></thead>
      <tbody>
        <tr>
          <td>
            <code>SELECT</code> with projection and aliases · <code>WHERE</code> · <code>GROUP BY</code> ·{" "}
            <code>SUM</code>/<code>COUNT</code>/<code>AVG</code>/<code>MIN</code>/<code>MAX</code> ·{" "}
            <code>COUNT(DISTINCT)</code> · <code>ORDER BY</code> with null placement · <code>LIMIT</code>/TopN ·
            equi and cross joins · arithmetic · <code>catalog.schema.table</code>
          </td>
          <td>
            CTEs · subqueries · window functions · <code>HAVING</code> · <code>SELECT DISTINCT</code> ·{" "}
            <code>CASE</code> · set operations · DDL and DML · non-equality join conditions
          </td>
        </tr>
      </tbody>
    </table>
    <Callout type="note">
      Treat unsupported syntax as unsupported even when the parser accepts it — the executable contract is the
      intersection of parsing, planning, and physical operator construction. Full matrix:{" "}
      <a href="/docs/sql-compatibility">SQL Compatibility</a>.
    </Callout>

    <h2>HTTP surface</h2>
    <table><thead><tr><th>Method</th><th>Path</th><th>Purpose</th></tr></thead><tbody>
      <tr><td>POST</td><td><code>/v1/statement</code></td><td>Execute SQL on the coordinator.</td></tr>
      <tr><td>GET</td><td><code>/v1/query</code></td><td>List recent process-local query records.</td></tr>
      <tr><td>GET / DELETE</td><td><code>/v1/query/{`{query_id}`}</code></td><td>Read query state, or cancel active work and drop its stored result.</td></tr>
      <tr><td>GET</td><td><code>/v1/cluster</code></td><td>Inspect the coordinator and active workers.</td></tr>
      <tr><td>GET</td><td><code>/v1/catalog</code></td><td>List registered catalogs.</td></tr>
      <tr><td>GET / POST</td><td><code>/v1/catalog/definitions</code></td><td>List or create durable catalog definitions; mutation requires the catalog-admin bearer token.</td></tr>
      <tr><td>GET / PUT / DELETE</td><td><code>/v1/catalog/definitions/{`{catalog_id}`}</code></td><td>Read, revision-replace, or delete a definition.</td></tr>
      <tr><td>GET</td><td><code>/v1/catalog/{`{catalog}`}/schema</code></td><td>List schemas in a catalog.</td></tr>
      <tr><td>GET</td><td><code>/v1/catalog/{`{catalog}`}/schema/{`{schema}`}/table</code></td><td>List tables in a schema.</td></tr>
      <tr><td>GET</td><td><code>/health</code>, <code>/ready</code></td><td>Liveness and readiness.</td></tr>
    </tbody></table>
    <Code lang="bash">{`curl -s localhost:8080/v1/statement \\
  -H 'content-type: application/json' \\
  -d '{"sql":"SELECT count(*) FROM warehouse.default.orders"}'`}</Code>
    <Callout type="warn">
      Internal task/exchange routes and catalog mutations carry bearer tokens, but{" "}
      <code>/v1/statement</code> is not an end-user security boundary. Anyone who can reach the port can run
      SQL against every registered catalog.
    </Callout>

    <h2>Read next</h2>
    <ul>
      <li><a href="/docs/engine/architecture">Architecture</a> — processes, crates, and startup.</li>
      <li><a href="/docs/engine/sql">Engine SQL</a> — semantics and integration gates.</li>
      <li><a href="/docs/engine/distributed">Distributed Runtime</a> — stages, fragments, exchange, retry.</li>
      <li><a href="/docs/engine/storage">Storage &amp; Catalogs</a> — reads and native metadata.</li>
      <li><a href="/docs/memory">Engine Memory</a> — reservations, spill, and safety boundaries.</li>
    </ul>

    <Pager prev={{ href: "/docs/connectors", title: "Connector Matrix" }} next={{ href: "/docs/engine/architecture", title: "Engine Architecture" }} />
  </div>;
}
