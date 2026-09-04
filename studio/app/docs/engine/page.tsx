import { Callout, Code, Diagram, PageHeader, Pager } from "../../../components/docs/prose";

export const metadata = { title: "Kaveon Engine" };

export default function EngineDocs() {
  return <div className="docs-prose">
    <PageHeader eyebrow="Build & Operate" title="Kaveon Engine" lead="Kaveon's independent Rust analytical engine combines durable catalog resolution, SQL optimization, vectorized Arrow operators, and distributed stage execution over local Parquet and Delta." />
    <Callout type="warn"><strong>Alpha boundary:</strong> Engine is not yet Studio&rsquo;s execution backend. ADLS Gen2/S3 reads, broader Delta/Iceberg semantics, end-user HTTP authorization/TLS, admission control, and aggregate/join spill remain production gates.</Callout>
    <Diagram src="/docs/architecture/kaveon-engine-pipeline.svg" alt="Kaveon Engine coordinator, distributed vectorized execution, exchange, catalog, and lake-read pipeline" caption="The alpha runtime plans versioned fragments on the coordinator and executes Arrow batches across workers. Cloud storage and advanced optimizer/operational capabilities remain explicit targets." />

    <h2>What ships today</h2>
    <ul>
      <li>SQL parsing, logical and physical planning, and Arrow <code>RecordBatch</code> execution.</li>
      <li>Direct reads from local Parquet and Delta tables with projection, conservative predicate pushdown, and deterministic splits.</li>
      <li>Local and distributed filters, projections, aggregates, exact distinct counts, joins, Sort/TopN, and limits.</li>
      <li>A remote-first CLI and Axum coordinator with worker discovery, versioned fragments, authenticated Arrow IPC exchange, retry, and cancellation.</li>
      <li>A durable single-coordinator SQLite/WAL catalog and operational console at <code>/ui</code>.</li>
    </ul>

    <h2>Run locally</h2>
    <Code lang="bash">{`cargo run -p kaveon-cli -- --help
cargo run -p kaveon-server -- /path/to/config.toml
# Dashboard: http://localhost:8080/ui
# Readiness: http://localhost:8080/ready`}</Code>
    <p>Configuration can set node identity, environment, coordinator role, HTTP port, discovery URI, local data directory, and catalog directory. Corresponding <code>KAVEON_*</code> environment variables override TOML values.</p>

    <h2>HTTP surface</h2>
    <table><thead><tr><th>Method</th><th>Path</th><th>Purpose</th></tr></thead><tbody>
      <tr><td>POST</td><td><code>/v1/statement</code></td><td>Execute SQL on the coordinator.</td></tr>
      <tr><td>GET</td><td><code>/v1/query</code></td><td>List recent process-local query records.</td></tr>
      <tr><td>GET / DELETE</td><td><code>/v1/query/{`{query_id}`}</code></td><td>Read query state or cancel active distributed work and remove its stored result.</td></tr>
      <tr><td>GET</td><td><code>/v1/cluster</code></td><td>Inspect coordinator and active workers.</td></tr>
      <tr><td>GET</td><td><code>/v1/catalog</code></td><td>List registered catalogs.</td></tr>
      <tr><td>GET / POST</td><td><code>/v1/catalog/definitions</code></td><td>List or create durable catalog definitions; mutation requires the catalog-admin bearer token and actor.</td></tr>
      <tr><td>GET / PUT / DELETE</td><td><code>/v1/catalog/definitions/{`{catalog_id}`}</code></td><td>Read, revision-replace, or delete a durable catalog definition.</td></tr>
      <tr><td>GET</td><td><code>/v1/catalog/{`{catalog}`}/schema</code></td><td>List schemas in a catalog.</td></tr>
      <tr><td>GET</td><td><code>/v1/catalog/{`{catalog}`}/schema/{`{schema}`}/table</code></td><td>List tables in a schema.</td></tr>
      <tr><td>GET</td><td><code>/health</code>, <code>/ready</code></td><td>Liveness and readiness probes.</td></tr>
    </tbody></table>
    <Callout type="note">Internal task/exchange routes and catalog mutations have dedicated bearer tokens, but the statement API is not an end-user security boundary. Keep the alpha server private. See <code>SECURITY.md</code>, <code>STATUS.md</code>, and <code>engine/CATALOG.md</code>.</Callout>
    <Pager prev={{ href: "/docs/architecture", title: "Architecture" }} next={{ href: "/docs/api-reference", title: "API Reference" }} />
  </div>;
}
