import { PageHeader, Callout, Code, Pager } from "../../../components/docs/prose";

export const metadata = { title: "Auth & RBAC" };

export default function AuthDocs() {
  return (
    <div className="docs-prose">
      <PageHeader
        eyebrow="Platform"
        title="Auth & RBAC"
        lead="Sign-in is provider-based (no local passwords by default), roles gate what you can do, and a visibility model gates what you can see. The API trusts identity only from the frontend proxy, sealed with a shared secret."
      />

      <h2>Sign-in providers</h2>
      <p>
        Authentication is <a href="https://authjs.dev" target="_blank" rel="noreferrer">NextAuth (Auth.js v5)</a>, entirely
        inside the Next.js app — no external gateway. Each provider activates automatically when its credentials are
        present in the environment:
      </p>
      <table>
        <thead><tr><th>Provider</th><th>Env vars</th></tr></thead>
        <tbody>
          <tr><td>GitHub</td><td><code>GITHUB_ID</code> · <code>GITHUB_SECRET</code></td></tr>
          <tr><td>Google</td><td><code>GOOGLE_ID</code> · <code>GOOGLE_SECRET</code></td></tr>
          <tr><td>Microsoft Entra ID</td><td><code>AUTH_MICROSOFT_ENTRA_ID_ID</code> · <code>_SECRET</code> · <code>_ISSUER</code></td></tr>
        </tbody>
      </table>
      <p>Also required: <code>AUTH_SECRET</code> (session signing). The OAuth callback URL is <code>{"{origin}"}/api/auth/callback/{"{provider}"}</code> — register it on each provider app.</p>
      <Callout type="warn">
        Each provider&rsquo;s app must list your exact origins as redirect URIs (e.g. <code>https://kaveon.vercel.app/…</code>
        and <code>http://localhost:3000/…</code>). A missing origin is the usual cause of a
        &ldquo;redirect_uri not associated&rdquo; error.
      </Callout>

      <h2>Roles</h2>
      <p>Four roles, ordered by privilege. The role is resolved from the JWT claim, then the metadata DB, defaulting to Viewer:</p>
      <table>
        <thead><tr><th>Role</th><th>Can</th></tr></thead>
        <tbody>
          <tr><td><strong>Viewer</strong></td><td>Read published dashboards and charts</td></tr>
          <tr><td><strong>Analyst</strong></td><td>+ Run SQL, build charts and datasets</td></tr>
          <tr><td><strong>Editor</strong></td><td>+ Publish content, delete</td></tr>
          <tr><td><strong>Admin</strong></td><td>+ Manage users, data sources, metadata server, AI keys</td></tr>
        </tbody>
      </table>
      <p>
        Admins are seeded from <code>AUTH_ADMIN_EMAILS</code> (comma-separated). Every state-changing endpoint is guarded by
        a <code>require_min_role(&quot;…&quot;)</code> dependency — a lower role gets <code>403 forbidden</code>.
      </p>
      <Callout type="note">
        The four-role ladder lives in the API&rsquo;s authorization layer (<code>Viewer / Analyst / Editor / Admin</code>).
        The NextAuth sign-in itself resolves each user to just two of them: <strong>Admin</strong> when their email is
        listed in <code>AUTH_ADMIN_EMAILS</code>, otherwise <strong>Viewer</strong>.
      </Callout>

      <h2>Visibility</h2>
      <p>
        Independent of role, each dataset, chart, and dashboard has a visibility level — <strong>private</strong> (owner),
        <strong> internal</strong> (signed-in users), <strong>published</strong> — enforced by a <code>can_read()</code>
        helper on every list/get call.
      </p>

      <h2>The proxy trust contract</h2>
      <p>The browser never holds an API token. The Next.js proxy is the only ingress and stamps identity headers server-side:</p>
      <Code lang="text">{`kaveon-web  /api/kaveon/[...path]
   ├─ reads the NextAuth session (server-side)
   ├─ injects  X-User-Email · X-User-Name · X-User-Role
   └─ signs the request with  X-Proxy-Secret = KAVEON_PROXY_SECRET
        ▼
kaveon-api  trusts X-User-* ONLY when the secret matches`}</Code>
      <p>The same <code>KAVEON_PROXY_SECRET</code> must be set on both the web and API deployments.</p>

      <h2>Hardening built in</h2>
      <ul>
        <li><strong>Identifier injection</strong> — all user identifiers are quoted/escaped before hitting SQL.</li>
        <li><strong>Operator injection</strong> — filter operators are validated against a strict allowlist.</li>
        <li><strong>Credential exposure</strong> — the <code>connection_string</code> column is stripped from every API response.</li>
        <li><strong>Identity spoofing</strong> — raw <code>x-user-email</code> headers are rejected unless the proxy secret matches.</li>
        <li><strong>Information disclosure</strong> — internal errors are logged server-side; clients get a generic message.</li>
      </ul>

      <Pager
        prev={{ href: "/docs/architecture", title: "Architecture" }}
        next={{ href: "/docs/deployment", title: "Deployment" }}
      />
    </div>
  );
}
