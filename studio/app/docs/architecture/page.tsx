import { Callout, Code, Diagram, PageHeader, Pager } from "../../../components/docs/prose";

export const metadata = { title: "Architecture" };

export default function ArchitectureDocs() {
  return (
    <div className="docs-prose">
      <PageHeader eyebrow="Platform" title="Architecture" lead="Kaveon is one product with three pillars: Studio, the deterministic Data Language Model, and the Rust analytical Engine. Studio and the API provide the product surface; the Engine provides the distributed analytical path for lake-backed statements." />

      <Callout type="note"><strong>Status vocabulary:</strong> Current means it is in the request path people use. Alpha means the path is implemented and qualified, but its SQL and cloud-format coverage is still expanding. Target means approved architecture that is not yet implemented.</Callout>

      <Diagram src="/docs/architecture/kaveon-platform-architecture.svg" alt="Kaveon platform architecture showing Studio, DLM, and Engine with current and target boundaries" caption="The product boundary includes all three pillars. Solid connections are current; explicitly labeled target connections are roadmap architecture. Open the diagram for a full-size view." />

      <h2>Runtime boundaries</h2>
      <table>
        <thead><tr><th>Component</th><th>Runtime</th><th>Maturity</th><th>Responsibility</th></tr></thead>
        <tbody>
          <tr><td><strong>Kaveon Studio</strong></td><td>Next.js 15 · React 19</td><td>Current</td><td>Ask, SQL Lab, semantic datasets, charts, dashboards, and administration.</td></tr>
          <tr><td><strong>Platform API + DLM</strong></td><td>FastAPI · Python</td><td>Current</td><td>Authenticated application services, deterministic question resolution, and the statements it hands the Engine.</td></tr>
          <tr><td><strong>Kaveon Engine</strong></td><td>Rust · Arrow · Delta on ADLS Gen2</td><td>Alpha · qualified path</td><td>Durable catalog, optimized plans, distributed vectorized stages, Arrow IPC exchange, cube and sketch answers, and KaveonDB transactional records.</td></tr>
        </tbody>
      </table>

      <h2>The request path</h2>
      <Code lang="text">{`Browser
  │ same-origin Auth.js session
  ▼
Kaveon Studio (Vercel, AKS, or VM)
  │ /api/kaveon/* proxy · authenticated identity headers
  ▼
Platform API + DLM (FastAPI)
  │
  ├─ Kaveon Engine ──► Delta tables in ADLS Gen2
  │                    cube cells and sketches, or a distributed scan
  └─ KaveonDB (kaveon.product.*) ──► the platform's own records`}</Code>
      <p>The browser does not send trusted identity headers directly. Studio derives identity from the server-side session and signs the proxy request with <code>KAVEON_PROXY_SECRET</code>. FastAPI can also validate configured provider-issued bearer tokens for direct API clients. Each query runs against one selected source; cross-source federation is not implemented.</p>

      <h2>How a statement executes</h2>
      <Code lang="text">{`Remote CLI or Engine HTTP client
  ▼
Coordinator: durable catalog → SQL → optimizer → stage graph
  ▼
Versioned fragments → worker tasks → Arrow IPC exchanges
  ▼
local Parquet / Delta splits → root Arrow result`}</Code>
      <p>The Engine executes distributed scans, partial and final aggregates, Sort/TopN, and repartitioned or broadcast joins, with retry, cancellation, exchange cleanup and bounded Sort/TopN spill. A cube-shaped aggregate skips execution entirely and is answered from precomputed cells and HyperLogLog sketches; the record says which lane a statement took. Delta on <strong>ADLS Gen2 is in production use</strong>. S3, Iceberg, aggregate and join spill, and engine HTTP authentication with TLS remain target work.</p>
      <Callout type="warn">Internal exchange and catalog-mutation routes carry bearer tokens, but statement clients still have no end-user authentication, authorization or TLS of their own. In the reference deployment the Engine&rsquo;s ports are bound to localhost and only the API reaches them. Keep it behind a trusted boundary.</Callout>

      <h2>Data and control planes</h2>
      <ul>
        <li><strong>Control plane:</strong> datasets, charts, dashboards, roles, history, DLM artifacts and configuration, held as transactional rows in <code>kaveon.product.*</code> and written only through KaveonDB&rsquo;s transaction boundary.</li>
        <li><strong>Lake data plane:</strong> Delta tables in ADLS Gen2, read by the Engine. This is where a query&rsquo;s rows come from.</li>
        <li><strong>Registered SQL data plane:</strong> a Fabric SQL, Azure SQL, PostgreSQL, MySQL or StarRocks source a tenant registers. One source per query; cross-source federation is not implemented.</li>
      </ul>

      <h2>Architectural invariants</h2>
      <ul>
        <li>Identity is established at a verified trust boundary; raw client identity headers are never authoritative.</li>
        <li>Optimization may read extra data but must never omit qualifying rows.</li>
        <li>DLM acceleration complements Engine performance; it is not a substitute for a fast compute path.</li>
        <li>Performance claims must name the data, hardware, version, cache state, concurrency, and date.</li>
        <li>Current and target behaviour remain visibly distinct in product documentation.</li>
      </ul>

      <Pager prev={{ href: "/docs/concepts", title: "Core Concepts" }} next={{ href: "/docs/api-reference", title: "API Reference" }} />
    </div>
  );
}
