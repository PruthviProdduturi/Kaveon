import { PageHeader, Callout, Pager } from "../../components/docs/prose";

export const metadata = { title: "Introduction" };

export default function DocsIntro() {
  return (
    <div className="docs-prose">
      <PageHeader
        eyebrow="Introduction"
        title="Kaveon documentation"
        lead="Kaveon is a self-hosted analytics platform — query live data, build charts, assemble dashboards, and ask questions in plain English. This is the reference for using and operating it."
      />

      <h2>What Kaveon is</h2>
      <p>
        Kaveon sits between you and your data. Point Studio at a warehouse or database and you
        get a private analytics workspace: a full <strong>SQL Lab</strong>, a drag-and-drop <strong>chart builder</strong>,
        composable <strong>dashboards</strong>, reusable <strong>semantic datasets</strong>, and a{" "}
        <strong>DLM (Data Language Model)</strong> that turns plain-English questions into charts with no hosted
        LLM — answering the common ones straight from precomputed context. It is open source, MIT-licensed,
        and can be self-hosted on infrastructure you control.
      </p>
      <p>
        Data sources are configurable from the UI — no config-file edits, no restarts to add one. Authentication
        providers (GitHub, Google, Microsoft Entra) activate automatically when their env vars are set.
      </p>

      <Callout type="tip">
        New here? Jump straight to the <a href="/docs/quickstart">Quickstart</a> — sign in, connect a source,
        and get your first chart on screen in a few minutes.
      </Callout>

      <h2>Choose your path</h2>
      <table>
        <thead><tr><th>If you want to…</th><th>Start here</th></tr></thead>
        <tbody>
          <tr><td>Evaluate the product and understand its model</td><td><a href="/docs/concepts">Core concepts</a></td></tr>
          <tr><td>Explore data and build analytical experiences</td><td><a href="/docs/quickstart">Studio quickstart</a></td></tr>
          <tr><td>Integrate with Kaveon programmatically</td><td><a href="/docs/api-reference">API reference</a></td></tr>
          <tr><td>Deploy and run the platform</td><td><a href="/docs/deployment">Deployment</a> and <a href="/docs/operations">Operations</a></td></tr>
          <tr><td>Develop or evaluate the Rust query runtime</td><td><a href="/docs/engine">Kaveon Engine</a></td></tr>
          <tr><td>Study the underlying methods and IP</td><td><a href="/docs/research">Papers &amp; patents</a></td></tr>
        </tbody>
      </table>

      <h2>How it fits together</h2>
      <p>Kaveon is a small monorepo with Studio, a platform API, an independent query engine, and a data layer:</p>
      <table>
        <thead>
          <tr><th>Piece</th><th>Stack</th><th>Responsibility</th></tr>
        </thead>
        <tbody>
          <tr>
            <td><strong>Kaveon Studio</strong></td>
            <td>Next.js 15 · TypeScript · React 19</td>
            <td>The UI — SQL Lab, chart &amp; dashboard builders, AI chat. Handles sign-in (NextAuth) and proxies data calls.</td>
          </tr>
          <tr>
            <td><strong>kaveon-api</strong></td>
            <td>FastAPI · Python 3.11</td>
            <td>Query execution, semantic SQL generation, the DLM engine, RBAC, connection pooling to every data source.</td>
          </tr>
          <tr>
            <td><strong>kaveon-engine</strong></td>
            <td>Rust · Arrow · Parquet</td>
            <td>An alpha query runtime with CLI and HTTP entry points. It is not yet wired into the shipping Studio/API query path.</td>
          </tr>
          <tr>
            <td><strong>Data layer</strong></td>
            <td>Azure PostgreSQL · Fabric SQL · Azure SQL · MySQL · StarRocks</td>
            <td>A two-plane store — <code>kaveonmeta</code> (Kaveon&rsquo;s own state + DLM context) and the <code>kaveon</code> warehouse — plus the data sources you register.</td>
          </tr>
        </tbody>
      </table>

      <h2>How these docs are organized</h2>
      <ul>
        <li><strong>Getting Started</strong> — what Kaveon is, a hands-on quickstart, and the core concepts you&rsquo;ll reuse everywhere.</li>
        <li><strong>Studio</strong> — SQL Lab, AI / NL→SQL, charts, dashboards, semantic datasets, and data sources.</li>
        <li><strong>Intelligence</strong> — the Data Language Model and freshness-based routing.</li>
        <li><strong>Build &amp; Operate</strong> — architecture, Engine, APIs, authentication, deployment, and operations.</li>
        <li><strong>Research</strong> — long-form technical papers and the patent disclosure.</li>
      </ul>
      <p>Each desktop page has an outline on the right; use the documentation filter to find pages by title, description, or keyword.</p>

      <Pager next={{ href: "/docs/quickstart", title: "Quickstart" }} />
    </div>
  );
}
