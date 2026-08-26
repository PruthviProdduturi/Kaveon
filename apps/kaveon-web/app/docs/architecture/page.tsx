import { PageHeader, Callout, Code, Pager } from "../../../components/docs/prose";

export const metadata = { title: "Architecture" };

export default function ArchitectureDocs() {
  return (
    <div className="docs-prose">
      <PageHeader
        eyebrow="Platform"
        title="Architecture"
        lead="Kaveon is a pnpm monorepo with two applications — a Next.js frontend and a FastAPI backend — over a two-plane database (a small control + context store and a data warehouse) plus your registered data sources. The browser only ever talks to the frontend."
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
  ├── kaveonmeta   (Azure PG — control + context: datasets, charts, dashboards,
  │                 roles, history, and the DLM tables dlm_* / context_*)
  ├── kaveon       (Azure PG — the data warehouse: the actual rows)
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
          <tr><td><code>utils/nlToSql.ts</code></td><td>Template-based NL→SQL engine — the <em>fallback</em> path (the DLM is primary)</td></tr>
          <tr><td><code>components/ContextBanner.tsx</code></td><td>Homepage banner: what the DLM can answer, with hover detail</td></tr>
          <tr><td><code>components/DatasetContextPanel.tsx</code></td><td>Dataset page: last-generated, duration, indexed dims/metrics, Regenerate</td></tr>
          <tr><td><code>utils/echartsTheme.ts</code></td><td>Dark/light theming for ECharts</td></tr>
          <tr><td><code>utils/querySemaphore.ts</code></td><td>Client-side query concurrency limit</td></tr>
          <tr><td><code>components/charts/ChartBuilderContext.tsx</code></td><td>Chart registry, SQL generation, builder state</td></tr>
          <tr><td><code>components/dashboards/DashboardCanvas.tsx</code></td><td>Grid-layout dashboard canvas</td></tr>
        </tbody>
      </table>

      <h2>Backend — kaveon-api</h2>
      <p>
        FastAPI with a routers/services/middleware split: one router per domain, business logic in services
        (<code>query_generator</code>, the <code>dlm</code> / <code>context_*</code> engine, auth/role resolution),
        and cross-cutting concerns in middleware. Identity is established by the frontend proxy, not the API: middleware
        reads the <code>X-User-*</code> headers and trusts them only when <code>X-Proxy-Secret</code> matches
        <code>KAVEON_PROXY_SECRET</code> — there is no bearer token or JWT verification in the request path. A
        per-database connection pool is warmed at startup and kept alive with a 5-minute heartbeat; the small
        <code>kaveonmeta</code> pool is sized independently from the warehouse pools.
      </p>

      <h2>Query paths — the DLM</h2>
      <p>
        The homepage&rsquo;s primary NL→SQL engine is the <strong>DLM (Data Language Model)</strong> — a per-dataset
        compiled context artifact built with <strong>no hosted LLM</strong>. It resolves a question deterministically
        and, for the common cases, answers from <strong>precomputed context with no database scan at all</strong>.
        Only novel slices fall through to a single live query. Every answer is labelled honestly:
      </p>
      <ul>
        <li><strong>⚡ From context · no DB scan</strong> — a totals / by-dimension / single-dimension-filter answer served from in-memory DLM context loaded from the precomputed <code>dlm_answers</code> store.</li>
        <li><strong>Live query · Xs</strong> — a year-slice or multi-filter combination assembled into one warehouse query, then cached.</li>
      </ul>
      <p>
        Because the <code>kaveonmeta</code> control + context plane is physically separate from the <code>kaveon</code>
        warehouse, context answers never contend with a multi-million-row scan. The in-browser template parser
        (<code>utils/nlToSql.ts</code>) remains as the fallback for shapes the DLM does not yet build (mainly
        time-series trends). See the <a href="/docs/nl-to-sql">NL→SQL</a> page for details.
      </p>

      <Callout type="note">
        Generating a DLM is a one-time <strong>encode</strong> step (read the warehouse&rsquo;s statistics and precompute
        answers), not per-dataset model training. The dataset page shows when context was last generated, how long it
        took, and which dimensions and metrics are indexed.
      </Callout>

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
