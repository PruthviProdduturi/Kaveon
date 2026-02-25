# LoomX — Azure Deployment Guide

> **Security principle throughout this guide**: No Service Principal secrets, no client credentials, no passwords stored anywhere. Every Azure resource interaction uses either OIDC Workload Identity Federation (GitHub Actions) or Managed Identity (running services).

---

## Table of Contents

1. [Architecture Overview](#1-architecture-overview)
2. [Prerequisites](#2-prerequisites)
3. [Step 1 — Azure Resource Group & Container Registry](#3-step-1--azure-resource-group--container-registry)
4. [Step 2 — User-Assigned Managed Identity](#4-step-2--user-assigned-managed-identity)
5. [Step 3 — Container Apps Environment](#5-step-3--container-apps-environment)
6. [Step 4 — App Registration for MSAL (Frontend Auth)](#6-step-4--app-registration-for-msal-frontend-auth)
7. [Step 5 — App Registration for GitHub Actions (OIDC)](#7-step-5--app-registration-for-github-actions-oidc)
8. [Step 6 — Deploy the Three Container Apps](#8-step-6--deploy-the-three-container-apps)
9. [Step 7 — Configure GitHub Repository Variables](#9-step-7--configure-github-repository-variables)
10. [Step 8 — Environment Variables Reference](#10-step-8--environment-variables-reference)
11. [Step 9 — First-Run Setup Wizard](#11-step-9--first-run-setup-wizard)
12. [Ongoing Deployments (CI/CD)](#12-ongoing-deployments-cicd)
13. [Troubleshooting](#13-troubleshooting)

---

## 1. Architecture Overview

```
┌─────────────────────────────────────────────────────────────────────────┐
│                        Azure Container Apps Environment                  │
│                                                                          │
│   ┌──────────────────┐   ┌──────────────────┐   ┌──────────────────┐   │
│   │   loomx-web      │   │   loomx-api      │   │ loomx-python-    │   │
│   │   Next.js 15     │   │   Express/TS     │   │ proxy            │   │
│   │   Port 3000      │   │   Port 8080      │   │ Flask/gunicorn   │   │
│   │   EXTERNAL       │   │   EXTERNAL       │   │ Port 5001        │   │
│   │   (public HTTPS) │   │   (public HTTPS) │   │ INTERNAL only    │   │
│   └────────┬─────────┘   └────────┬─────────┘   └────────┬─────────┘   │
│            │                      │ REST calls            │ ODBC        │
│            │ REST/MSAL            └──────────────────────►│             │
│            │                                               │             │
└────────────┼───────────────────────────────────────────────┼─────────────┘
             │ HTTPS (browser)                                │ TDS/ODBC 18
             ▼                                                ▼
         End Users                               Microsoft Fabric SQL
                                                 (serverless warehouse +
                                                  metadata DB)

Authentication:
  • Browser → loomx-web: Azure AD MSAL (PKCE, no client_secret)
  • Browser → loomx-api: Azure AD Bearer tokens (validated by API)
  • Container Apps → Azure resources: User-Assigned Managed Identity
  • Fabric SQL: Managed Identity token (no SQL username/password)
  • GitHub Actions → Azure: OIDC Workload Identity Federation (no secret)
```

### Service roles

| Service | Visibility | Purpose |
|---|---|---|
| `loomx-web` | External (HTTPS) | Next.js frontend, served to browsers |
| `loomx-api` | External (HTTPS) | REST API consumed by browser and loomx-web SSR |
| `loomx-python-proxy` | **Internal only** | ODBC bridge to Microsoft Fabric SQL; never exposed publicly |

---

## 2. Prerequisites

| Tool | Version | Install |
|---|---|---|
| Azure CLI | ≥ 2.60 | `winget install Microsoft.AzureCLI` |
| Docker Desktop | ≥ 4.x | https://www.docker.com/products/docker-desktop |
| GitHub repository | — | Must have Actions enabled |
| Azure subscription | — | Contributor access required |
| Microsoft Fabric workspace | — | With SQL Warehouse + a separate metadata DB |

Install the Container Apps CLI extension once:
```bash
az extension add --name containerapp --upgrade
```

---

## 3. Step 1 — Azure Resource Group & Container Registry

### 3.1 Create a Resource Group

```bash
LOCATION="eastus"          # choose a region close to your users
RG="loomx-rg"              # your resource group name

az group create \
  --name "$RG" \
  --location "$LOCATION"
```

### 3.2 Create Azure Container Registry (ACR)

> ACR stores all Docker images. Admin credentials are **disabled** — images are pushed/pulled via Managed Identity and OIDC.

```bash
ACR_NAME="loomxacr"        # globally unique; lowercase alphanumeric only

az acr create \
  --resource-group "$RG" \
  --name "$ACR_NAME" \
  --sku Basic \
  --admin-enabled false
```

Note the full login server: `${ACR_NAME}.azurecr.io`

---

## 4. Step 2 — User-Assigned Managed Identity

A single User-Assigned Managed Identity (UAMI) is assigned to all three Container Apps. It is granted:
- **AcrPull** on the Container Registry (to pull images)
- **Fabric workspace member/contributor** (for Python proxy's ODBC access to Fabric SQL via token auth)

### 4.1 Create the identity

```bash
IDENTITY_NAME="loomx-identity"

az identity create \
  --resource-group "$RG" \
  --name "$IDENTITY_NAME"

# Save the resource ID and principal ID for later steps
IDENTITY_RESOURCE_ID=$(az identity show \
  --resource-group "$RG" \
  --name "$IDENTITY_NAME" \
  --query id -o tsv)

IDENTITY_PRINCIPAL_ID=$(az identity show \
  --resource-group "$RG" \
  --name "$IDENTITY_NAME" \
  --query principalId -o tsv)

echo "Identity resource ID: $IDENTITY_RESOURCE_ID"
echo "Identity principal ID (object ID): $IDENTITY_PRINCIPAL_ID"
```

### 4.2 Grant AcrPull on the Container Registry

```bash
ACR_RESOURCE_ID=$(az acr show \
  --resource-group "$RG" \
  --name "$ACR_NAME" \
  --query id -o tsv)

az role assignment create \
  --assignee "$IDENTITY_PRINCIPAL_ID" \
  --role "AcrPull" \
  --scope "$ACR_RESOURCE_ID"
```

### 4.3 Grant Fabric SQL access

Microsoft Fabric SQL uses Azure AD token authentication. The Managed Identity must be added as a member in the Fabric workspace.

1. Open the Microsoft Fabric portal → your workspace → **Manage access**
2. Click **Add people or groups**
3. Search for `loomx-identity` (the display name of your UAMI)
4. Assign the **Contributor** role (allows schema read/write for the setup wizard) or **Member** role for read-only after initial setup

> **No SQL username/password is ever stored.** The Python proxy uses `azure.identity.DefaultAzureCredential` to obtain a token from the UAMI and passes it as the ODBC password.

---

## 5. Step 3 — Container Apps Environment

All three containers share one environment (shared VNet, log analytics, etc.).

### 5.1 Create Log Analytics workspace

```bash
LAW_NAME="loomx-logs"

az monitor log-analytics workspace create \
  --resource-group "$RG" \
  --workspace-name "$LAW_NAME"

LAW_ID=$(az monitor log-analytics workspace show \
  --resource-group "$RG" \
  --workspace-name "$LAW_NAME" \
  --query customerId -o tsv)

LAW_KEY=$(az monitor log-analytics workspace get-shared-keys \
  --resource-group "$RG" \
  --workspace-name "$LAW_NAME" \
  --query primarySharedKey -o tsv)
```

### 5.2 Create the Container Apps Environment

```bash
CAE_NAME="loomx-env"

az containerapp env create \
  --resource-group "$RG" \
  --name "$CAE_NAME" \
  --location "$LOCATION" \
  --logs-workspace-id "$LAW_ID" \
  --logs-workspace-key "$LAW_KEY"
```

---

## 6. Step 4 — App Registration for MSAL (Frontend Auth)

This App Registration is what the browser uses to sign users in via Azure AD. It is a **public client** — no client secret is ever created or stored.

### 6.1 Create the App Registration

1. Azure Portal → **Azure Active Directory** → **App registrations** → **New registration**
2. Name: `LoomX Web`
3. Supported account types: **Single tenant** (or Multi-tenant if needed)
4. Redirect URI:
   - Platform: **Single-page application (SPA)**
   - URI: `http://localhost:3000` (add your production URL after deploy; see step 6.3)
5. Click **Register**

### 6.2 Configure the app

After registration:

1. **Authentication** tab:
   - Ensure **Access tokens** and **ID tokens** are checked under Implicit grant (for MSAL)
   - Add `http://localhost:3000` and `https://<your-loomx-web-fqdn>` as allowed redirect URIs
   - Set **Allow public client flows** to **Yes**

2. **API permissions** tab:
   - `Microsoft Graph` → `User.Read` (sign-in and read user profile) — already added by default
   - `Azure SQL Database` → `user_impersonation` (if accessing Fabric SQL directly from API with delegated token)

3. **Expose an API** tab (optional — only if loomx-api validates tokens with `aud`):
   - Add a scope: `api://<client-id>/access_as_user`

### 6.3 Note the values

From **Overview**:
- **Application (client) ID** → `AAD_CLIENT_ID` GitHub variable
- **Directory (tenant) ID** → `AAD_TENANT_ID` GitHub variable

> After deploying loomx-web, come back and add its FQDN as a redirect URI (e.g., `https://loomx-web.politebeach-abc123.eastus.azurecontainerapps.io`).

---

## 7. Step 5 — App Registration for GitHub Actions (OIDC)

GitHub Actions authenticates to Azure using **OIDC Workload Identity Federation**. No client secret is ever created.

### 7.1 Create the App Registration

1. Azure Portal → **Azure Active Directory** → **App registrations** → **New registration**
2. Name: `LoomX GitHub Actions`
3. Supported account types: **Single tenant**
4. No redirect URI needed
5. Click **Register**

### 7.2 Add a Federated Credential

1. Open the new App Registration → **Certificates & secrets** → **Federated credentials** tab
2. Click **Add credential**
3. Scenario: **GitHub Actions deploying Azure resources**
4. Fill in:
   - **Organization**: your GitHub organization/username
   - **Repository**: your repository name (e.g., `my-org/LoomX`)
   - **Entity type**: **Branch**
   - **Branch**: `main`
   - **Name**: `loomx-main-deploy`
5. Click **Add**

### 7.3 Grant the identity Azure RBAC permissions

The GitHub Actions identity needs to push images to ACR and deploy to Container Apps.

```bash
# Get the App Registration's service principal object ID
GITHUB_SP_ID=$(az ad sp list \
  --display-name "LoomX GitHub Actions" \
  --query "[0].id" -o tsv)

# Grant Contributor on the resource group (needed for Container Apps deploy)
SUBSCRIPTION_ID=$(az account show --query id -o tsv)

az role assignment create \
  --assignee "$GITHUB_SP_ID" \
  --role "Contributor" \
  --scope "/subscriptions/$SUBSCRIPTION_ID/resourceGroups/$RG"

# Grant AcrPush on ACR (for docker push)
az role assignment create \
  --assignee "$GITHUB_SP_ID" \
  --role "AcrPush" \
  --scope "$ACR_RESOURCE_ID"
```

### 7.4 Note the values

From the App Registration **Overview**:
- **Application (client) ID** → `AZURE_CLIENT_ID` GitHub variable
- **Directory (tenant) ID** → `AZURE_TENANT_ID` GitHub variable
- **Subscription ID** (from `az account show`) → `AZURE_SUBSCRIPTION_ID` GitHub variable

---

## 8. Step 6 — Deploy the Three Container Apps

> The first deployment is done manually via Azure CLI. After this, every push to `main` is deployed automatically by the GitHub Actions workflow.

You'll need placeholder images to create the Container Apps. Build and push them first:

```bash
# Login to Azure + ACR
az login
az acr login --name "$ACR_NAME"

# Build and push placeholder images (no build-args needed for this first push)
docker build -t "${ACR_NAME}.azurecr.io/loomx-python-proxy:init" \
  -f apps/loomx-python-proxy/Dockerfile .
docker push "${ACR_NAME}.azurecr.io/loomx-python-proxy:init"

docker build -t "${ACR_NAME}.azurecr.io/loomx-api:init" \
  -f apps/loomx-api/Dockerfile .
docker push "${ACR_NAME}.azurecr.io/loomx-api:init"

docker build \
  --build-arg API_URL="https://placeholder.example.com" \
  --build-arg AZURE_CLIENT_ID="placeholder" \
  --build-arg AZURE_TENANT_ID="placeholder" \
  --build-arg WEB_URL="http://localhost:3000" \
  -t "${ACR_NAME}.azurecr.io/loomx-web:init" \
  -f apps/loomx-web/Dockerfile .
docker push "${ACR_NAME}.azurecr.io/loomx-web:init"
```

### 8.1 Deploy loomx-python-proxy (internal)

```bash
PROXY_APP_NAME="loomx-python-proxy"

az containerapp create \
  --name "$PROXY_APP_NAME" \
  --resource-group "$RG" \
  --environment "$CAE_NAME" \
  --image "${ACR_NAME}.azurecr.io/loomx-python-proxy:init" \
  --registry-server "${ACR_NAME}.azurecr.io" \
  --registry-identity "$IDENTITY_RESOURCE_ID" \
  --user-assigned-identities "$IDENTITY_RESOURCE_ID" \
  --ingress internal \
  --target-port 5001 \
  --min-replicas 1 \
  --max-replicas 3 \
  --cpu 0.5 \
  --memory 1.0Gi \
  --env-vars \
    "FABRIC_DATAWAREHOUSE_ENDPOINT=secretref:fabric-dw-endpoint" \
    "FABRIC_DATAWAREHOUSE_DATABASE=secretref:fabric-dw-database" \
    "FABRIC_METADATA_ENDPOINT=secretref:fabric-meta-endpoint" \
    "FABRIC_METADATA_DATABASE=secretref:fabric-meta-database"
```

> **Note on secrets**: Use `az containerapp secret set` to store sensitive values (Fabric SQL endpoints, DB names) as Container App secrets, then reference them via `secretref:`. Fabric SQL **endpoints** are not truly secret (they're public hostnames), but treating them as secrets avoids baking them into environment variable definitions visible in the portal.

Set the secrets:
```bash
az containerapp secret set \
  --name "$PROXY_APP_NAME" \
  --resource-group "$RG" \
  --secrets \
    "fabric-dw-endpoint=<your-fabric-warehouse-endpoint>.datawarehouse.fabric.microsoft.com" \
    "fabric-dw-database=<your-warehouse-db-name>" \
    "fabric-meta-endpoint=<your-fabric-metadata-endpoint>.datawarehouse.fabric.microsoft.com" \
    "fabric-meta-database=<your-metadata-db-name>"
```

Get the proxy internal FQDN (needed for loomx-api):
```bash
PROXY_FQDN=$(az containerapp show \
  --name "$PROXY_APP_NAME" \
  --resource-group "$RG" \
  --query "properties.configuration.ingress.fqdn" -o tsv)
echo "Proxy internal FQDN: $PROXY_FQDN"
# e.g. loomx-python-proxy.internal.politebeach-abc123.eastus.azurecontainerapps.io
```

### 8.2 Deploy loomx-api

```bash
API_APP_NAME="loomx-api"

az containerapp create \
  --name "$API_APP_NAME" \
  --resource-group "$RG" \
  --environment "$CAE_NAME" \
  --image "${ACR_NAME}.azurecr.io/loomx-api:init" \
  --registry-server "${ACR_NAME}.azurecr.io" \
  --registry-identity "$IDENTITY_RESOURCE_ID" \
  --user-assigned-identities "$IDENTITY_RESOURCE_ID" \
  --ingress external \
  --target-port 8080 \
  --min-replicas 1 \
  --max-replicas 5 \
  --cpu 0.5 \
  --memory 1.0Gi \
  --env-vars \
    "NODE_ENV=production" \
    "PORT=8080" \
    "PYTHON_PROXY_URL=https://${PROXY_FQDN}" \
    "METADATA_DB_URL=secretref:metadata-db-url"
```

> `PYTHON_PROXY_URL` uses the internal FQDN of the proxy. Traffic stays within the Container Apps Environment and never leaves Azure's network.

Set loomx-api secrets:
```bash
az containerapp secret set \
  --name "$API_APP_NAME" \
  --resource-group "$RG" \
  --secrets \
    "metadata-db-url=<connection string for your metadata DB if loomx-api connects directly>"
```

Get the API public FQDN:
```bash
API_FQDN=$(az containerapp show \
  --name "$API_APP_NAME" \
  --resource-group "$RG" \
  --query "properties.configuration.ingress.fqdn" -o tsv)
echo "API public FQDN: $API_FQDN"
# e.g. loomx-api.politebeach-abc123.eastus.azurecontainerapps.io
```

### 8.3 Deploy loomx-web

The Next.js frontend bakes the API URL and AAD credentials into the bundle **at build time** via Docker `--build-arg`. You need the API FQDN and MSAL App Registration values before this first build.

```bash
WEB_APP_NAME="loomx-web"
AAD_CLIENT_ID="<your-msal-app-registration-client-id>"
AAD_TENANT_ID="<your-azure-ad-tenant-id>"

# Rebuild the web image with correct build-args now that you have the API FQDN
docker build \
  --build-arg API_URL="https://${API_FQDN}" \
  --build-arg AZURE_CLIENT_ID="${AAD_CLIENT_ID}" \
  --build-arg AZURE_TENANT_ID="${AAD_TENANT_ID}" \
  --build-arg WEB_URL="https://<placeholder-web-fqdn>" \
  -t "${ACR_NAME}.azurecr.io/loomx-web:init-configured" \
  -f apps/loomx-web/Dockerfile .
docker push "${ACR_NAME}.azurecr.io/loomx-web:init-configured"

az containerapp create \
  --name "$WEB_APP_NAME" \
  --resource-group "$RG" \
  --environment "$CAE_NAME" \
  --image "${ACR_NAME}.azurecr.io/loomx-web:init-configured" \
  --registry-server "${ACR_NAME}.azurecr.io" \
  --registry-identity "$IDENTITY_RESOURCE_ID" \
  --user-assigned-identities "$IDENTITY_RESOURCE_ID" \
  --ingress external \
  --target-port 3000 \
  --min-replicas 1 \
  --max-replicas 5 \
  --cpu 0.5 \
  --memory 1.0Gi \
  --env-vars "NODE_ENV=production"
```

Get the web public FQDN:
```bash
WEB_FQDN=$(az containerapp show \
  --name "$WEB_APP_NAME" \
  --resource-group "$RG" \
  --query "properties.configuration.ingress.fqdn" -o tsv)
echo "Web public FQDN: $WEB_FQDN"
# e.g. loomx-web.politebeach-abc123.eastus.azurecontainerapps.io
```

**Important final step**: Now that you have the real web FQDN, go back to the MSAL App Registration (Step 6) and:
1. Add `https://${WEB_FQDN}` as a redirect URI
2. Rebuild and redeploy loomx-web with `WEB_URL=https://${WEB_FQDN}` as the correct `--build-arg`

---

## 9. Step 7 — Configure GitHub Repository Variables

In your GitHub repository: **Settings → Secrets and Variables → Variables → New repository variable**

Create each of the following (these are non-secret configuration values; the workflow uses `vars.*`):

| Variable | Value | Description |
|---|---|---|
| `AZURE_CLIENT_ID` | `<App Registration client ID from Step 7>` | GitHub Actions OIDC app |
| `AZURE_TENANT_ID` | `<Your Azure AD tenant ID>` | GitHub Actions OIDC |
| `AZURE_SUBSCRIPTION_ID` | `<Your Azure subscription ID>` | GitHub Actions OIDC |
| `ACR_NAME` | `loomxacr` | Your ACR name (without `.azurecr.io`) |
| `RESOURCE_GROUP` | `loomx-rg` | Azure resource group |
| `LOOMX_API_FQDN` | `loomx-api.politebeach-abc123.eastus.azurecontainerapps.io` | API FQDN **without** `https://` |
| `LOOMX_WEB_URL` | `https://loomx-web.politebeach-abc123.eastus.azurecontainerapps.io` | Web URL **with** `https://` |
| `AAD_CLIENT_ID` | `<MSAL App Registration client ID from Step 6>` | Frontend MSAL auth |
| `AAD_TENANT_ID` | `<Your Azure AD tenant ID>` | Frontend MSAL auth |
| `CONTAINER_APP_WEB` | `loomx-web` | Container App name |
| `CONTAINER_APP_API` | `loomx-api` | Container App name |
| `CONTAINER_APP_PROXY` | `loomx-python-proxy` | Container App name |

> **Why variables and not secrets?** These values are not sensitive (they're public URLs and IDs, not passwords or keys). Using **Variables** (not Secrets) makes them visible in the GitHub UI for easier auditing and maintenance. The only truly secret information — Fabric SQL endpoints and database names — live as Container App secrets in Azure, not in GitHub at all.

---

## 10. Step 8 — Environment Variables Reference

### loomx-python-proxy

| Variable | Source | Description |
|---|---|---|
| `FABRIC_DATAWAREHOUSE_ENDPOINT` | Container App secret | Fabric warehouse ODBC hostname |
| `FABRIC_DATAWAREHOUSE_DATABASE` | Container App secret | Fabric warehouse DB name |
| `FABRIC_METADATA_ENDPOINT` | Container App secret | Fabric metadata DB ODBC hostname |
| `FABRIC_METADATA_DATABASE` | Container App secret | Fabric metadata DB name |
| `PORT` | Dockerfile default (`5001`) | Port gunicorn listens on |
| `FLASK_ENV` | Dockerfile default (`production`) | Disables Flask debug mode |

Authentication to Fabric SQL uses `azure.identity.DefaultAzureCredential`, which automatically picks up the Container App's User-Assigned Managed Identity — no credentials in env vars.

### loomx-api

| Variable | Source | Description |
|---|---|---|
| `NODE_ENV` | Container App env var | Set to `production` |
| `PORT` | Container App env var (`8080`) | Express listen port |
| `PYTHON_PROXY_URL` | Container App env var | Internal URL of loomx-python-proxy |
| `AZURE_CLIENT_ID` | Container App env var (optional) | UAMI client ID if needed for SDK auth |

### loomx-web

All `NEXT_PUBLIC_*` variables are **baked into the bundle at Docker build time** via `--build-arg`. They are not runtime environment variables.

| Build Arg | Maps to | Description |
|---|---|---|
| `API_URL` | `NEXT_PUBLIC_API_BASE_URL`, `NEXT_PUBLIC_API_URL` | Full `https://` URL of loomx-api |
| `AZURE_CLIENT_ID` | `NEXT_PUBLIC_AAD_CLIENT_ID`, `NEXT_PUBLIC_AZURE_CLIENT_ID` | MSAL app client ID |
| `AZURE_TENANT_ID` | `NEXT_PUBLIC_AAD_TENANT_ID`, `NEXT_PUBLIC_AZURE_TENANT_ID` | Azure AD tenant ID |
| `WEB_URL` | `NEXT_PUBLIC_AAD_REDIRECT_URI`, `NEXT_PUBLIC_AZURE_REDIRECT_URI` | Redirect URI after AAD login |

---

## 11. Step 9 — First-Run Setup Wizard

On first access, LoomX detects that the metadata database schema has not been initialised and presents the **Setup Wizard**.

### What the wizard does

1. Checks for required tables in the metadata database (`GET /api/v1/setup/status`)
2. Runs `schema.sql` against the metadata database (`POST /api/v1/setup/run`)
3. Creates all required tables (datasets, charts, dashboards, query_history, data_sources, favorites, etc.)
4. Marks setup as complete

### Triggering the wizard

Navigate to `https://<your-loomx-web-fqdn>`. If the metadata DB is empty, the wizard modal appears automatically after sign-in.

### Running setup manually (optional)

If you prefer to initialize the schema before first user access:

```bash
# Port-forward to loomx-api locally (requires Azure CLI + containerapp extension)
az containerapp exec \
  --name loomx-api \
  --resource-group loomx-rg \
  --command "wget -qO- http://localhost:8080/api/v1/setup/status"

# Or run the setup via API with a valid Bearer token:
curl -X POST https://<API_FQDN>/api/v1/setup/run \
  -H "Authorization: Bearer <your-aad-token>"
```

---

## 12. Ongoing Deployments (CI/CD)

Every push to `main` triggers `.github/workflows/deploy.yml`, which:

1. **Authenticates** to Azure via OIDC (no secret exchanged — GitHub's OIDC provider is trusted by the federated credential on the App Registration)
2. **Logs in to ACR** using the OIDC identity (which has AcrPush)
3. **Builds and pushes** all three Docker images tagged with the git SHA
4. **Deploys** each Container App by updating its image to the new SHA-tagged version

```
git push origin main
    └─► GitHub Actions trigger
            ├─ docker build + push: loomx-python-proxy:<sha>
            ├─ docker build + push: loomx-api:<sha>
            ├─ docker build + push: loomx-web:<sha>  (bakes API_URL, AAD creds)
            ├─ az containerapp update: loomx-python-proxy
            ├─ az containerapp update: loomx-api
            └─ az containerapp update: loomx-web
```

### Scaling

Container Apps scale to zero when idle (saving cost) and scale out under load. Minimum replicas are set to `1` to avoid cold-start delays for real users. Adjust in the portal or via:

```bash
az containerapp update \
  --name loomx-api \
  --resource-group loomx-rg \
  --min-replicas 1 \
  --max-replicas 10
```

---

## 13. Troubleshooting

### Container fails to start

```bash
# View last 100 log lines
az containerapp logs show \
  --name loomx-api \
  --resource-group loomx-rg \
  --tail 100
```

### Image pull errors (`Unauthorized`)

Verify the UAMI has **AcrPull** on the registry:
```bash
az role assignment list \
  --assignee "$IDENTITY_PRINCIPAL_ID" \
  --scope "$ACR_RESOURCE_ID" \
  --query "[].roleDefinitionName" -o tsv
```

### Python proxy cannot connect to Fabric SQL

1. Confirm the UAMI is added as a Fabric workspace member
2. Check the Container App environment variables point to the correct endpoints
3. Test connectivity:
   ```bash
   az containerapp exec \
     --name loomx-python-proxy \
     --resource-group loomx-rg \
     --command "python -c \"from azure.identity import DefaultAzureCredential; t = DefaultAzureCredential().get_token('https://database.windows.net/.default'); print('Token OK:', t.token[:20])\""
   ```

### GitHub Actions OIDC authentication fails

Verify the federated credential matches **exactly**:
- Organization + repository name (case-sensitive)
- Entity type: `branch`
- Branch: `main`

Check in Azure Portal → App Registration → Certificates & secrets → Federated credentials.

### `NEXT_PUBLIC_*` variables are wrong or blank in production

These are baked at **build time**, not runtime. Update the GitHub variable (`LOOMX_API_FQDN`, `AAD_CLIENT_ID`, etc.) and **re-run the workflow** to rebuild the image with the correct values.

### First page load is slow (~10–12 seconds)

This is expected on the **very first sign-in** after a deployment or after a period of inactivity. Microsoft Fabric SQL Serverless has a cold-start time of approximately 10 seconds when no connections have been made recently. LoomX mitigates this with:

- **Connection pool warmup** triggered on user sign-in (not at startup)
- **Heartbeat thread** in the Python proxy that pings the pool every 5 minutes to keep Fabric serverless warm
- **Parallel data fetching** on the home page — all API calls fire simultaneously at sign-in

After the first cold start, subsequent page loads complete in under 1 second.

---

*LoomX deployment guide — last updated February 2026*
