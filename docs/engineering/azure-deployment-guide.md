# Kaveon on Azure: deploy, connect and test

Use **Part A** if Kaveon is already deployed. Use **Part B** once to create a new
test environment. Commands below are for PowerShell. The Engine UI uses port
8080; Studio uses 3000. The AKS template deploys the Engine, not Studio/API.

## Part A — connect to an existing deployment

### 1. Install the client tools

You need Azure CLI, kubectl, kubelogin and the Kaveon CLI. On Windows x64,
install Azure CLI if it is missing, then reopen PowerShell:

```powershell
winget install --exact --id Microsoft.AzureCLI
```

Install the remaining tools and make them available in this terminal:

```powershell
az aks install-cli
$env:PATH = "$HOME\.azure-kubectl;$HOME\.azure-kubelogin;$env:PATH"
irm https://raw.githubusercontent.com/PruthviProdduturi/Kaveon/dev/scripts/install.ps1 | iex
$env:PATH = "$env:LOCALAPPDATA\kaveon\bin;$env:PATH"
kubectl version --client
kubelogin --version
kaveon --version
```

The installer downloads the latest `engine-dev` preview release. Confirm
`kaveon --help` includes `--auth` and `--ca-cert` for Microsoft sign-in.
On Linux x64 or Apple Silicon macOS, use:

```bash
curl -fsSL https://raw.githubusercontent.com/PruthviProdduturi/Kaveon/dev/scripts/install.sh | bash
```

Intel macOS and Linux ARM do not currently have prebuilt release assets.

### 2. Sign in and select Kaveon

For the existing test environment:

```powershell
az login --tenant 72f988bf-86f1-41af-91ab-2d7cd011db47
az account set --subscription eaa4a83d-8511-497c-b0bc-40aa5f0deae1
az aks get-credentials --resource-group test-prproddu-test --name kaveon-test-aks --overwrite-existing
kubelogin convert-kubeconfig -l azurecli
kubectl config current-context
kubectl get nodes
kubectl get services -n kaveon
```

If `$env:KUBECONFIG` points to a separate file, use that same file consistently
with `az aks get-credentials --file`, or clear the variable before using the default
kubeconfig. Expected context: `kaveon-test-aks`. Expected nodes: four Ready nodes.
Expected client Service: `kaveon`.

### 3. Start the tunnel

```powershell
kubectl port-forward service/kaveon 8080:8080 -n kaveon --address 127.0.0.1
```

Leave this terminal open. If 8080 is occupied, stop or move the identified local
application first. Kaveon's local Docker API now uses 8082 to avoid this conflict.

If Docker or another application holds 8080, use 18443 without stopping it:

```powershell
kubectl --context kaveon-test-aks -n kaveon port-forward service/kaveon 18443:8080 --address 127.0.0.1
```

Wait for `Forwarding from 127.0.0.1:18443 -> 8080` and leave that terminal open.
Use `https://localhost:18443` for both the CLI and UI in the following steps.
Docker is not required to connect to AKS; do not kill Docker's backend to free a port.

### 4. Open the Engine UI

Open **https://localhost:8080/ui**, then select **Sign in with Microsoft**.
Use your work account. You do not paste an Engine token or use `--user system`
to gain permissions. An administrator assigns your Engine role separately.

The test deployment uses a private CA. Have the deployment administrator provide
the **public CA certificate only**, and trust that certificate using your normal
workstation process. Do not distribute the TLS private key or Engine token files.
The certificate can be exported by an authorized cluster administrator with:

```powershell
$tls = kubectl get secret kaveon-engine-tls -n kaveon -o json | ConvertFrom-Json
$chain = [Text.Encoding]::UTF8.GetString([Convert]::FromBase64String($tls.data.'ca.crt'))
$ca = [regex]::Match($chain, '(?s)-----BEGIN CERTIFICATE-----.*?-----END CERTIFICATE-----').Value
[IO.File]::WriteAllText("$PWD\kaveon-ca.crt", $ca, [Text.UTF8Encoding]::new($false))
```

