import { PageHeader, Callout, Code, Pager } from "../../../components/docs/prose";

export const metadata = { title: "Deployment" };

export default function DeploymentDocs() {
  return (
    <div className="docs-prose">
      <PageHeader
        eyebrow="Platform"
        title="Deployment"
        lead="Kaveon deploys as three tiers — the Next.js frontend on Vercel, the FastAPI backend on Azure Container Apps, and Azure PostgreSQL. CI builds and ships on every push."
      />

      <h2>Topology</h2>
      <Code lang="text">{`Browser ──► Vercel  (kaveon-web · NextAuth: GitHub / Google / Microsoft)
               │  same-origin /api/kaveon proxy (injects X-User-* + secret)
               ▼
            Azure Container Apps  (kaveon-api · FastAPI · image from kaveonacr)
               │  psycopg2 / DefaultAzureCredential (Managed Identity)
               ▼
            Azure PostgreSQL Flexible Server (PG 18)
               ├── kaveonmeta  (control plane + DLM/context)
               └── kaveon      (data warehouse — the rows)`}</Code>
      <p>
        The browser only talks to Vercel; the proxy forwards to the Container App with <code>X-User-*</code> headers
        stamped by <code>KAVEON_PROXY_SECRET</code>, which the API validates (see <a href="/docs/auth">Auth &amp; RBAC</a>).
      </p>
      <Callout type="note">
        Both databases live on one <strong>Azure Database for PostgreSQL Flexible Server (PG 18)</strong>:{" "}
        <code>kaveonmeta</code> holds Kaveon&rsquo;s own state plus the DLM context, and <code>kaveon</code> is the data
        warehouse. In production both authenticate via <strong>Managed Identity</strong> — no stored password. Self-hosting
        elsewhere only needs the <code>METADATA_*</code> connection settings changed.
      </Callout>

      <h2>CI/CD</h2>
      <p>
        <code>.github/workflows/ci.yml</code> runs on every push and PR to <code>dev</code>:
      </p>
      <ul>
        <li><strong>web</strong> — install, type-check shared types, lint, tsc, and build <code>kaveon-web</code>.</li>
        <li><strong>api</strong> — install, <code>compileall</code>, and run pytest if tests exist.</li>
        <li><strong>secrets</strong> — a gitleaks scan.</li>
        <li><strong>deploy (web)</strong> — on push to <code>dev</code> (after web passes), <code>vercel deploy --prod</code>
          using the <code>VERCEL_TOKEN</code> / <code>VERCEL_ORG_ID</code> / <code>VERCEL_PROJECT_ID</code> secrets.</li>
      </ul>
      <p>
        <code>.github/workflows/deploy.yml</code> ships the API on push to <code>dev</code>: it builds and pushes the
        image to <code>kaveonacr.azurecr.io</code> and runs <code>az containerapp update</code> to roll it out. The Vercel
        build installs with <code>npm install --legacy-peer-deps</code> (pnpm fails in Vercel&rsquo;s build sandbox).
      </p>

      <h2>Key environment variables</h2>
      <table>
        <thead><tr><th>Where</th><th>Vars</th></tr></thead>
        <tbody>
          <tr><td>Both tiers</td><td><code>KAVEON_PROXY_SECRET</code> (must match)</td></tr>
          <tr><td>Web (Vercel)</td><td><code>AUTH_SECRET</code>, <code>AUTH_URL</code>, provider IDs/secrets, <code>API_URL</code></td></tr>
          <tr><td>API (Azure Container Apps)</td><td><code>METADATA_DATABASE</code> (=<code>kaveonmeta</code>), <code>AAD_DATABASES</code> (=<code>kaveon</code>), <code>METADATA_HOST</code>/<code>PORT</code>/<code>SSLMODE</code>, <code>KAVEON_PROXY_SECRET</code></td></tr>
        </tbody>
      </table>
      <Callout type="tip">
        First run needs no database config — the setup wizard appears on first sign-in and initializes the metadata schema
        for you. Data-source endpoints are always registered from the UI, never from <code>.env</code>.
      </Callout>

      <h2>Production notes</h2>
      <p>
        The reference deployment is a zero-cost demo, not hardened. For production, put the API on private networking, use
        managed secrets (e.g. Key Vault), run a dedicated warehouse, and prefer managed-identity auth for Fabric/Azure SQL
        over connection strings.
      </p>

      <Pager prev={{ href: "/docs/auth", title: "Auth & RBAC" }} />
    </div>
  );
}
