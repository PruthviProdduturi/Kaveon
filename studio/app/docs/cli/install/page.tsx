import { Callout, Code, PageHeader, Pager } from "../../../../components/docs/prose";

export const metadata = { title: "Install and connect — Kaveon CLI" };

export default function CliInstallDocs() {
  return <div className="docs-prose">
    <PageHeader eyebrow="CLI" title="Install and connect" lead="Put the Kaveon client on a machine, then connect to a local Engine, a private endpoint, or the qualified AKS coordinator." />
    <h2>Install</h2>
    <Code lang="powershell">{`irm https://raw.githubusercontent.com/PruthviProdduturi/Kaveon/dev/scripts/install.ps1 | iex
$env:PATH = "$env:LOCALAPPDATA\\kaveon\\bin;$env:PATH"
kaveon --version`}</Code>
    <Code lang="bash">{`curl -fsSL https://raw.githubusercontent.com/PruthviProdduturi/Kaveon/dev/scripts/install.sh | bash
kaveon --version`}</Code>
    <p>Tagged releases publish platform archives and <code>SHA256SUMS</code>. The same client supports Windows x64, macOS Intel and Apple Silicon, and Linux x64.</p>
    <h2>Local mode</h2>
    <Code lang="bash">{`kaveon --local --data-dir /data/warehouse
kaveon --local --data-dir /data/warehouse --catalog OpenSource --schema public`}</Code>
    <h2>Remote mode</h2>
    <Code lang="bash">{`kaveon --server https://engine.example.com \\
  --catalog OpenSource --schema public`}</Code>
    <p>Use <code>--auth auto</code> for a supplied token or Azure CLI login, <code>--auth azure-cli</code> to require Azure CLI, and <code>--auth microsoft</code> for device sign-in. <code>--auth none</code> is loopback development only.</p>
    <h2>Qualified AKS connection</h2>
    <Code lang="powershell">{`az login
az account set --subscription 4ed07f02-b111-4eea-98ce-1c177d573a51
az aks get-credentials --resource-group kaveon-rg --name kaveon-aks --overwrite-existing
kubelogin convert-kubeconfig -l azurecli
kubectl -n kaveon port-forward service/kaveon 18443:8080 --address 127.0.0.1
kaveon --server https://localhost:18443 --ca-cert .\\kaveon-ca.crt --catalog OpenSource --schema public`}</Code>
    <Callout type="warn">Keep the port-forward, CA file, and access token private. The coordinator is a trusted service boundary, not a public browser endpoint.</Callout>
    <Pager prev={{ href: "/docs/cli", title: "Kaveon CLI" }} next={{ href: "/docs/cli/commands", title: "Interactive commands" }} />
  </div>;
}
