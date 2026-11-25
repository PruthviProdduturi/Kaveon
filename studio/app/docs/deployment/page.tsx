import { PageHeader, Code, Pager } from "../../../components/docs/prose";

export const metadata = { title: "Deployment topology" };

export default function DeploymentDocs() {
  return <div className="docs-prose">
    <PageHeader eyebrow="Deploy &amp; Operate" title="Deployment topology" lead="Studio is the browser entry point. The platform API handles product requests and DLM routing. The Engine coordinates analytical queries and commits product records." />
    <p>For the Azure form, Linux VM instructions and first-sign-in checks, start with <a href="/docs/installation">Deploy Kaveon</a>.</p>

    <h2>Where each service runs</h2>
    <table>
      <thead><tr><th>Installation</th><th>Studio and API</th><th>Engine and storage</th></tr></thead>
      <tbody>
        <tr><td>Local Docker or Linux VM</td><td>Compose runs Studio and the API on the same host. A shared VM needs an HTTPS endpoint and configured sign-in.</td><td>Coordinator and two workers. Local files are mounted at <code>/data</code>. The coordinator uses a persistent volume for its catalog.</td></tr>
        <tr><td>AKS</td><td>The platform Helm chart runs Studio and the API. Route public HTTPS to Studio and keep the API service private.</td><td>The Engine Helm chart runs the coordinator and workers. Workload identity grants access to the configured ADLS containers.</td></tr>
        <tr><td>Studio on Vercel</td><td>Vercel hosts Studio. Configure its server-side proxy with an API endpoint it can securely reach.</td><td>The API and Engine run on separate compute. A private Kubernetes service address requires a network path from the Studio host.</td></tr>
      </tbody>
    </table>

    <p><strong>AWS deployment · Coming soon.</strong> Use the <a href="/docs/installation#azure">guided Azure installation</a> for the current cloud preview.</p>

    <h2>From the browser to the lake</h2>
    <Code lang="text">{`Browser → Studio → authenticated Studio proxy → platform API
                                                  ├─ DLM context answer
                                                  ├─ analytical SQL → Engine → lake
                                                  └─ product write → KaveonDB`}</Code>
    <p>Studio derives the user identity from the session. The API verifies the proxy credential and authorizes the request. The Engine reads a registered lake using the deployment’s storage identity. A catalog can point to another storage account when that identity has access. See <a href="/docs/auth">Auth &amp; RBAC</a>.</p>

    <h2>What persists</h2>
    <table>
      <thead><tr><th>State</th><th>Location</th><th>Recovery dependency</th></tr></thead>
      <tbody>
        <tr><td>Source table data</td><td>Registered files or lake tables</td><td>Keep their storage, credentials and paths accessible to the replacement deployment.</td></tr>
        <tr><td>Product records</td><td>KaveonDB’s configured local store or ADLS account, container and prefix</td><td>Preserve the store and reconnect to the same location to recover datasets, charts and dashboards.</td></tr>
        <tr><td>Engine catalog</td><td>Coordinator storage, including <code>/var/lib/kaveon/catalog.db</code></td><td>Keep the volume or a tested catalog backup. A restored lake alone does not recreate table registrations.</td></tr>
      </tbody>
    </table>

    <h2>Release and verify</h2>
    <p>The AKS Engine and platform use separate Helm releases and digest-pinned images. Keep the release values, image digests, trust bundle and storage identifiers with the installation record. Protect credentials in the deployment’s secret store.</p>
    <p>Verify Studio over HTTPS, sign in, run a SQL Lab query and read a registered table. Check worker health from the Engine console. For recovery and ongoing checks, see <a href="/docs/operations">Operations</a> and <a href="/docs/upgrades">Upgrades</a>.</p>
    <Pager prev={{ href: "/docs/concepts", title: "Core concepts" }} next={{ href: "/docs/auth", title: "Auth & RBAC" }} />
  </div>;
}
