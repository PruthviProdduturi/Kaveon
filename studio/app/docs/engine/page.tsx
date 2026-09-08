import { Callout, Code, Diagram, PageHeader, Pager } from "../../../components/docs/prose";

export const metadata = { title: "Kaveon Engine" };

export default function EngineDocs() {
  return <div className="docs-prose">
    <PageHeader
      eyebrow="Engine"
      title="Kaveon Engine"
      lead="A vectorized columnar query engine in Rust: its own SQL parser, planner, optimizer, catalog, and distributed runtime, reading Parquet and Delta directly over Arrow."
    />

    <Callout type="warn">
      <strong>Alpha.</strong> Engine is not yet Studio&rsquo;s execution backend — queries you run in the UI do
      not automatically go through it. An opt-in platform bridge delegates authenticated queries and
      catalog synchronization. Engine now supports authenticated principals, TLS, queued resource groups,
      and partitioned aggregate/join spill. Live cloud qualification, broader SQL coverage, and sustained
      production-scale performance remain open gates.
    </Callout>

    <Diagram
      src="/docs/architecture/kaveon-engine-pipeline.svg"
      alt="Kaveon Engine coordinator, distributed vectorized execution, exchange, catalog, and lake-read pipeline"
      caption="The coordinator pins versioned fragments and table snapshots; workers execute Arrow batches and exchange partitions. Cloud deployment qualification and advanced optimization remain targets."
    />

    <h2>Why it exists</h2>
    <p>
      Most analytics products wrap an engine someone else wrote — DuckDB embedded, Trino deployed alongside,
      or SQL pushed down to whatever the customer already runs. Kaveon Engine is first-party so that
      execution, storage access, and the semantic layer can be designed against each other rather than
      negotiated across a boundary. It embeds no other engine.
    </p>

    <h2>Install</h2>
    <Code lang="bash">{`# Linux x64 / Apple Silicon macOS — installs to ~/.local/bin
curl -sSf https://raw.githubusercontent.com/PruthviProdduturi/Kaveon/dev/scripts/install.sh | sh

# Windows x64 (PowerShell; no source clone required)
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
    <p>
      The installed client connects to a coordinator by default. With Microsoft authentication enabled,
      <code>--auth auto</code> reuses Azure CLI login when possible, then falls back to Microsoft device
      sign-in. Use <code>--auth azure-cli</code> to require Azure CLI or <code>--auth microsoft</code> to
      choose device sign-in. The UI identifies the client as Kaveon CLI and shows the signed Entra username;
      immutable Entra object identity remains the ownership key.
    </p>
    <Code lang="bash">{`kaveon --server https://engine.example.com --catalog medallion --schema test

# AKS port-forward with the deployment CA
kubectl -n kaveon port-forward service/kaveon 18443:8080 --address 127.0.0.1
kaveon --server https://localhost:18443 --ca-cert ./kaveon-ca.crt --catalog medallion --schema test`}</Code>
    <p>
      See the <a href="https://github.com/PruthviProdduturi/Kaveon/blob/dev/docs/engineering/azure-deployment-guide.md">Azure deployment guide</a> for
      certificate handling and the full connection procedure.
    </p>
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
      <code>/v1/statement</code>. Remote CLI metadata supports <code>IN</code>/<code>FROM</code> targets and
      validates <code>USE</code> before updating its prompt. It does not add SQL <code>SHOW</code> or
      <code>USE</code> support to the server.
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
            <code>HAVING</code> · <code>SUM</code>/<code>COUNT</code>/<code>AVG</code>/<code>MIN</code>/<code>MAX</code> ·{" "}
            <code>COUNT/SUM/AVG(DISTINCT)</code> · <code>ORDER BY</code> with null placement ·{" "}
            <code>LIMIT</code>/TopN · equi and cross joins · window functions · set operations ·{" "}
            <code>CASE</code> · date/time functions · <code>CAST</code> · arithmetic ·{" "}
            <code>catalog.schema.table</code>
          </td>
          <td>
            Scalar and correlated subqueries · non-equality join conditions · DDL and DML ·
            comprehensive decimal and date/time edge cases
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
  -d '{"query":"SELECT count(*) FROM warehouse.default.orders"}'`}</Code>
    <Callout type="note">
      Production Engine deployments use TLS and authenticated principals. Optional Entra delegated sign-in
      maps approved object IDs to Engine roles; query ownership remains tied to that immutable identity.
      Static and bridge credentials are also supported for their configured boundaries. The explicit
      <code> insecure_development </code> setting is for local development only and must not be exposed as a
      production access path.
    </Callout>

    <h2>Read next</h2>
    <ul>
      <li><a href="/docs/engine/architecture">Architecture</a> — processes, crates, and startup.</li>
      <li><a href="/docs/engine/sql">Engine SQL</a> — semantics and integration gates.</li>
      <li><a href="/docs/engine/distributed">Distributed Runtime</a> — stages, fragments, exchange, retry.</li>
      <li><a href="/docs/engine/storage">Storage &amp; Catalogs</a> — reads and native metadata.</li>
      <li><a href="/docs/memory">Engine Memory</a> — reservations, spill, and safety boundaries.</li>
    </ul>

    <Pager prev={{ href: "/docs/freshness", title: "Freshness Algorithm" }} next={{ href: "/docs/engine/architecture", title: "Engine Architecture" }} />
  </div>;
}
