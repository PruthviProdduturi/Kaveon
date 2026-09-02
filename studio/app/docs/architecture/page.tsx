import { Callout, Code, Diagram, PageHeader, Pager } from "../../../components/docs/prose";

export const metadata = { title: "Architecture" };

export default function ArchitectureDocs() {
  return (
    <div className="docs-prose">
      <PageHeader eyebrow="Platform" title="Architecture" lead="Kaveon is one product with three pillars: Studio, the deterministic Data Language Model, and the Rust analytical Engine. The shipping application and alpha Engine are separate runtimes today; their unified control plane is the target architecture." />

      <Callout type="note"><strong>Status vocabulary:</strong> Current means available in the shipping Studio/API path. Alpha means runnable but not production-integrated. Target means approved architecture that is not yet implemented.</Callout>

      <Diagram src="/docs/architecture/kaveon-platform-architecture.svg" alt="Kaveon platform architecture showing Studio, DLM, and Engine with current and target boundaries" caption="The product boundary includes all three pillars. Solid connections are current; explicitly labeled target connections are roadmap architecture. Open the diagram for a full-size view." />

      <h2>Runtime boundaries</h2>
      <table>
        <thead><tr><th>Component</th><th>Runtime</th><th>Maturity</th><th>Responsibility</th></tr></thead>
        <tbody>
          <tr><td><strong>Kaveon Studio</strong></td><td>Next.js 15 · React 19</td><td>Current</td><td>Ask, SQL Lab, semantic datasets, charts, dashboards, and administration.</td></tr>
          <tr><td><strong>Platform API + DLM</strong></td><td>FastAPI · Python</td><td>Current</td><td>Authenticated application services, metadata, deterministic resolution, and registered SQL-source execution.</td></tr>
          <tr><td><strong>Kaveon Engine</strong></td><td>Rust · Arrow · Parquet</td><td>Alpha</td><td>Local Parquet catalog resolution, SQL planning, and vectorized batch execution through separate CLI and HTTP entry points.</td></tr>
        </tbody>
      </table>

      <h2>Shipping application path</h2>
      <Code lang="text">{`Browser
  │ same-origin Auth.js session
  ▼
Kaveon Studio (Vercel)
  │ /api/kaveon/* proxy · authenticated identity headers
  ▼
Platform API + DLM (FastAPI)
  ├─ metadata/context database
  └─ one selected registered SQL source per query`}</Code>
      <p>The browser does not send trusted identity headers directly. Studio derives identity from the server-side session and signs the proxy request with <code>KAVEON_PROXY_SECRET</code>. FastAPI can also validate configured provider-issued bearer tokens for direct API clients. Each current query runs against one selected source; cross-source federation is not implemented.</p>

      <h2>Engine alpha path</h2>
      <Code lang="text">{`CLI or Engine HTTP client
  ▼
catalog.schema.table resolution
  ▼
SQL parser → logical plan → physical BatchOperator pipeline
  ▼
local Parquet reader → Arrow RecordBatch results`}</Code>
      <p>The Engine currently supports local Parquet with strict projection and conservative row-group pruning. It is not called by Studio or FastAPI. The Docker coordinator and workers exchange discovery heartbeats, but query fragments, shuffle, retry, cancellation, ADLS Gen2, S3, Delta Lake, and Iceberg remain target work.</p>
      <Callout type="warn">The alpha Engine HTTP service has no authentication or TLS. Keep it on a trusted local network boundary. See <a href="/docs/engine">Engine</a> and <a href="/docs/operations">Operations</a> before running it.</Callout>

      <h2>Data and control planes</h2>
      <ul>
        <li><strong>Metadata/context plane:</strong> datasets, charts, dashboards, roles, history, DLM artifacts, and configuration.</li>
        <li><strong>Registered SQL data plane:</strong> the selected Fabric SQL, Azure SQL, PostgreSQL, MySQL, or StarRocks source used by the current API path.</li>
        <li><strong>Live Lake Path:</strong> customer-controlled Parquet today in the standalone Engine; cloud object stores and table formats are target capabilities.</li>
      </ul>

      <h2>Architectural invariants</h2>
      <ul>
        <li>Identity is established at a verified trust boundary; raw client identity headers are never authoritative.</li>
        <li>Optimization may read extra data but must never omit qualifying rows.</li>
        <li>DLM acceleration complements Engine performance; it is not a substitute for a fast compute path.</li>
        <li>Performance claims must name the data, hardware, version, cache state, concurrency, and date.</li>
        <li>Current, alpha, and target behavior remain visibly distinct in product documentation.</li>
      </ul>

      <Pager prev={{ href: "/docs/data-sources", title: "Data Sources" }} next={{ href: "/docs/engine", title: "Kaveon Engine" }} />
    </div>
  );
}
