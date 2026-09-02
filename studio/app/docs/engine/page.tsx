import { Callout, Code, Diagram, PageHeader, Pager } from "../../../components/docs/prose";

export const metadata = { title: "Kaveon Engine" };

export default function EngineDocs() {
  return <div className="docs-prose">
    <PageHeader eyebrow="Build & Operate" title="Kaveon Engine" lead="Kaveon's independent Rust query engine provides SQL planning and vectorized Arrow execution over local Parquet, with CLI and HTTP entry points." />
    <Callout type="warn"><strong>Alpha boundary:</strong> Engine is not yet Studio&rsquo;s execution backend. Cloud object stores, Delta/Iceberg readers, distributed fragments, HTTP authentication, and TLS are target capabilities—not shipped behavior.</Callout>
    <Diagram src="/docs/architecture/kaveon-engine-pipeline.svg" alt="Current and target stages in the Kaveon Engine vectorized query pipeline" caption="The solid bypass is today&rsquo;s parser-to-physical-plan path. Filter pushdown, Sort/TopN, cloud storage, and lakehouse table formats are labeled target capabilities." />

    <h2>What ships today</h2>
    <ul>
      <li>SQL parsing, logical and physical planning, and Arrow <code>RecordBatch</code> execution.</li>
      <li>Direct reads from local, single-file Parquet tables registered in a catalog.</li>
      <li>A command-line client and an Axum HTTP server with coordinator/worker discovery.</li>
      <li>A lightweight dashboard at <code>/ui</code> for node status.</li>
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
      <tr><td>GET / DELETE</td><td><code>/v1/query/{`{query_id}`}</code></td><td>Read or remove a stored result; deletion does not cancel running synchronous work.</td></tr>
      <tr><td>GET</td><td><code>/v1/cluster</code></td><td>Inspect coordinator and active workers.</td></tr>
      <tr><td>GET</td><td><code>/v1/catalog</code></td><td>List registered catalogs.</td></tr>
      <tr><td>GET</td><td><code>/v1/catalog/{`{catalog}`}/schema</code></td><td>List schemas in a catalog.</td></tr>
      <tr><td>GET</td><td><code>/v1/catalog/{`{catalog}`}/schema/{`{schema}`}/table</code></td><td>List tables in a schema.</td></tr>
      <tr><td>GET</td><td><code>/health</code>, <code>/ready</code></td><td>Liveness and readiness probes.</td></tr>
    </tbody></table>
    <Callout type="note">Keep the alpha HTTP server behind a trusted local boundary. See the repository <code>SECURITY.md</code> and <code>STATUS.md</code> before exposing any endpoint.</Callout>
    <Pager prev={{ href: "/docs/architecture", title: "Architecture" }} next={{ href: "/docs/api-reference", title: "API Reference" }} />
  </div>;
}
