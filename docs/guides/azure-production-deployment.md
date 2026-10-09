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
