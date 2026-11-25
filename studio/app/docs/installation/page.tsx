import { Callout, Code, PageHeader, Pager } from "../../../components/docs/prose";

export const metadata = { title: "Deploy Kaveon" };

const azureInstaller = "https://portal.azure.com/#create/Microsoft.Template/uri/https%3A%2F%2Fraw.githubusercontent.com%2FPruthviProdduturi%2FKaveon%2Fa2c4d3f99f4bf9e49f77726b856a59182e8d29ef%2Finfra%2Fmarketplace%2FmainTemplate.json/createUIDefinitionUri/https%3A%2F%2Fraw.githubusercontent.com%2FPruthviProdduturi%2FKaveon%2Fa2c4d3f99f4bf9e49f77726b856a59182e8d29ef%2Finfra%2Fmarketplace%2FcreateUiDefinition.json";
const azurePackage = "https://github.com/PruthviProdduturi/Kaveon/tree/a2c4d3f99f4bf9e49f77726b856a59182e8d29ef/infra/marketplace";

export default function InstallationDocs() {
  return <div className="docs-prose">
    <PageHeader eyebrow="Getting Started" title="Deploy Kaveon" lead="Start a local workspace with Docker, run it on a Linux server, or provision an Azure installation in your own subscription." />

    <h2>Choose where to run</h2>
    <table>
      <thead><tr><th>Target</th><th>What you get</th><th>Start here</th></tr></thead>
      <tbody>
        <tr><td>Local Docker</td><td>Studio, API and a two-worker Engine on your machine.</td><td><a href="/docs/quickstart">Run the quickstart</a></td></tr>
        <tr><td>Azure Kubernetes Service</td><td>A guided infrastructure and sign-in preview, followed by an operator-managed application install.</td><td><a href={azureInstaller}>Open the Azure installer</a></td></tr>
        <tr><td>Linux VM</td><td>The Compose stack behind your HTTPS endpoint.</td><td><a href="#linux-vm">Configure a Linux VM</a></td></tr>
      </tbody>
    </table>

    <Callout type="note"><strong>AWS deployment · Coming soon.</strong> The guided cloud installer currently targets Azure. An AWS installation path for EKS, IAM and S3 is planned.</Callout>

    <h2 id="azure">Deploy to Azure</h2>
    <p>You need access to create resources and role assignments in the target subscription and permission to register an application and assign its administrator in the target Microsoft Entra tenant.</p>
    <ol>
      <li><a href={azureInstaller}>Open the guided Azure form</a>. On <strong>Basics</strong>, select the subscription, resource group and region. Choose an existing group or enter your own name, such as <code>rg-kaveon-prod</code>.</li>
      <li>Enter an installation name such as <code>kaveon</code>. The environment defaults to <strong>Production</strong>. Select Development or Staging when appropriate. When updating an existing installation, use its original group, installation name and environment.</li>
      <li>On <strong>Compute</strong>, review the worker size and autoscaling limit. The defaults start two D4s v5 system nodes and one B2s worker, with a maximum of two workers. The system nodes and workers incur compute charges while running.</li>
      <li>On <strong>Review + create</strong>, review the settings, then select <strong>Create</strong> to provision infrastructure. Kaveon derives the storage and registry names. The signed-in installer is the initial Kaveon administrator.</li>
    </ol>
    <p>The deployment creates AKS, a container registry, ADLS Gen2 containers, managed identities, networking and the Entra registration. In the deployment’s <strong>Outputs</strong>, keep <code>clusterName</code>, <code>studioPublicIpName</code>, <code>plannedStudioOrigin</code> and <code>entraHelmValues</code> for the application install.</p>

    <h2>Bring Studio online on AKS</h2>
    <p>A successful template deployment completes the infrastructure step. The Studio URL becomes usable after an operator deploys the workloads and configures HTTPS.</p>
    <ol>
      <li>Prepare compatible Engine, API and Studio images pinned by digest, the workload identities, the Engine credentials and CA, and the product storage settings. The <a href={azurePackage}>Azure package guide</a> documents the inputs and installation boundaries.</li>
      <li>Install the Engine Helm chart, then the platform chart for API and Studio. Supply the directory values from the template to both releases. Configure the Studio ingress to use the reserved IP, with a certificate for the planned Studio hostname.</li>
      <li>Open the HTTPS Studio origin, sign in with Microsoft and run <code>SELECT 1 AS ready;</code> in SQL Lab. Then register a lake catalog and read a table to verify storage identity and data access.</li>
    </ol>
    <p>For Kubernetes configuration, see the <a href="https://github.com/PruthviProdduturi/Kaveon/tree/a2c4d3f99f4bf9e49f77726b856a59182e8d29ef/infra/helm/kaveon-engine">Engine chart</a> and <a href="https://github.com/PruthviProdduturi/Kaveon/tree/a2c4d3f99f4bf9e49f77726b856a59182e8d29ef/infra/helm/kaveon-platform">platform chart</a>. The public Azure form and its workload identity settings are specific to AKS.</p>

    <h2 id="linux-vm">Linux VM</h2>
    <p>Install Git, Bash and Docker with Compose v2, then follow the <a href="/docs/quickstart">local quickstart</a> on the VM. Use a persistent disk for the coordinator volume and any local data. For shared access, configure Studio sign-in, set its public HTTPS origin and point a TLS reverse proxy at Studio on port 3000. Keep the API and Engine on the private interface.</p>
    <Code lang="bash">{`docker compose ps
curl -fsS http://localhost:8082/api/health
curl -fsS http://localhost:8081/health`}</Code>
    <p>See <a href="/docs/deployment">Deployment topology</a> for service placement, <a href="/docs/auth">Auth &amp; RBAC</a> for identity, and <a href="/docs/operations">Operations</a> for backups and recovery.</p>

    <Pager prev={{ href: "/docs/quickstart", title: "Quickstart" }} next={{ href: "/docs/concepts", title: "Core concepts" }} />
  </div>;
}
