import { Callout, Code, PageHeader, Pager } from "../../../components/docs/prose";

export const metadata = { title: "API Reference" };

export default function ApiReferenceDocs() {
  return <div className="docs-prose">
    <PageHeader
      eyebrow="Platform"
      title="API reference"
      lead="Kaveon exposes two independent HTTP surfaces: the FastAPI platform API that Studio calls, and the standalone Rust Engine API. They have different base paths, different authentication, and different maturity."
    />

    <Callout type="note">
      The deployed OpenAPI document is the precise contract for your running version. This page covers the
      shape of each surface, how authentication works, and the endpoints you are most likely to call.
    </Callout>

    <h2>Which surface</h2>
    <table><thead><tr><th>Surface</th><th>Base path</th><th>Use for</th><th>Maturity</th></tr></thead><tbody>
      <tr><td>Studio proxy</td><td><code>/api/kaveon/*</code></td><td>Browser and user-facing clients</td><td>Current</td></tr>
      <tr><td>Platform API</td><td><code>/api/v1/*</code></td><td>Server-side integrations behind the trusted boundary</td><td>Current</td></tr>
      <tr><td>Engine HTTP</td><td><code>/v1/*</code></td><td>SQL, catalog, cluster, and query operations</td><td>Alpha</td></tr>
    </tbody></table>

    <h2>Authentication</h2>
    <p>
      The browser never sends trusted identity to the platform API. Studio resolves the session server-side,
      then its same-origin proxy stamps <code>X-User-*</code> headers and seals the request with{" "}
      <code>KAVEON_PROXY_SECRET</code>, which must match on Studio and the API. The API rejects identity
      headers that do not arrive with the matching secret.
    </p>
    <Code lang="bash">{`# From a browser or user-facing client — go through the proxy, send no identity
curl -s https://your-studio-host/api/kaveon/api/v1/datasets

# Server-side, behind the trust boundary — supply the proxy secret yourself
curl -s http://kaveon-api:8080/api/v1/datasets \\
  -H "x-proxy-secret: $KAVEON_PROXY_SECRET" \\
  -H "x-user-email: analyst@example.com" \\
  -H "x-user-role: Analyst"`}</Code>
    <Callout type="warn">
      Never expose the platform API directly to untrusted networks with identity headers enabled. Anything
      that can reach it and knows the secret can assert any identity and role.
    </Callout>

    <h2>Roles on the API</h2>
    <p>
      Endpoints enforce a minimum role of <code>Viewer → Analyst → Editor → Admin</code>. Query execution is
      the one with a notable exception: a Viewer may execute only from a dashboard or filter context, and
      only a single read-only <code>SELECT</code> — no DDL, no DML, no stacked statements. Everything else
      requires Analyst.
    </p>

    <h2>Running SQL</h2>
    <table><thead><tr><th>Method</th><th>Path</th><th>Purpose</th></tr></thead><tbody>
      <tr><td>POST</td><td><code>/api/v1/sql/execute</code></td><td>Execute synchronously.</td></tr>
      <tr><td>POST</td><td><code>/api/v1/sql/execute-async</code></td><td>Submit a job; returns a job id.</td></tr>
      <tr><td>GET / DELETE</td><td><code>/api/v1/sql/async/{`{job_id}`}</code></td><td>Poll for results, or cancel.</td></tr>
      <tr><td>POST</td><td><code>/api/v1/sql/generate</code></td><td>Generate SQL from a dataset and chart definition.</td></tr>
      <tr><td>DELETE</td><td><code>/api/v1/sql/cache</code></td><td>Invalidate cached results.</td></tr>
      <tr><td>POST</td><td><code>/api/v1/lab/query</code></td><td>SQL Lab execution — cancellable, recorded in history.</td></tr>
      <tr><td>POST</td><td><code>/api/v1/lab/ctas</code></td><td>Create a table from a query.</td></tr>
      <tr><td>GET</td><td><code>/api/v1/lab/query-history</code></td><td>Your own query history.</td></tr>
    </tbody></table>
    <Code lang="bash">{`curl -s $API/api/v1/sql/execute \\
  -H 'content-type: application/json' \\
  -d '{
    "sql_text":  "SELECT region, SUM(total) AS revenue FROM orders GROUP BY region",
    "database":  "kaveon",
    "source":    "lab",
    "row_limit": 1000,
    "use_cache": true,
    "cache_ttl": 300
  }'`}</Code>
    <p>
      <code>sql_text</code> and <code>database</code> are required. <code>row_limit</code> accepts 1–5000 and{" "}
      <code>cache_ttl</code> 30–3600 seconds; caching is off unless you ask for it. Results are keyed by a
      SHA of the query, so an identical request inside the TTL returns without touching the source.
    </p>

    <h2>Asking questions — DLM</h2>
    <table><thead><tr><th>Method</th><th>Path</th><th>Purpose</th></tr></thead><tbody>
      <tr><td>POST</td><td><code>/api/v1/dlm/ask</code></td><td>Resolve a natural-language question deterministically.</td></tr>
      <tr><td>POST</td><td><code>/api/v1/dlm/serve-chart</code></td><td>Answer directly as chart-ready series.</td></tr>
      <tr><td>GET</td><td><code>/api/v1/dlm/route</code></td><td>See which dataset a question routes to.</td></tr>
      <tr><td>GET</td><td><code>/api/v1/dlm/coverage</code></td><td>What the compiled context can answer.</td></tr>
      <tr><td>GET</td><td><code>/api/v1/dlm/filter-values</code></td><td>Dimension values from context, no source scan.</td></tr>
      <tr><td>POST</td><td><code>/api/v1/datasets/{`{id}`}/dlm/generate</code></td><td>Compile the dataset context.</td></tr>
      <tr><td>GET / PUT</td><td><code>/api/v1/datasets/{`{id}`}/dlm/context</code></td><td>Read or curate the context spec.</td></tr>
      <tr><td>GET</td><td><code>/api/v1/datasets/{`{id}`}/freshness</code></td><td>Staleness of the compiled context.</td></tr>
    </tbody></table>
    <Code lang="bash">{`curl -s $API/api/v1/dlm/ask \\
  -H 'content-type: application/json' \\
  -d '{"question":"revenue by region","limit":50}'`}</Code>
    <p>
      The response reports the routed dataset, the assembled SQL, chart hints, and which path answered.{" "}
      <code>ok: false</code> means nothing matched — the DLM declines rather than guessing, which is the
      point of a deterministic resolver. Asking against a stale dataset also triggers a background rebuild.
    </p>

    <h2>Content and configuration</h2>
    <p>
      Datasets, charts, dashboards, and data sources follow the same REST shape — <code>GET</code> to list,{" "}
      <code>GET /{`{id}`}</code> to read, <code>POST</code> to create, <code>PUT</code> or{" "}
      <code>PATCH</code> to update, <code>DELETE</code> to remove, plus a <code>/summary</code> listing and a{" "}
      <code>/favorite</code> toggle.
    </p>
    <table><thead><tr><th>Domain</th><th>Base</th><th>Notable</th></tr></thead><tbody>
      <tr><td>Datasets</td><td><code>/api/v1/datasets</code></td><td><code>/{`{id}`}/columns</code></td></tr>
      <tr><td>Charts</td><td><code>/api/v1/charts</code></td><td>—</td></tr>
      <tr><td>Dashboards</td><td><code>/api/v1/dashboards</code></td><td>string ids, not integers</td></tr>
      <tr><td>Data sources</td><td><code>/api/v1/data-sources</code></td><td><code>/{`{id}`}/test</code>, <code>/{`{id}`}/table-count</code></td></tr>
      <tr><td>Health</td><td><code>/api/health</code></td><td>unauthenticated probe</td></tr>
    </tbody></table>
    <Callout type="note">
      Connection strings are never returned in data-source responses. They are still stored unencrypted in
      the metadata table — see <a href="/docs/connectors">Connector Matrix</a>.
    </Callout>

    <h2>Errors</h2>
    <p>Errors carry a stable machine-readable code alongside a human-readable message:</p>
    <Code lang="json">{`{ "detail": { "code": "forbidden", "message": "Analyst role required to execute queries." } }`}</Code>
    <table><thead><tr><th>Status</th><th>Means</th></tr></thead><tbody>
      <tr><td>401</td><td>No usable identity — the proxy secret is missing or does not match.</td></tr>
      <tr><td>403</td><td>Authenticated, but the role or visibility rule denies it.</td></tr>
      <tr><td>429</td><td>Rate limited — 120 SQL executions per user per minute.</td></tr>
    </tbody></table>

    <h2>Engine HTTP</h2>
    <p>
      A separate service with its own base path. The statement body takes <code>query</code>, optionally
      scoped by <code>catalog</code> and <code>schema</code>:
    </p>
    <Code lang="bash">{`curl -s localhost:8080/v1/statement \\
  -H 'content-type: application/json' \\
  -d '{"query":"SELECT count(*) FROM warehouse.default.orders"}'`}</Code>
    <p>
      Responses carry a query id, state, columns, rows, and elapsed time. Results are materialized in
      process memory and history is process-local, so do not assume durability or retention across restarts.
      Full endpoint list: <a href="/docs/engine">Kaveon Engine</a>.
    </p>

    <h2>Compatibility</h2>
    <ul>
      <li>Pin clients to a deployed version; the API is pre-1.0 and response fields may change.</li>
      <li>Route user-facing traffic through the Studio proxy, never directly at the platform API.</li>
      <li>Keep the Engine API on a trusted network during alpha.</li>
    </ul>

    <Pager prev={{ href: "/docs/architecture", title: "Architecture" }} next={{ href: "/docs/connectors", title: "Connector Matrix" }} />
  </div>;
}
