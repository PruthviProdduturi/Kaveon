# Kaveon production-shaped Azure deployment

Kaveon's production-shaped Azure resource is named `kaveon-aks` in the
`kaveon-rg` resource group. It is an AKS Free-tier control plane with one
system node and an autoscaled worker pool. The worker pool starts at one node,
scales to four, and uses the existing `kaveonlake` ADLS Gen2 account and
`kaveonacr` registry. The Bicep template does not recreate or overwrite lake
data.

The checked-in template is
[`infra/bicep/environments/aks-production.bicep`](../../infra/bicep/environments/aks-production.bicep).
It creates only resource-scoped identities, role assignments, networking and
the AKS cluster. It does not change subscription policies.

## GitHub Actions

The workflow [`.github/workflows/azure-aks.yml`](../../.github/workflows/azure-aks.yml)
uses Azure federated identity credentials. Configure these repository secrets:

| Secret | Purpose |
|---|---|
| `AZURE_CLIENT_ID` | GitHub Actions application/client ID |
| `AZURE_TENANT_ID` | Entra tenant ID |
| `AZURE_SUBSCRIPTION_ID` | Subscription that owns `kaveon-rg` |

Configure these repository variables:

| Variable | Default | Purpose |
|---|---|---|
| `AZURE_RESOURCE_GROUP` | `kaveon-rg` | Resource group |
| `AZURE_LOCATION` | `westus2` | Azure region |
| `AZURE_OPERATOR_OBJECT_ID` | none | Initial AKS Azure RBAC administrator |
| `KAVEON_STORAGE_ACCOUNT` | `kaveonlake` | Existing ADLS account |
| `KAVEON_REGISTRY` | `kaveonacr` | Existing ACR |

The workflow provisions the cluster and verifies the worker autoscaler bounds.
It does not create credentials or place secrets in GitHub logs. Helm release
deployment remains a separate gated step because Engine TLS and Entra secrets
must be supplied through the deployment environment's secret manager.

Studio deployment is already handled by the Vercel job in `ci.yml`; Vercel
receives the web build while AKS hosts the Engine/API workload. They are
separate deployment targets connected by the configured API/Engine URL.

## How an image reaches the cluster

One route per component, and both end in ACR with an immutable digest. AKS
pulls only from `kaveonacr`; an image anywhere else cannot be deployed, whatever
else is true of it.

| Component | Built by | Registry | Deployed by |
|---|---|---|---|
| API, Studio | `deploy-aks-platform.yml` on push to `dev` | `kaveonacr` | same workflow, digest-pinned |
| Engine | `deploy-aks-engine.yml` (`workflow_dispatch`) | `kaveonacr` | same workflow, `image_digest` + `accept_restart` |

`scripts/deploy-kaveon-aks.sh` refuses anything that is not a `sha256:` digest
for either platform image. A tag is a moving target and two pulls of one tag can
differ; a digest cannot.

### What this replaced

Three routes had grown, and a fourth nearly did:

* **Hand-built Engine tags.** Engine images were built with `az acr build` and
  branch-named tags such as `runtime-filter-object-delta-00c46d59`, outside any
  workflow. Nothing recorded which commit an image held beyond the tag, and
  the newest image in the registry could sit a day behind `dev` with nothing
  saying so.
* **CI images that cannot be deployed.** `containers.yml` and
  `engine-preview.yml` publish to `ghcr.io`. Useful for a preview; the cluster
  does not pull from there, so a green Containers run is not a deployable
  Engine.
* **`deploy-aks-engine.yml` existed and had never been run.** The route
  designed for this was unused while the ad-hoc one carried production.

If you are about to build an Engine image by hand, that is the signal that the
Engine's build is not yet wired into a workflow the way the platform's is --
not an invitation to invent a fifth tag format.

### The toolchain the image is built on

`engine/Dockerfile` builds with the context `engine/`, so the repository-root
`rust-toolchain.toml` is not in it. `engine/rust-toolchain.toml` pins the same
channel for the image build. **The two must move together**: with only the root
pin, CI compiles on the pinned toolchain while the image compiles on whatever
the `cargo-chef` base carries, which is how a change can pass `cargo fmt`,
`cargo clippy -D warnings` and the whole test suite and still fail in the
registry.

## Pausing the hosted demo

When the cluster is deliberately offline, the application surfaces say so
rather than failing silently. Two Studio variables control it:

| Variable | Purpose |
|---|---|
| `NEXT_PUBLIC_KAVEON_SERVICE_NOTICE` | The message. Unset, nothing renders. |
| `NEXT_PUBLIC_KAVEON_SERVICE_NOTICE_RETURNS` | When it is expected back, e.g. `12 November 2026`. |

Ending the window is a configuration change rather than a deploy of new code.
A dismissal is keyed on the message text, so changing the message shows it
again to people who dismissed the previous one.

The notice appears only on the application surfaces. `ClientLayout` serves
`/`, `/login` and `/docs` outside the shell and those are unaffected by the
cluster being down -- interrupting someone reading the documentation to tell
them a cluster they were not using is offline would be noise.

Studio is deployed to Vercel and does not depend on the AKS cluster, so the
product overview and the whole documentation site stay available while the
cluster is stopped. What stops is everything that reads data: SQL Lab, the
catalog, charts, dashboards and chat.

## Manual operator check

```powershell
az login
az account set --subscription 4ed07f02-b111-4eea-98ce-1c177d573a51
az aks get-credentials --resource-group kaveon-rg --name kaveon-aks --overwrite-existing
kubelogin convert-kubeconfig -l azurecli
kubectl get nodes
az aks nodepool show --resource-group kaveon-rg --cluster-name kaveon-aks --name workers `
  --query '{count:count,autoscaling:enableAutoScaling,min:minCount,max:maxCount}' -o json
```

The AKS Free tier makes the control plane free; worker VMs, disks, networking,
ACR and ADLS remain billable. Stop the cluster when it is not in use:

```powershell
az aks stop --resource-group kaveon-rg --name kaveon-aks
```

Start it again with `az aks start`. The cluster is intentionally separate from
the local Docker stack described in [Self-hosting Kaveon](self-hosting.md).
