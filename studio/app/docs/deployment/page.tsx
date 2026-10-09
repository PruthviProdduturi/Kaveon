import { PageHeader, Callout, Code, Diagram, Pager } from "../../../components/docs/prose";

export const metadata = { title: "Deployment" };

export default function DeploymentDocs() {
  return (
    <div className="docs-prose">
      <PageHeader
        eyebrow="Deploy &amp; Operate"
        title="Deployment"
        lead="Studio runs on Vercel. Everything else — the API, the Kaveon Engine and its workers — runs on one Azure VM and reads Delta tables from ADLS Gen2. The Engine is the query path, not a parallel experiment."
      />

      <h2>Topology</h2>
      <Diagram
        src="/docs/architecture/kaveon-deployment-topology.svg"
        alt="Browser to Studio on Vercel, through a same-origin proxy to the API on kaveon-vm, into the Kaveon Engine coordinator and two workers, reading Delta tables in ADLS Gen2 while the platform's own records are written to a separate container"
        caption="One request path. Table data and the platform's own records are separate containers in the same storage account."
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
            ADLS Gen2 · kaveonlake
               ├── opensource/snapshots/…     Delta tables — the rows
               └── product/kaveon/system/v2   KaveonDB — the platform's records`}</Code>
      <p>
        The browser only talks to Vercel. The proxy forwards to the API with <code>X-User-*</code> headers stamped by{" "}
        <code>KAVEON_PROXY_SECRET</code>, which the API validates (see <a href="/docs/auth">Auth &amp; RBAC</a>). The
        Engine&rsquo;s own ports are bound to localhost on the VM and are never reachable from outside it.
      </p>
      <p>
        The reference host is a <code>Standard_B2als_v2</code> — two vCPUs and 3&nbsp;GB — in <code>westus2</code>,
        running the repository&rsquo;s own <code>docker-compose.yml</code>: the coordinator, two workers, the API,
        Studio and Caddy. Datasets, charts, dashboards, saved statements, chat history and audit entries are
        transactional rows in <code>kaveon.product.*</code>, written only through KaveonDB&rsquo;s transaction
        boundary; the table data itself is Delta in object storage, read in place.
      </p>

      <h2>Where state lives, and what a new host gets back</h2>
      <p>
        Three places, and only two of them are in object storage. This is the thing to know before you replace the
        machine.
      </p>
      <table>
        <thead><tr><th>What</th><th>Where</th><th>Survives a new host</th></tr></thead>
        <tbody>
          <tr>
            <td>Table data</td>
            <td><code>kaveonlake</code> / <code>opensource</code></td>
            <td>Yes — object storage</td>
          </tr>
          <tr>
            <td>The platform&rsquo;s records: datasets, charts, dashboards, saved statements, chat history, favourites, audit</td>
            <td><code>kaveonlake</code> / <code>product</code> / <code>kaveon/system/v2</code></td>
            <td>Yes — object storage</td>
          </tr>
          <tr>
            <td>The Engine&rsquo;s catalog: catalogs, schemas, every table definition, planner statistics, cubes</td>
            <td><code>/var/lib/kaveon/catalog.db</code> on the coordinator (the <code>catalog-data</code> volume)</td>
            <td><strong>No</strong> — a file on the host</td>
          </tr>
        </tbody>
      </table>
      <Callout type="warn">
        <strong>The data is durable; the map to it is not yet.</strong> A replaced host comes up able to read
        everything and knowing about nothing: no catalogs, no table definitions, no statistics and no cubes, so
        dashboards that were answered from precomputed cells go back to scanning. Back up{" "}
        <code>catalog.db</code> alongside the system store, and see{" "}
        <a href="/docs/operations">Operations</a> for the sequence that rebuilds the catalog when you do not have it.
        Folding this third place into the first two is a design in progress.
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
        run the API test suite, run the operational script tests, scan for secrets, and validate the documentation. The
        documentation gate fails closed — a new Engine setting without a row in{" "}
        <a href="/docs/engine">the settings table</a> turns CI red and holds the Studio deploy with it. On a green run,
        Studio deploys to Vercel.
      </p>
      <p>
        <strong>The API is not deployed by a workflow.</strong> It is updated on the VM by checking out the commit and
        rebuilding its image there, which is a manual step today — nothing in CI reaches the host.{" "}
        <code>.github/workflows/deploy.yml</code> still describes an Azure Container Apps rollout and is{" "}
        <strong>manual-only</strong>: the container app it targeted was deleted once the VM became the deployment, and
        a workflow that always fails hides the one that matters.
      </p>

      <h2>Running the whole platform locally</h2>
      <p>
        <code>scripts/kaveon-up.sh</code> brings up the same stack on any machine with Docker, generating its secrets
        once and writing them to <code>.env</code>. Point it at a directory of Delta or Parquet tables to mount
        read-only; the one decision worth making is where Kaveon keeps its own records. See{" "}
        <a href="/docs/quickstart">Quickstart</a> for the short path and the self-hosting guide for the long one.
      </p>
      <Code lang="bash">{`./scripts/kaveon-up.sh --data /mnt/warehouse
./scripts/kaveon-up.sh --storage adls://myaccount/product/kaveon/system

# Studio:   http://localhost:3000
# API:      http://localhost:8082/api/health
# Engine:   http://localhost:8081/v1/statement`}</Code>
      <p>
        Only Studio&rsquo;s port is meant for a person. The API and the Engine publish to loopback so that Studio stays
        the only front door, which is what the proxy secret assumes.
      </p>
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
          <tr><td>The system store (coordinator and API, same values)</td><td><code>KAVEON_PRODUCT_STORAGE_MODE</code>, and for object storage <code>KAVEON_PRODUCT_ADLS_ACCOUNT</code>, <code>KAVEON_PRODUCT_ADLS_CONTAINER</code>, <code>KAVEON_PRODUCT_ADLS_PREFIX</code>; for a directory <code>KAVEON_PRODUCT_LOCAL_PATH</code></td></tr>
          <tr><td>Engine (coordinator and workers)</td><td><code>KAVEON_DATA_DIR</code>, <code>KAVEON_EXCHANGE_TOKEN</code>, <code>KAVEON_DISCOVERY_URI</code>, memory and spill settings</td></tr>
        </tbody>
      </table>
      <p>
        The coordinator and the API read the system-store variables from the same values, so Settings can report where
        the control plane is kept without being told twice. The API only reads them; it never writes to that store.
      </p>

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
