import { PageHeader, Callout, Code, Pager } from "../../../components/docs/prose";

export const metadata = { title: "Deployment" };

export default function DeploymentDocs() {
  return (
    <div className="docs-prose">
      <PageHeader
        eyebrow="Platform"
        title="Deployment"
        lead="Kaveon deploys as three tiers — the Next.js frontend, the FastAPI backend, and a Postgres database. CI builds and ships on every push. Every tier has a free tier for demos."
      />

      <h2>Topology</h2>
      <Code lang="text">{`Browser ──► Vercel  (kaveon-web · NextAuth: GitHub / Google / Microsoft)
               │  same-origin /api/kaveon proxy (injects X-User-* + secret)
               ▼
            Render  (kaveon-api · FastAPI, Docker)
               │  psycopg2 (user/password + SSL)
               ▼
            Postgres  (metadata + demo data)`}</Code>
      <p>
        The browser only talks to Vercel; the proxy forwards to Render with <code>X-User-*</code> headers stamped by
        <code> KAVEON_PROXY_SECRET</code>, which the API validates (see <a href="/docs/auth">Auth &amp; RBAC</a>).
      </p>
      <Callout type="note">
        The Postgres tier is portable — the reference demo runs on <strong>Neon</strong> (serverless), and production can
        run on <strong>Azure Database for PostgreSQL Flexible Server</strong>. Migrating between them is a schema + data
        copy (<code>scripts/migrate-neon-to-azure.py</code>); only the <code>METADATA_*</code> connection settings change.
      </Callout>

      <h2>CI/CD</h2>
      <p>
        <code>.github/workflows/ci.yml</code> runs on every push and PR to <code>dev</code>:
      </p>
      <ul>
        <li><strong>web</strong> — install, type-check shared types, lint, tsc, and build <code>kaveon-web</code>.</li>
        <li><strong>api</strong> — install, <code>compileall</code>, and run pytest if tests exist.</li>
        <li><strong>secrets</strong> — a gitleaks scan.</li>
        <li><strong>deploy</strong> — on push to <code>dev</code> (after web passes), <code>vercel deploy --prod</code> using
          the <code>VERCEL_TOKEN</code> / <code>VERCEL_ORG_ID</code> / <code>VERCEL_PROJECT_ID</code> secrets.</li>
      </ul>
      <p>Render auto-deploys the API from its own Git integration (<code>render.yaml</code> Blueprint).</p>

      <h2>Key environment variables</h2>
      <table>
        <thead><tr><th>Where</th><th>Vars</th></tr></thead>
        <tbody>
          <tr><td>Both tiers</td><td><code>KAVEON_PROXY_SECRET</code> (must match)</td></tr>
          <tr><td>Web (Vercel)</td><td><code>AUTH_SECRET</code>, <code>AUTH_URL</code>, provider IDs/secrets, <code>API_URL</code></td></tr>
          <tr><td>API (Render)</td><td><code>METADATA_*</code> (DB connection), <code>KAVEON_PROXY_SECRET</code></td></tr>
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
