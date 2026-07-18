# Weft — Azure Deployment Guide

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
8. [Step 6 — Deploy the Two Container Apps](#8-step-6--deploy-the-two-container-apps)
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
│   ┌──────────────────────────┐       ┌──────────────────────────┐       │
│   │   weft-web              │       │   weft-api              │       │
│   │   Next.js 15             │       │   FastAPI / Python 3.11  │       │
│   │   Port 3000              │       │   Port 8080              │       │
│   │   EXTERNAL               │       │   EXTERNAL               │       │
│   │   (public HTTPS)         │       │   (public HTTPS)         │       │
│   └────────────┬─────────────┘       └────────────┬─────────────┘       │
│                │ REST/MSAL                         │ ODBC/TLS            │
└────────────────┼───────────────────────────────────┼─────────────────────┘
                 │ HTTPS (browser)                    │ TDS / ODBC 18
                 ▼                                    ▼
             End Users                    Microsoft Fabric SQL
                                          (serverless warehouse +
                                           metadata DB)

Authentication:
  • Browser → weft-web: Azure AD MSAL (PKCE, no client_secret)
  • Browser → weft-api: Azure AD Bearer tokens (RS256 verified by API)
  • weft-api → Fabric SQL: DefaultAzureCredential (Managed Identity token)
  • GitHub Actions → Azure: OIDC Workload Identity Federation (no secret)
```

### Service roles

| Service | Visibility | Purpose |
|---|---|---|
| `weft-web` | External (HTTPS) | Next.js frontend, served to browsers |
| `weft-api` | External (HTTPS) | FastAPI backend — REST API, business logic, and direct ODBC connection pool to Fabric SQL |

> The former `weft-python-proxy` Flask sidecar has been eliminated. The FastAPI backend handles ODBC connectivity in-process via `pyodbc` + `azure-identity`, removing the inter-service HTTP hop.

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
RG="weft-rg"              # your resource group name

az group create \
  --name "$RG" \
  --location "$LOCATION"
```

### 3.2 Create Azure Container Registry (ACR)

> ACR stores all Docker images. Admin credentials are **disabled** — images are pushed/pulled via Managed Identity and OIDC.

```bash
ACR_NAME="weftacr"        # globally unique; lowercase alphanumeric only

az acr create \
  --resource-group "$RG" \
  --name "$ACR_NAME" \
  --sku Basic \
  --admin-enabled false
```

Note the full login server: `${ACR_NAME}.azurecr.io`

---

## 4. Step 2 — User-Assigned Managed Identity

A single User-Assigned Managed Identity (UAMI) is assigned to both Container Apps. It is granted:
- **AcrPull** on the Container Registry (to pull images)
- **Fabric workspace member/contributor** (for the API's ODBC access to Fabric SQL via token auth)

### 4.1 Create the identity

```bash
IDENTITY_NAME="weft-identity"

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
3. Search for `weft-identity` (the display name of your UAMI)
4. Assign the **Contributor** role (allows schema read/write for the setup wizard) or **Member** role for read-only after initial setup

> **No SQL username/password is ever stored.** The API uses `azure.identity.DefaultAzureCredential` to obtain a token from the UAMI and passes it as the ODBC connection attribute (`SQL_COPT_SS_ACCESS_TOKEN`).

---

## 5. Step 3 — Container Apps Environment

Both containers share one environment (shared VNet, log analytics, etc.).

### 5.1 Create Log Analytics workspace

```bash
LAW_NAME="weft-logs"

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
CAE_NAME="weft-env"

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
2. Name: `Weft Web`
3. Supported account types: **Single tenant** (or Multi-tenant if needed)
4. Redirect URI:
   - Platform: **Single-page application (SPA)**
   - URI: `http://localhost:3000` (add your production URL after deploy; see step 6.3)
5. Click **Register**

### 6.2 Configure the app

After registration:

1. **Authentication** tab:
   - Ensure **Access tokens** and **ID tokens** are checked under Implicit grant (for MSAL)
   - Add `http://localhost:3000` and `https://<your-weft-web-fqdn>` as allowed redirect URIs
   - Set **Allow public client flows** to **Yes**

2. **API permissions** tab:
   - `Microsoft Graph` → `User.Read` (sign-in and read user profile) — already added by default
   - `Azure SQL Database` → `user_impersonation` (for Fabric SQL delegated access)

3. **Expose an API** tab:
   - Add a scope: `api://<client-id>/access_as_user`

### 6.3 Note the values

From **Overview**:
- **Application (client) ID** → `AZURE_CLIENT_ID` / `AZURE_CLIENT_ID`
- **Directory (tenant) ID** → `AZURE_TENANT_ID` / `AZURE_TENANT_ID`

> After deploying weft-web, come back and add its FQDN as a redirect URI (e.g., `https://weft-web.politebeach-abc123.eastus.azurecontainerapps.io`).

### 6.4 Configure App Roles

Weft uses Azure AD App Roles to assign user permissions. Add these four roles to the App Registration manifest.

1. Open the App Registration → **App roles** → **Create app role**
2. Create each of the following roles:

| Display name | Value | Allowed member types | Description |
|---|---|---|---|
| Weft Viewer | `Weft.Viewer` | Users/Groups | Read-only access to published dashboards |
| Weft Analyst | `Weft.Analyst` | Users/Groups | SQL Lab, chart and dataset creation |
| Weft Editor | `Weft.Editor` | Users/Groups | All Analyst permissions + publish content |
| Weft Admin | `Weft.Admin` | Users/Groups | Full access including user and data source management |

3. Assign users: **Enterprise Applications** → **[your Weft app]** → **Users and groups** → **Add user/group** → select user → select role

> **No role = no access**: Azure AD users without an assigned App Role receive a 403 "No Access" response and are shown a sign-out screen. Assign the App Role in Azure portal before they sign in.

---

## 7. Step 5 — App Registration for GitHub Actions (OIDC)

GitHub Actions authenticates to Azure using **OIDC Workload Identity Federation**. No client secret is ever created.

### 7.1 Create the App Registration

1. Azure Portal → **Azure Active Directory** → **App registrations** → **New registration**
2. Name: `Weft GitHub Actions`
3. Supported account types: **Single tenant**
4. No redirect URI needed
5. Click **Register**

### 7.2 Add a Federated Credential

1. Open the new App Registration → **Certificates & secrets** → **Federated credentials** tab
2. Click **Add credential**
3. Scenario: **GitHub Actions deploying Azure resources**
4. Fill in:
   - **Organization**: your GitHub organization/username
   - **Repository**: your repository name (e.g., `my-org/Weft`)
   - **Entity type**: **Branch**
   - **Branch**: `main`
   - **Name**: `weft-main-deploy`
5. Click **Add**

### 7.3 Grant the identity Azure RBAC permissions

```bash
# Get the App Registration's service principal object ID
GITHUB_SP_ID=$(az ad sp list \
  --display-name "Weft GitHub Actions" \
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

## 8. Step 6 — Deploy the Two Container Apps

> The first deployment is done manually via Azure CLI. After this, every push to `main` is deployed automatically by the GitHub Actions workflow.

Build and push placeholder images first:

```bash
# Login to Azure + ACR
az login
az acr login --name "$ACR_NAME"

# Build and push weft-api image
docker build -t "${ACR_NAME}.azurecr.io/weft-api:init" \
  -f apps/weft-api/Dockerfile .
docker push "${ACR_NAME}.azurecr.io/weft-api:init"

# Build and push weft-web image (placeholder build-args)
docker build \
  --build-arg API_URL="https://placeholder.example.com" \
  --build-arg AZURE_CLIENT_ID="placeholder" \
  --build-arg AZURE_TENANT_ID="placeholder" \
  --build-arg WEB_URL="http://localhost:3000" \
  -t "${ACR_NAME}.azurecr.io/weft-web:init" \
  -f apps/weft-web/Dockerfile .
docker push "${ACR_NAME}.azurecr.io/weft-web:init"
```

### 8.1 Deploy weft-api

```bash
API_APP_NAME="weft-api"
AZURE_CLIENT_ID="<your-msal-app-registration-client-id>"
AZURE_TENANT_ID="<your-azure-ad-tenant-id>"

az containerapp create \
  --name "$API_APP_NAME" \
  --resource-group "$RG" \
  --environment "$CAE_NAME" \
  --image "${ACR_NAME}.azurecr.io/weft-api:init" \
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
    "AZURE_CLIENT_ID=${AZURE_CLIENT_ID}" \
    "AZURE_TENANT_ID=${AZURE_TENANT_ID}" \
    "FABRIC_METADATA_ENDPOINT=secretref:fabric-meta-endpoint" \
    "FABRIC_METADATA_DATABASE=secretref:fabric-meta-database"
```

Set the secrets:
```bash
az containerapp secret set \
  --name "$API_APP_NAME" \
  --resource-group "$RG" \
  --secrets \
    "fabric-meta-endpoint=<your-fabric-metadata-endpoint>.datawarehouse.fabric.microsoft.com" \
    "fabric-meta-database=<your-metadata-db-name>"
```

Get the API public FQDN:
```bash
API_FQDN=$(az containerapp show \
  --name "$API_APP_NAME" \
  --resource-group "$RG" \
  --query "properties.configuration.ingress.fqdn" -o tsv)
echo "API public FQDN: $API_FQDN"
# e.g. weft-api.politebeach-abc123.eastus.azurecontainerapps.io
```

Update weft-api with its own public URL for CORS:
```bash
az containerapp update \
  --name "$API_APP_NAME" \
  --resource-group "$RG" \
  --set-env-vars "WEB_URL=https://<weft-web-fqdn>"
```

### 8.2 Deploy weft-web

The Next.js frontend bakes the API URL and AAD credentials into the bundle **at build time** via Docker `--build-arg`.

```bash
WEB_APP_NAME="weft-web"

# Rebuild the web image with correct build-args
docker build \
  --build-arg API_URL="https://${API_FQDN}" \
  --build-arg AZURE_CLIENT_ID="${AZURE_CLIENT_ID}" \
  --build-arg AZURE_TENANT_ID="${AZURE_TENANT_ID}" \
  --build-arg WEB_URL="https://<placeholder-web-fqdn>" \
  -t "${ACR_NAME}.azurecr.io/weft-web:init-configured" \
  -f apps/weft-web/Dockerfile .
docker push "${ACR_NAME}.azurecr.io/weft-web:init-configured"

az containerapp create \
  --name "$WEB_APP_NAME" \
  --resource-group "$RG" \
  --environment "$CAE_NAME" \
  --image "${ACR_NAME}.azurecr.io/weft-web:init-configured" \
  --registry-server "${ACR_NAME}.azurecr.io" \
  --registry-identity "$IDENTITY_RESOURCE_ID" \
  --user-assigned-identities "$IDENTITY_RESOURCE_ID" \
  --ingress external \
  --target-port 3000 \
  --min-replicas 1 \
  --max-replicas 5 \
  --cpu 0.5 \
  --memory 1.0Gi
```

Get the web public FQDN:
```bash
WEB_FQDN=$(az containerapp show \
  --name "$WEB_APP_NAME" \
  --resource-group "$RG" \
  --query "properties.configuration.ingress.fqdn" -o tsv)
echo "Web public FQDN: $WEB_FQDN"
# e.g. weft-web.politebeach-abc123.eastus.azurecontainerapps.io
```

**Important final step**: Now that you have the real web FQDN:
1. Add `https://${WEB_FQDN}` as a redirect URI in the MSAL App Registration (Step 6)
2. Update weft-api's `WEB_URL` env var to `https://${WEB_FQDN}` (for CORS)
3. Rebuild and redeploy weft-web with `WEB_URL=https://${WEB_FQDN}` as the `--build-arg`

---

## 9. Step 7 — Configure GitHub Repository Variables

In your GitHub repository: **Settings → Secrets and Variables → Variables → New repository variable**

Create each of the following:

| Variable | Value | Description |
|---|---|---|
| `AZURE_CLIENT_ID` | `<App Registration client ID from Step 7>` | GitHub Actions OIDC app |
| `AZURE_TENANT_ID` | `<Your Azure AD tenant ID>` | GitHub Actions OIDC |
| `AZURE_SUBSCRIPTION_ID` | `<Your Azure subscription ID>` | GitHub Actions OIDC |
| `ACR_NAME` | `weftacr` | Your ACR name (without `.azurecr.io`) |
| `RESOURCE_GROUP` | `weft-rg` | Azure resource group |
| `WEFT_API_FQDN` | `weft-api.politebeach-abc123.eastus.azurecontainerapps.io` | API FQDN **without** `https://` |
| `WEFT_WEB_URL` | `https://weft-web.politebeach-abc123.eastus.azurecontainerapps.io` | Web URL **with** `https://` |
| `AZURE_CLIENT_ID` | `<MSAL App Registration client ID from Step 6>` | Frontend MSAL + API JWT verification |
| `AZURE_TENANT_ID` | `<Your Azure AD tenant ID>` | Frontend MSAL + API JWT verification |
| `CONTAINER_APP_WEB` | `weft-web` | Container App name |
| `CONTAINER_APP_API` | `weft-api` | Container App name |

> **Why variables and not secrets?** These values are not sensitive (they're public URLs and IDs, not passwords or keys). Using **Variables** (not Secrets) makes them visible in the GitHub UI for easier auditing and maintenance. The only truly secret information — Fabric SQL endpoints and database names — live as Container App secrets in Azure, not in GitHub at all.

---

## 10. Step 8 — Environment Variables Reference

### weft-api

Authentication to Fabric SQL uses `azure.identity.DefaultAzureCredential`, which automatically picks up the Container App's User-Assigned Managed Identity — no credentials in env vars.

| Variable | Source | Description |
|---|---|---|
| `AZURE_CLIENT_ID` | Container App env var | App Registration client ID for JWT audience verification |
| `AZURE_TENANT_ID` | Container App env var | Azure AD tenant ID for JWKS endpoint construction |
| `WEB_URL` | Container App env var | Allowed CORS origin (your weft-web FQDN with `https://`) |
| `FABRIC_METADATA_ENDPOINT` | Container App secret | Fabric metadata DB ODBC hostname |
| `FABRIC_METADATA_DATABASE` | Container App secret | Fabric metadata DB name |
| `API_PORT` | Dockerfile default (`8080`) | Port gunicorn/uvicorn listens on |

### weft-web

All `NEXT_PUBLIC_*` variables are **baked into the bundle at Docker build time** via `--build-arg`. They are not runtime environment variables.

| Build Arg | Maps to | Description |
|---|---|---|
| `API_URL` | `NEXT_PUBLIC_API_BASE_URL`, `NEXT_PUBLIC_API_URL` | Full `https://` URL of weft-api |
| `AZURE_CLIENT_ID` | `NEXT_PUBLIC_AZURE_CLIENT_ID`, `NEXT_PUBLIC_AZURE_CLIENT_ID` | MSAL app client ID |
| `AZURE_TENANT_ID` | `NEXT_PUBLIC_AZURE_TENANT_ID`, `NEXT_PUBLIC_AZURE_TENANT_ID` | Azure AD tenant ID |
| `WEB_URL` | `NEXT_PUBLIC_AAD_REDIRECT_URI`, `NEXT_PUBLIC_AZURE_REDIRECT_URI` | Redirect URI after AAD login |

---

## 11. Step 9 — First-Run Setup Wizard

On first access, Weft detects that the metadata database schema has not been initialised and presents the **Setup Wizard**.

### What the wizard does

1. Checks connectivity and schema status (`GET /api/v1/setup/status`)
2. Tests the connection to the provided endpoint (`POST /api/v1/setup/test`)
3. Runs `schema.sql` against the metadata database, creating all required tables (`POST /api/v1/setup/initialize`)
4. Writes `FABRIC_METADATA_ENDPOINT` and `FABRIC_METADATA_DATABASE` to `.env` and triggers an API restart

> Once configured, the setup endpoints return `403 Forbidden` — they cannot be invoked again without manually clearing the environment variables.

### First-Admin Setup

Before switching to Azure AD auth, assign the **Admin** App Role to yourself in Azure portal (Enterprise Applications → Weft → Users and groups). Then save the auth config — you will be signed in as Admin on the next login.

### Triggering the wizard

Navigate to `https://<your-weft-web-fqdn>`. If `FABRIC_METADATA_ENDPOINT` / `FABRIC_METADATA_DATABASE` are not set, the wizard modal appears automatically after sign-in.

### Running setup manually (optional)

```bash
# Check setup status
curl https://<API_FQDN>/api/v1/setup/status

# Test a connection (only available before the app is configured)
curl -X POST https://<API_FQDN>/api/v1/setup/test \
  -H "Content-Type: application/json" \
  -d '{"endpoint": "<your-endpoint>", "database": "<your-db>"}'
```

---

## 12. Ongoing Deployments (CI/CD)

Every push to `main` triggers `.github/workflows/deploy.yml`, which:

1. **Authenticates** to Azure via OIDC (no secret exchanged)
2. **Logs in to ACR** using the OIDC identity (which has AcrPush)
3. **Builds and pushes** both Docker images tagged with the git SHA
4. **Deploys** each Container App by updating its image to the new SHA-tagged version

```
git push origin main
    └─► GitHub Actions trigger
            ├─ docker build + push: weft-api:<sha>
            ├─ docker build + push: weft-web:<sha>  (bakes API_URL, AAD creds)
            ├─ az containerapp update: weft-api
            └─ az containerapp update: weft-web
```

### Scaling

Container Apps scale to zero when idle (saving cost) and scale out under load. Minimum replicas are set to `1` to avoid cold-start delays for real users.

```bash
az containerapp update \
  --name weft-api \
  --resource-group weft-rg \
  --min-replicas 1 \
  --max-replicas 10
```

---

## 13. Troubleshooting

### Container fails to start

```bash
# View last 100 log lines
az containerapp logs show \
  --name weft-api \
  --resource-group weft-rg \
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

### API cannot connect to Fabric SQL

1. Confirm the UAMI is added as a Fabric workspace member
2. Check the Container App environment variables point to the correct endpoints
3. Test the Managed Identity token:
   ```bash
   az containerapp exec \
     --name weft-api \
     --resource-group weft-rg \
     --command "python -c \"from azure.identity import DefaultAzureCredential; t = DefaultAzureCredential().get_token('https://database.windows.net/.default'); print('Token OK:', t.token[:20])\""
   ```

### GitHub Actions OIDC authentication fails

Verify the federated credential matches **exactly**:
- Organization + repository name (case-sensitive)
- Entity type: `branch`
- Branch: `main`

Check in Azure Portal → App Registration → Certificates & secrets → Federated credentials.

### `NEXT_PUBLIC_*` variables are wrong or blank in production

These are baked at **build time**, not runtime. Update the GitHub variable (`WEFT_API_FQDN`, `AZURE_CLIENT_ID`, etc.) and **re-run the workflow** to rebuild the image with the correct values.

### User sees wrong role or cannot access content after role assignment

Role assignments from the Azure AD portal take effect on the **next sign-in** (the JWT is issued with the new roles claim).

If a user reports incorrect permissions:
1. Ask them to sign out and sign back in (refreshes the JWT roles claim)
2. Check their assignment: **Enterprise Applications → [Weft app] → Users and groups**
3. Or check the DB-level assignment at `GET /api/v1/users` (Admin only)

### First page load is slow (~10–12 seconds)

This is expected on the **very first sign-in** after a deployment or after a period of inactivity. Microsoft Fabric SQL Serverless has a cold-start time of approximately 10 seconds when no connections have been made recently. Weft mitigates this with:

- **Connection pool warmup** triggered at API startup
- **Heartbeat thread** that pings the pool every 5 minutes to keep Fabric serverless warm
- **Parallel data fetching** on the home page — all API calls fire simultaneously at sign-in

After the first cold start, subsequent page loads complete in under 1 second.

---

*Weft deployment guide — last updated March 2026*
