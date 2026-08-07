import { PageHeader, Callout, Code, Pager } from "../../../components/docs/prose";

export const metadata = { title: "Architecture" };

export default function ArchitectureDocs() {
  return (
    <div className="docs-prose">
      <PageHeader
        eyebrow="Platform"
        title="Architecture"
        lead="Kaveon is a pnpm monorepo with two applications — a Next.js frontend and a FastAPI backend — over a metadata database and your data sources. The browser only ever talks to the frontend."
      />

      <h2>The two applications</h2>
      <table>
        <thead><tr><th>App</th><th>Path</th><th>Runtime</th></tr></thead>
        <tbody>
          <tr><td><code>kaveon-web</code></td><td><code>apps/kaveon-web</code></td><td>Next.js 15 · React 19 · TypeScript</td></tr>
          <tr><td><code>kaveon-api</code></td><td><code>apps/kaveon-api</code></td><td>FastAPI · Python 3.11</td></tr>
        </tbody>
      </table>
      <p>Shared TypeScript types live in <code>packages/types</code>.</p>

      <h2>Request flow</h2>
      <Code lang="text">{`Browser
  │  session cookie (same-origin)
  ▼
kaveon-web  (Next.js — App Router, RSC by default)
  │  /api/kaveon/[...path]  — the only ingress to the API
  │  stamps X-User-Email, X-User-Name, X-User-Role, X-Proxy-Secret
  ▼
kaveon-api  (FastAPI)
  ├── Metadata DB  (Postgres — datasets, charts, dashboards, roles, history)
  └── Data Sources (Fabric SQL · Azure SQL · PostgreSQL · MySQL · StarRocks)`}</Code>
      <Callout type="note">
        The API is <strong>never exposed to the browser</strong>. All traffic hits Next.js; the proxy route forwards it
        server-side with the identity headers and a shared secret the API validates. See{" "}
        <a href="/docs/auth">Auth &amp; RBAC</a> for that contract.
      </Callout>

      <h2>Frontend — kaveon-web</h2>
      <p>Next.js 15 App Router, React Server Components by default, with client components for the interactive builders.</p>
      <Code lang="text">{`app/
  page.tsx            — Homepage / NL→SQL chat
  lab/                — SQL Lab (Monaco)
  charts/  datasets/  — builders + lists
  dashboards/         — react-grid-layout canvas
  data-sources/       — connection management
  docs/               — this documentation (public)
  api/kaveon/[...]/   — API proxy (route.ts)
  api/auth/           — NextAuth (Auth.js v5)`}</Code>
      <table>
        <thead><tr><th>Key utility</th><th>Purpose</th></tr></thead>
        <tbody>
          <tr><td><code>utils/nlToSql.ts</code></td><td>Template-based NL→SQL engine</td></tr>
          <tr><td><code>utils/echartsTheme.ts</code></td><td>Dark/light theming for ECharts</td></tr>
          <tr><td><code>utils/querySemaphore.ts</code></td><td>Client-side query concurrency limit</td></tr>
          <tr><td><code>components/charts/ChartBuilderContext.tsx</code></td><td>Chart registry, SQL generation, builder state</td></tr>
          <tr><td><code>components/dashboards/DashboardCanvas.tsx</code></td><td>Grid-layout dashboard canvas</td></tr>
        </tbody>
      </table>

      <h2>Backend — kaveon-api</h2>
      <p>
        FastAPI with a routers/services/middleware split: one router per domain, business logic in services
        (<code>query_generator</code>, <code>ai_service</code>, auth/role resolution), and cross-cutting concerns
        (JWT verification, RBAC, rate limiting, error shaping) in middleware. A per-database connection pool is warmed at
        startup and kept alive with a 5-minute heartbeat.
      </p>

      <h2>Theming</h2>
      <p>
        Every color is a CSS variable — no hardcoded values in components — so light/dark and per-user themes apply
        instantly. ECharts options are passed through <code>applyChartTheme(option, isDark)</code> so charts match.
      </p>
      <Code lang="css">{`var(--accent)        /* #4A9EE8 — brand blue */
var(--bg-surface)    /* cards / panels */
var(--text-primary)  var(--text-muted)  var(--border)`}</Code>

      <Pager
        prev={{ href: "/docs/data-sources", title: "Data Sources" }}
        next={{ href: "/docs/auth", title: "Auth & RBAC" }}
      />
    </div>
  );
}