This exports the first certificate in the generated bundle, the test CA. Import
only that verified certificate when using a private CA; a publicly trusted deployed
certificate does not require this step. Certificates in this test expire after
30 days and must be rotated before expiry.

### 5. Connect the CLI in a second terminal

```powershell
kaveon --server https://localhost:8080 --ca-cert ./kaveon-ca.crt --catalog medallion --schema test
```

The CLI discovers Microsoft authentication from the Engine and first tries your
existing `az login` session for an Engine-scoped token. It renews that token through
Azure CLI when needed. If the session cannot provide a token, it falls back to a
Microsoft device sign-in URL and code. Kaveon keeps tokens in process memory.
Use `--auth azure-cli` to require Azure CLI authentication, or `--auth microsoft`
to choose device sign-in explicitly. Loopback Engine requests bypass proxies.
Neither Graph/Resource Manager tokens nor an arbitrary `--user` grant Engine access.

If Azure requests interactive consent, sign in for the Engine scope once:

```powershell
az login --tenant YOUR-TENANT-ID --scope "api://YOUR-APP-CLIENT-ID/access_as_user"
```

For a publicly trusted server certificate, omit `--ca-cert`. On the original
deployment workstation, the existing `tmp/aks-private-v2/ca.crt` also works.

For the alternate port, with the public CA saved in your home directory:

```powershell
kaveon --server https://localhost:18443 --ca-cert "$HOME\kaveon-ca.crt" --catalog medallion --schema test
```

### 6. Run a query

In the CLI, discover catalogs and select your session context:

```sql
SHOW CATALOGS;
SHOW SCHEMAS IN medallion;
USE medallion.test;
SHOW TABLES;
```

The prompt shows the selected schema, for example `kaveon:test>`. These metadata
commands are handled by the CLI through authenticated catalog APIs; they do not
imply that the Engine's SQL HTTP endpoint accepts `SHOW` or `USE`. The existing
`.catalogs`, `.schemas`, `.tables` and `.use medallion.test` shortcuts still work.
Output is printed directly without an external pager such as `less`.
Use `help` for commands and `exit` or `quit` to leave; these do not need semicolons.

Then run the fixture queries:

```sql
SELECT COUNT(*), SUM(amount_cents) FROM orders;
SELECT * FROM medallion_gold.test.daily_sales ORDER BY order_date;
```

The first query returns `10000` and `486727696`. For a one-shot check:

```powershell
kaveon --server https://localhost:8080 --ca-cert ./kaveon-ca.crt --catalog medallion --schema test --execute "SELECT COUNT(*) FROM orders"
```

## Part B — deploy a new Azure test environment

### 1. Gather these values and permissions

| Value | What to supply |
|---|---|
| Tenant ID | Your Microsoft Entra tenant |
| Subscription ID | The subscription that will pay for the resources |
| Resource group | A new isolated test group |
| Region | An allowed region with quota for four nodes |
| App/client ID | An approved single-tenant Entra application |
| Operator object ID | The Entra user to receive initial Engine admin access |

The deploying identity needs resource creation and resource-scoped role-assignment
permissions. App ownership/registration and consent permissions are separate from
Azure subscription access. No subscription policy changes are part of this guide.

Install Azure CLI, kubectl, kubelogin, Helm, Docker Desktop in Linux-container mode,
and Python. No Git or source checkout is needed. Download the preview deployment
bundle after its GitHub release is published:

```powershell
Invoke-WebRequest https://github.com/PruthviProdduturi/Kaveon/releases/download/engine-preview/kaveon-deploy.zip -OutFile kaveon-deploy.zip
Expand-Archive kaveon-deploy.zip -DestinationPath kaveon-deploy
Set-Location kaveon-deploy
```

The bundle contains infrastructure templates, the Helm chart and setup helpers.
Use its `preview-artifacts.json` to identify the exact image and chart build. This
is preview packaging; a missing release asset means publication has not finished.
For synthetic fixtures and test certificates:

