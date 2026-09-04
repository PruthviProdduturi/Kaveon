import { Callout, PageHeader, Pager } from "../../../components/docs/prose";

export const metadata = { title: "API Reference" };

export default function ApiReferenceDocs() {
  return <div className="docs-prose">
    <PageHeader eyebrow="Build & Operate" title="API reference" lead="Kaveon has two distinct HTTP surfaces: the FastAPI platform API used by Studio and the standalone Rust Engine API." />
    <Callout type="note">Do not send browser-created identity headers directly to FastAPI. Studio&rsquo;s same-origin proxy establishes identity server-side and seals trusted headers with <code>KAVEON_PROXY_SECRET</code>.</Callout>

    <h2>Choose the right surface</h2>
    <table><thead><tr><th>Surface</th><th>Use it for</th><th>Ingress</th></tr></thead><tbody>
      <tr><td>Studio proxy</td><td>Datasets, charts, dashboards, data sources, SQL Lab, and DLM</td><td><code>/api/kaveon/*</code></td></tr>
      <tr><td>FastAPI</td><td>Platform services called through the trusted proxy</td><td>Private backend deployment</td></tr>
      <tr><td>Engine HTTP</td><td>Alpha SQL, catalog, query-result, and cluster operations</td><td><code>/v1/*</code></td></tr>
    </tbody></table>

    <h2>Platform domains</h2>
    <ul>
      <li><strong>SQL Lab</strong> — synchronous helpers, cancellable query jobs, history, and saved queries.</li>
      <li><strong>Semantic layer</strong> — data sources, datasets, metrics, dimensions, and metadata.</li>
      <li><strong>Presentation</strong> — charts, dashboards, visibility, and sharing.</li>
      <li><strong>DLM</strong> — encode context, ask questions, serve charts, inspect freshness, and rebuild.</li>
    </ul>
    <p>Domain-specific request examples live in the SQL Lab, DLM, datasets, and authentication guides. Treat deployed OpenAPI output as the precise contract for the running FastAPI version.</p>

    <h2>Engine conventions</h2>
    <p>Engine statement requests use JSON with a <code>query</code> string. Successful responses include an ID, state, columns, data, and elapsed time. Errors include a stable code and human-readable message. The Engine currently materializes results in process memory; consumers must not assume durability or unbounded result retention.</p>

    <h2>Compatibility and security</h2>
    <ul>
      <li>Pin clients to the deployed Kaveon version and verify response fields before upgrading.</li>
      <li>Use the Studio proxy for user-facing applications; never trust identity headers supplied by a browser.</li>
      <li>Keep Engine HTTP private during alpha. Internal exchange and catalog-mutation tokens do not replace end-user statement authentication, authorization, or TLS termination.</li>
    </ul>
    <Pager prev={{ href: "/docs/engine", title: "Kaveon Engine" }} next={{ href: "/docs/auth", title: "Auth & RBAC" }} />
  </div>;
}
