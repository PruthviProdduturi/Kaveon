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
        Kaveon sits between you and your data. Point it at a warehouse or database and you
        get a private analytics workspace: a full <strong>SQL Lab</strong>, a drag-and-drop <strong>chart builder</strong>,
        composable <strong>dashboards</strong>, reusable <strong>semantic datasets</strong>, and an{" "}
        <strong>AI layer</strong> that turns plain-English questions into charts. It is open source, MIT-licensed,
        and runs entirely on infrastructure you control.
      </p>
      <p>
        Everything is configurable from the UI — no config-file edits, no restarts to add a data source or
        switch authentication provider.
      </p>

      <Callout type="tip">
        New here? Jump straight to the <a href="/docs/quickstart">Quickstart</a> — sign in, connect a source,
        and get your first chart on screen in a few minutes.
      </Callout>

      <h2>How it fits together</h2>
      <p>Kaveon is a small monorepo with two services and a data layer:</p>
      <table>
        <thead>
          <tr><th>Piece</th><th>Stack</th><th>Responsibility</th></tr>
        </thead>
        <tbody>
          <tr>
            <td><strong>kaveon-web</strong></td>
            <td>Next.js 15 · TypeScript · React 19</td>
            <td>The UI — SQL Lab, chart &amp; dashboard builders, AI chat. Handles sign-in (NextAuth) and proxies data calls.</td>
          </tr>
          <tr>
            <td><strong>kaveon-api</strong></td>
            <td>FastAPI · Python 3.11</td>
            <td>Query execution, semantic SQL generation, RBAC, connection pooling to every data source.</td>
          </tr>
          <tr>
            <td><strong>Data layer</strong></td>
            <td>Postgres · Fabric SQL · Azure SQL · MySQL · StarRocks</td>
            <td>A metadata database (Kaveon&rsquo;s own state) plus the data sources you register.</td>
          </tr>
        </tbody>
      </table>

      <h2>How these docs are organized</h2>
      <ul>
        <li><strong>Getting Started</strong> — what Kaveon is, a hands-on quickstart, and the core concepts you&rsquo;ll reuse everywhere.</li>
        <li><strong>Features</strong> — deep dives on SQL Lab, the AI / NL→SQL engine, the chart builder, dashboards, semantic datasets, and data sources.</li>
        <li><strong>Platform</strong> — architecture, authentication &amp; RBAC, and how to deploy.</li>
      </ul>
      <p>Each page has an outline on the right; use the search box in the sidebar to jump anywhere.</p>

      <Pager next={{ href: "/docs/quickstart", title: "Quickstart" }} />
    </div>
  );
}