```powershell
python -m pip install pyarrow==25.0.1 cryptography
az bicep install
```

### 2. Configure your Entra application

In Entra **App registrations**, create or select your approved application:

1. Use a single-tenant application and access-token version 2.
2. Under **Expose an API**, use `api://<client-id>` and add delegated scope
   `access_as_user`. Obtain whatever user/admin consent your tenant requires.
3. Under **Authentication**, add a **Single-page application** platform with
   `https://localhost:8080/ui` and `https://localhost:18443/ui` as redirect URLs.
4. Enable **Allow public client flows** for the CLI's device sign-in.
5. Under **Expose an API → Authorized client applications**, add Microsoft Azure CLI
   (`04b07795-8ddb-461a-bbee-02f9e1bf7b46`) for the `access_as_user` scope so users
   can reuse Azure login. This does not assign Engine roles or change tenant policies.
6. Keep existing settings when using a shared application. A dedicated application
   is preferable for independent deployments. No browser/CLI client secret is needed.

Microsoft's corporate tenant also requires valid Service Tree ownership for new app
registration. That is not a universal Azure requirement. Do not use another team's
application or ownership ID without authorization. See [Entra setup details](engine-entra-sign-in.md).

### 3. Set deployment variables

Replace the example values:

```powershell
$tenant = "YOUR-TENANT-ID"
$subscription = "YOUR-SUBSCRIPTION-ID"
$group = "kaveon-test-rg"
$region = "eastus"
$clientId = "YOUR-APP-CLIENT-ID"
az login --tenant $tenant
az account set --subscription $subscription
$operatorId = az ad signed-in-user show --query id -o tsv
$operatorIp = (Invoke-RestMethod https://api.ipify.org).Trim()
az group create --name $group --location $region
```

### 4. Preview and create infrastructure

```powershell
az deployment group what-if --resource-group $group --template-file infra/bicep/environments/aks-test.bicep --parameters operatorObjectId=$operatorId operatorIpCidr="$operatorIp/32"
az deployment group create --name kaveon-test-infra --resource-group $group --template-file infra/bicep/environments/aks-test.bicep --parameters operatorObjectId=$operatorId operatorIpCidr="$operatorIp/32"
$outputs = az deployment group show --name kaveon-test-infra --resource-group $group --query properties.outputs -o json | ConvertFrom-Json
$cluster = $outputs.clusterName.value
$registry = $outputs.registryName.value
$storage = $outputs.storageAccountName.value
$readerClientId = $outputs.readerClientId.value
```

Preview and deploy the isolated product transaction storage entrypoint only
after the main infrastructure is present. Its dependencies are declared
`existing`, so the full-payload what-if must show only the container and its
container-scoped role assignment as `Create` operations:

```powershell
az deployment group what-if --resource-group $group --template-file infra/bicep/environments/aks-product-transactions.bicep --parameters storageAccountName=$storage workloadIdentityName=kaveon-test-reader --result-format FullResourcePayloads
az deployment group create --name kaveon-product-transactions --resource-group $group --template-file infra/bicep/environments/aks-product-transactions.bicep --parameters storageAccountName=$storage workloadIdentityName=kaveon-test-reader
$productOutputs = az deployment group show --name kaveon-product-transactions --resource-group $group --query properties.outputs -o json | ConvertFrom-Json
$productContainer = $productOutputs.productTransactionContainer.value
$productPrefix = $productOutputs.productCatalogPrefix.value
```

This creates one system node plus three worker nodes, ACR, ADLS Gen2
bronze/silver/gold containers, network and scoped identities/RBAC. The isolated
product template adds the dedicated transaction container. The Engine identity is Storage Blob Data
Reader at the account and Storage Blob Data Contributor only at the product
container. It cannot write to bronze, silver, or gold. Defaults are
Standard_D4s_v3 and Kubernetes 1.35.7; override `nodeSize`/`kubernetesVersion` if
Azure validation requires an allowed alternative. Four VMs and associated resources
are billable. Fixed node counts and Free AKS control-plane tier are not a spending cap.

