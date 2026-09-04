import { Callout, Code, Diagram, PageHeader, Pager } from "../../../components/docs/prose";

export const metadata = { title: "Architecture" };

export default function ArchitectureDocs() {
  return (
    <div className="docs-prose">
      <PageHeader eyebrow="Platform" title="Architecture" lead="Kaveon is one product with three pillars: Studio, the deterministic Data Language Model, and the Rust analytical Engine. Studio and DLM are hosted today; the distributed Engine is an independently runnable alpha awaiting the authenticated platform bridge." />

      <Callout type="note"><strong>Status vocabulary:</strong> Current means available in the shipping Studio/API path. Alpha means runnable but not production-integrated. Target means approved architecture that is not yet implemented.</Callout>

      <Diagram src="/docs/architecture/kaveon-platform-architecture.svg" alt="Kaveon platform architecture showing Studio, DLM, and Engine with current and target boundaries" caption="The product boundary includes all three pillars. Solid connections are current; explicitly labeled target connections are roadmap architecture. Open the diagram for a full-size view." />

      <h2>Runtime boundaries</h2>
      <table>
        <thead><tr><th>Component</th><th>Runtime</th><th>Maturity</th><th>Responsibility</th></tr></thead>
        <tbody>
          <tr><td><strong>Kaveon Studio</strong></td><td>Next.js 15 · React 19</td><td>Current</td><td>Ask, SQL Lab, semantic datasets, charts, dashboards, and administration.</td></tr>
          <tr><td><strong>Platform API + DLM</strong></td><td>FastAPI · Python</td><td>Current</td><td>Authenticated application services, metadata, deterministic resolution, and registered SQL-source execution.</td></tr>
          <tr><td><strong>Kaveon Engine</strong></td><td>Rust · Arrow · Parquet/Delta</td><td>Alpha</td><td>Durable catalog resolution, optimized SQL plans, distributed vectorized stages, Arrow IPC exchanges, and direct local lake reads.</td></tr>
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

      <h2>Engine distributed alpha path</h2>
      <Code lang="text">{`Remote CLI or Engine HTTP client
  ▼
Coordinator: durable catalog → SQL → optimizer → stage graph
  ▼
Versioned fragments → worker tasks → Arrow IPC exchanges
  ▼
local Parquet / Delta splits → root Arrow result`}</Code>
      <p>The Engine executes distributed scans, partial/final aggregates, Sort/TopN, and repartitioned or broadcast joins. Retry, cancellation, exchange cleanup, and bounded Sort/TopN spill foundations are wired. It is not yet called by Studio or FastAPI. ADLS Gen2, S3, Iceberg, admission control, aggregate/join spill, and production qualification remain target work.</p>
      <Callout type="warn">Internal exchange and catalog-mutation routes have bearer tokens, but statement clients do not yet have end-user authentication, authorization, or TLS. Keep Engine behind a trusted network boundary.</Callout>

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
