import { PageHeader, Callout, Code, Diagram, Pager } from "../../../components/docs/prose";

export const metadata = { title: "Deployment" };

export default function DeploymentDocs() {
  return (
    <div className="docs-prose">
      <PageHeader
        eyebrow="Deploy &amp; Operate"
        title="Deployment"
        lead="Studio runs on Vercel. Everything else — the API, the Kaveon Engine and its workers — runs on one Azure VM, and reads Delta tables from ADLS Gen2. The Engine is the query path, not a parallel experiment."
      />

      <h2>Topology</h2>
      <Diagram
        src="/docs/architecture/kaveon-deployment-topology.svg"
        alt="Browser to Studio on Vercel, through a same-origin proxy to the API on kaveon-vm, into the Kaveon Engine coordinator and two workers, reading Delta tables in ADLS Gen2"
        caption="One request path. The platform's own records live in KaveonDB, the Engine's built-in transactional catalog."
      />
      <Code lang="text">{`Browser ──► Vercel  (Kaveon Studio · Auth.js: GitHub / Google / Microsoft)
               │  same-origin /api/kaveon proxy (injects X-User-* + secret)
               ▼
            kaveon-vm  ── Caddy (TLS) ──► API + DLM (FastAPI)
               │
               ├──► Kaveon Engine coordinator ──► worker-1, worker-2
               │         planning, cube and statistics   fragment execution
               │                                         Arrow exchange
               ▼
            ADLS Gen2  (kaveonlake / opensource)
               ├── Delta tables — the rows
               └── kaveon.product.*  (KaveonDB — the platform's own records)`}</Code>
      <p>
        The browser only talks to Vercel. The proxy forwards to the API with <code>X-User-*</code> headers stamped by{" "}
        <code>KAVEON_PROXY_SECRET</code>, which the API validates (see <a href="/docs/auth">Auth &amp; RBAC</a>). The
        Engine&rsquo;s own ports are bound to localhost on the VM and are never reachable from outside it.
      </p>

      <Callout type="note">
        <strong>There is no PostgreSQL.</strong> Kaveon used to keep its control plane in{" "}
        <code>kaveonmeta</code> and its warehouse in <code>kaveon</code> on an Azure Database for PostgreSQL Flexible
        Server. Both are retired. Datasets, charts, dashboards, saved statements, chat history and audit entries are now
        transactional rows in <code>kaveon.product.*</code>, written only through KaveonDB&rsquo;s transaction boundary;
        the data itself is Delta in object storage. A legacy database passthrough still exists in the code and answers{" "}
        <code>503</code> — it is a signpost, not a path.
      </Callout>

      <h2>Where a query actually runs</h2>
      <p>
        A chart or a question reaches the Engine, and the Engine decides between two lanes. A cube-shaped aggregate is
        answered from precomputed cells and HyperLogLog sketches without reading the table; anything else is planned
        into fragments and executed across the workers with Arrow exchange, retry and cancellation. The Engine reports
        which lane it took on every statement, and Studio shows it. See <a href="/docs/engine">Kaveon Engine</a> for the
        execution model and <a href="/docs/freshness">Freshness</a> for when a precomputed answer is allowed to stand.
      </p>

      <h2>CI/CD</h2>
      <p>
        <code>.github/workflows/ci.yml</code> runs on every push and PR to <code>dev</code>: type-check and build Studio,
        run the API test suite, scan for secrets, and validate the documentation. The documentation gate fails closed —
        a new Engine setting without a row in <a href="/docs/engine">the settings table</a> turns CI red and holds the
        Studio deploy with it. On a green run, Studio deploys to Vercel.
      </p>
      <p>
        The API ships by rebuilding its image on the VM from the checked-out commit, not by rolling a registry tag.{" "}
        <code>.github/workflows/deploy.yml</code> still describes the Azure Container Apps rollout and is{" "}
        <strong>manual-only</strong>: the container app it targeted was deleted once the VM became the deployment, and a
        workflow that always fails hides the one that matters.
      </p>

      <h2>Running the whole platform locally</h2>
      <p>
        The root <code>docker-compose.yml</code> starts Studio, the API and DLM, an Engine coordinator and two workers.
        Point <code>KAVEON_DATA_PATH</code> at a directory of Delta or Parquet tables before starting; the workers mount
        it read-only.
      </p>
      <Code lang="powershell">{`$env:KAVEON_DATA_PATH = 'F:\\kaveon-data'
docker compose up --build

# Studio:   http://localhost:3000
# API:      http://localhost:8080
# Engine:   http://localhost:8081/ui`}</Code>
      <p>
        <code>docker-compose.workers.yml</code> adds two more workers. It exists for one reason: the statistics and cube
        pass gives each worker a single task over all of its files, and that task has a hard 600&nbsp;second ceiling, so
        a large table can only be cubed by putting fewer files in each task. Use it to build, then take the extra
        workers down — see the warning in that file before serving from it.
      </p>

      <h2>Key environment variables</h2>
      <table>
        <thead><tr><th>Where</th><th>Vars</th></tr></thead>
        <tbody>
          <tr><td>Both tiers</td><td><code>KAVEON_PROXY_SECRET</code> (must match)</td></tr>
          <tr><td>Studio (Vercel)</td><td><code>AUTH_SECRET</code>, <code>AUTH_URL</code>, provider IDs and secrets, <code>API_URL</code>, <code>AUTH_ADMIN_EMAILS</code></td></tr>
          <tr><td>API (kaveon-vm)</td><td><code>KAVEON_PROXY_SECRET</code>, <code>KAVEON_ENGINE_BRIDGE_TOKEN</code>, <code>KAVEON_ENGINE_CATALOG_TOKEN</code>, <code>KAVEON_CREDENTIAL_KEYS</code>, <code>KAVEON_CREDENTIAL_ACTIVE_KEY</code></td></tr>
          <tr><td>Engine (coordinator and workers)</td><td><code>KAVEON_DATA_DIR</code>, <code>KAVEON_EXCHANGE_TOKEN</code>, <code>KAVEON_DISCOVERY_URI</code>, memory and spill settings</td></tr>
        </tbody>
      </table>

      <h2>Production notes</h2>
      <p>
        The reference deployment is a small demo host, not a hardened one: a single burstable VM runs the API, the
        coordinator, both workers and the edge, and a cube build needs it temporarily resized. For production, separate
        the Engine from the API, put both on private networking, keep secrets in a managed store such as Key Vault, and
        prefer managed-identity auth for registered Fabric and Azure SQL sources over connection strings.
      </p>

      <Pager prev={{ href: "/docs/sql-compatibility", title: "SQL Compatibility" }} next={{ href: "/docs/auth", title: "Auth & RBAC" }} />
    </div>
  );
}
