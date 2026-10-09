import { PageHeader, Callout, Pager } from "../../../components/docs/prose";

export const metadata = { title: "Kaveon Studio" };

export default function StudioDocs() {
  return (
    <div className="docs-prose">
      <PageHeader
        eyebrow="Studio"
        title="Kaveon Studio"
        lead="The governed workspace for asking questions, writing SQL, defining semantic datasets, building charts, and publishing dashboards."
      />

      <p>
        Studio is the user-facing surface of Kaveon. It authenticates the user, sends trusted requests through
        the platform proxy, and presents results from the KaveonDB transactional plane, the distributed Engine,
        or a deterministic DLM context artifact. Hosting is portable: Studio can run on Vercel or as a container
        in the same AKS or VM deployment as the API and Engine.
      </p>

      <h2>Choose the surface</h2>
      <table>
        <thead><tr><th>Surface</th><th>Use it for</th><th>Guide</th></tr></thead>
        <tbody>
          <tr><td>Chat</td><td>Ask a governed natural-language question and inspect whether the answer came from context or a live query.</td><td><a href="/docs/dlm">Data Language Model</a></td></tr>
          <tr><td>SQL Lab</td><td>Write SQL, inspect results, cancel work, save statements, and review query details.</td><td><a href="/docs/sql-lab">SQL Lab</a></td></tr>
          <tr><td>Catalog</td><td>Browse catalogs, schemas, columns, file locations, statistics, and table details.</td><td><a href="/docs/engine/storage">Storage &amp; Catalogs</a></td></tr>
          <tr><td>Datasets</td><td>Declare dimensions, measures, time columns, and freshness semantics once.</td><td><a href="/docs/datasets">Semantic Datasets</a></td></tr>
          <tr><td>Charts</td><td>Turn a dataset query into a reusable visualization with filters and formatting.</td><td><a href="/docs/charts">Chart Builder</a></td></tr>
          <tr><td>Dashboards</td><td>Compose charts into a shareable, filterable analytical canvas.</td><td><a href="/docs/dashboards">Dashboards</a></td></tr>
          <tr><td>Settings</td><td>Manage identity, catalog access, storage connections, governance, and runtime status.</td><td><a href="/docs/auth">Auth &amp; RBAC</a></td></tr>
        </tbody>
      </table>

      <h2>One governed request path</h2>
      <p>
        The browser session never supplies trusted identity headers. Studio resolves the authenticated session
        server-side, calls the API through <code>/api/kaveon/*</code>, and the proxy stamps the verified principal
        before the API routes work to KaveonDB, the DLM, or the Engine. The same contract works when Studio is
        hosted by Vercel or by the AKS portal chart.
      </p>
      <Callout type="note">
        Studio is the product experience; it is not the storage engine. Product records are committed through
        KaveonDB, analytical SQL runs through the Engine, and DLM context is used only when its version is fresh
        enough to answer safely.
      </Callout>

      <h2>Deploy Studio</h2>
      <p>
        For a five-minute local run, use <a href="/docs/quickstart">Quickstart</a>. For Vercel, AKS, or a Linux
        VM, follow <a href="/docs/installation">Install &amp; deploy</a>, then verify the request path in
        <a href="/docs/deployment">Deployment topology</a> and <a href="/docs/operations">Operations</a>.
      </p>

      <Pager prev={{ href: "/docs/concepts", title: "Core concepts" }} next={{ href: "/docs/sql-lab", title: "SQL Lab" }} />
    </div>
  );
}
