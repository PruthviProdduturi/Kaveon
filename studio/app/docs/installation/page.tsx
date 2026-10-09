import { Callout, Code, PageHeader } from "../../../components/docs/prose";

export const metadata = { title: "Installation" };

export default function InstallationDocs() {
  return (
    <div className="docs-prose">
      <PageHeader
        eyebrow="Install Kaveon"
        title="Installation"
        lead="Run Kaveon locally, on a Linux host, or as a managed Kubernetes workload. Choose one deployment shape; the Engine, API, and Studio keep the same contracts in each."
      />

      <p>
        A Kaveon deployment has an Engine coordinator, optional Engine workers, the API and Studio. Table data stays in
        Parquet, Delta or Iceberg storage; KaveonDB stores the platform records transactionally. Studio can be hosted on
        Vercel while the data plane runs on your infrastructure.
      </p>

      <h2>Choose a deployment</h2>
      <table>
        <thead><tr><th>Target</th><th>Use it for</th><th>Start here</th></tr></thead>
        <tbody>
          <tr><td>Docker Compose</td><td>Local development, demos and a single host</td><td><a href="/docs/quickstart">Quickstart</a> · <a href="/docs/deployment">Deployment</a></td></tr>
          <tr><td>Linux VM</td><td>A small, controlled deployment with Docker and a reverse proxy</td><td><a href="/docs/deployment">Deployment</a> · <a href="/docs/operations">Operations</a></td></tr>
          <tr><td>Kubernetes with Helm</td><td>Worker replacement, autoscaling and repeatable releases</td><td><a href="/docs/deployment">Deployment</a></td></tr>
          <tr><td>Kaveon CLI</td><td>Interactive SQL and scripts from another machine</td><td><a href="/docs/engine">Engine</a></td></tr>
        </tbody>
      </table>

      <h2>Docker Compose</h2>
      <p>Docker is the shortest path. It starts the coordinator, workers, API and Studio without requiring PostgreSQL.</p>
      <Code lang="bash">{`git clone https://github.com/PruthviProdduturi/Kaveon.git
cd Kaveon
./scripts/kaveon-up.sh

# Studio       http://localhost:3000
# API health   http://localhost:8082/api/health
# Engine       http://localhost:8081/health`}</Code>
      <p>Use <code>./scripts/kaveon-up.sh --data /path/to/lake</code> to mount a local lake read-only. See the self-hosting guide for storage, backups and sign-in.</p>

      <h2>Linux VM</h2>
      <p>Use the same Compose stack on a Linux VM, bind the public entry point to a TLS reverse proxy, and keep the Engine and API private. A VM is the simplest persistent cloud deployment and is independent of Kubernetes.</p>
      <Callout type="warn">Do not publish the Engine or API ports directly. Put TLS and authentication at Studio or the reverse proxy, and back up both the KaveonDB system store and the Engine catalog.</Callout>

      <h2>Kubernetes with Helm</h2>
      <p>The Helm charts pin every image by immutable digest. The production-shaped Azure template creates <code>kaveon-aks</code> with one system node and a worker pool autoscaled from one to four nodes. It preserves the existing ADLS account and ACR; it does not change subscription policies.</p>
      <Code lang="powershell">{`az aks get-credentials --resource-group kaveon-rg --name kaveon-aks --overwrite-existing
kubelogin convert-kubeconfig -l azurecli
helm upgrade --install kaveon infra/helm/kaveon-test --namespace kaveon --create-namespace `
  --set image.repository=kaveonacr.azurecr.io/kaveon-engine `
  --set image.digest=sha256:<immutable-digest>`}</Code>
      <p>Use the Azure deployment guide for the Bicep workflow, workload identity, secrets, TLS and autoscaler verification. The chart is deliberately separate from infrastructure creation so credentials never enter source control.</p>

      <h2>Studio on Vercel</h2>
      <p>Studio is a separate web deployment. The repository CI workflow validates and deploys the Studio build to Vercel; it does not replace the Engine or API. Configure the same-origin API proxy and the deployed Engine/API URL in Vercel environment variables.</p>

      <h2>After installation</h2>
      <ol>
        <li>Open Catalog and register a Parquet, Delta or Iceberg location.</li>
        <li>Run <code>ANALYZE</code> so the planner can use exact source statistics.</li>
        <li>Run a bounded SQL query from SQL Lab or the CLI.</li>
        <li>Configure authentication before exposing Studio to other users.</li>
      </ol>
    </div>
  );
}