The test API endpoint is internet-reachable but requires Entra/Azure RBAC; local
accounts are disabled. Use `restrictApiToOperatorIps=true` only with stable,
verified egress CIDRs. The Engine Service itself remains private ClusterIP.

### 5. Import the published Engine image

Copy the immutable `ghcr.io/...@sha256:...` image reference from the bundle's
`preview-artifacts.json`. Import that exact image into your own ACR:

```powershell
$artifacts = Get-Content preview-artifacts.json -Raw | ConvertFrom-Json
$sourceImage = $artifacts.image_reference
$tag = "preview"
az acr import --name $registry --source $sourceImage --image "kaveon-engine:${tag}"
az acr login --name $registry
$digest = az acr repository show --name $registry --image "kaveon-engine:${tag}" --query digest -o tsv
docker pull "${registry}.azurecr.io/kaveon-engine@${digest}"
```

Stop if any command fails. Anonymous import requires the published GHCR package to
be public. Docker is used here only to read the image's public certificate roots;
users do not compile the Engine. The deployment below pins the imported digest.

### 6. Connect kubectl and prepare private configuration

```powershell
az aks get-credentials --resource-group $group --name $cluster --overwrite-existing
kubelogin convert-kubeconfig -l azurecli
kubectl get nodes
kubectl create namespace kaveon
New-Item -ItemType Directory -Force tmp/kaveon-private | Out-Null
icacls tmp/kaveon-private /inheritance:r /grant:r "$($env:USERDOMAIN)\$($env:USERNAME):(OI)(CI)F"
python scripts/aks-test-secrets.py --output tmp/kaveon-private --image "${registry}.azurecr.io/kaveon-engine@${digest}"
python scripts/configure-engine-entra.py --private tmp/kaveon-private --tenant-id $tenant --client-id $clientId --principal-object-id $operatorId
kubectl apply --server-side -f tmp/kaveon-private/secrets.json
kubectl apply --server-side -f tmp/kaveon-private/entra-secret.json
```

The generator refuses to overwrite an existing private directory containing files.
Keep these files private and out of Git. Public CA sharing is sufficient for users;
Engine admin credentials and private keys stay with the administrator.

### 7. Deploy the coordinator and three workers

```powershell
helm upgrade --install kaveon infra/helm/kaveon-test --namespace kaveon --set image.repository="${registry}.azurecr.io/kaveon-engine" --set image.digest=$digest --set coordinator.credentialsSecret=kaveon-coordinator-auth --set workloadIdentity.clientId=$readerClientId --set productTransactions.enabled=true --set productTransactions.account=$storage --set productTransactions.container=$productContainer --set productTransactions.prefix=$productPrefix
kubectl get pods -n kaveon -o wide
```

Wait until the coordinator container is Running before the next step. It may show
`0/1` initially: readiness requires the catalog created by the next step. Workers
should become Ready. Do not wait for coordinator readiness before bootstrapping an
empty catalog. The chart is included in the downloaded bundle, so no clone is
needed. Do not use `--wait` for the initial install before catalog bootstrap.
This command is for a new deployment; the existing test cluster was installed
with kubectl and needs a reviewed ownership migration before Helm can manage it.

### 8. Load and verify the medallion test data

```powershell
python scripts/generate-medallion-fixture.py --output tmp/aks-medallion
python scripts/aks-test-bundle.py --account $storage --private tmp/kaveon-private
kubectl cp tmp/kaveon-private/aks-bundle.tar.gz kaveon-coordinator-0:/tmp/aks-bundle.tar.gz -n kaveon
kubectl exec kaveon-coordinator-0 -n kaveon -- tar -xzf /tmp/aks-bundle.tar.gz -C /tmp
kubectl exec kaveon-coordinator-0 -n kaveon -- sh /tmp/aks-bundle/run.sh
kubectl get pods -n kaveon
```

Uploads run through the allowed AKS subnet using the deploying user's short-lived
Azure token. Engine reads use its separate read-only workload identity. RBAC changes
can take time to propagate; diagnose authorization failures before retrying. Fresh
bootstrap runs once; use `--queries-only` for later checks.

Expect three active workers, six finished query responses with exact fixture results,
and `UNAUTHORIZED 401`. If worker registration is still catching up on first startup,
rerun the queries-only bundle once all workers are Ready. See
`tmp/aks-medallion/expected-results.json` for every expected result.
Remove the temporary credential-bearing bundle from the coordinator after checks:

```powershell
kubectl exec kaveon-coordinator-0 -n kaveon -- rm -rf /tmp/aks-bundle /tmp/aks-bundle.tar.gz
```

Now follow **Part A**, using your tenant/subscription/resource-group/cluster values.
Other users also need AKS access plus an explicit object-ID/role entry in the
coordinator's Entra configuration. Being in the tenant alone does not grant access.

### 9. Stop compute when finished

```powershell
az aks stop --resource-group $group --name $cluster
```

Resume with `az aks start`. Disks, registry and storage can still incur charges while
AKS is stopped. This guide qualifies a test deployment: production needs reviewed
networking, certificates/rotation, backups/restore, observability, capacity and
availability requirements. The single coordinator is not highly available.

## Quick troubleshooting

| Symptom | Check |
|---|---|
| kubectl shows a different cluster | `kubectl config current-context` and `$env:KUBECONFIG` |
| kubectl times out before authentication | API network route and stable IP allowlist; credentials do not fix routing |
| Port-forward binds only `[::1]` | IPv4 port may be occupied; free it and use `--address 127.0.0.1` |
| `ERR_SSL_PROTOCOL_ERROR` | Use HTTPS for AKS Engine; ensure 8080 is not a different local HTTP service |
| Certificate warning | Trust the deployment's verified public CA; do not disable certificate validation |
| Microsoft button absent | `/v1/auth/config` must contain the configured Entra client/tenant |
| Microsoft consent required | Ask the Entra application/tenant administrator; AKS roles are separate |
| Engine returns 403 after sign-in | Check the delegated scope and user object-ID role assignment |
| CLI still asks for a static token or rejects `--auth` | Install a release containing the new CLI authentication support |
| CLI says `unexpected Microsoft device verification URL` | Reinstall the updated CLI, which accepts Microsoft's `https://login.microsoft.com/device` URL |

For a connection diagnostic, this public endpoint returns no credentials:

```powershell
Test-NetConnection 127.0.0.1 -Port 18443
curl.exe --noproxy "*" --ssl-revoke-best-effort --resolve localhost:18443:127.0.0.1 --cacert "$HOME\kaveon-ca.crt" https://localhost:18443/v1/auth/config
```

Windows curl may report unknown revocation status for the test CA. The diagnostic
uses best-effort revocation checks while retaining CA and hostname verification.
It does not disable TLS verification. The CLI uses a separate TLS library and
automatically bypasses proxies for loopback Engine connections.

## CLI 0.2.0 workflows

Re-run the installer and confirm `kaveon --version` reports `0.2.0`. Keep the
port-forward terminal open, then use a second terminal:

```powershell
kaveon https://localhost:18443/medallion/test --ca-cert "$HOME\kaveon-ca.crt"
```

The interactive prompt has a gray prefix and white active schema; `NO_COLOR`
disables colors. Up/down recall history, Ctrl+R searches it, Tab completes shell
and SQL keywords, and `exit` leaves the session. `--editing-mode VI` selects VI
bindings. Use `--no-history` to avoid saving SQL to the history file.

Run multiple statements from a UTF-8 file with `--file queries.sql`, or pass them
together with `--execute`. Batch errors stop execution; `--ignore-errors` continues
and still exits unsuccessfully if any statement failed. See the
[CLI guide](../guides/engine-cli.md) for connection defaults and output formats.
