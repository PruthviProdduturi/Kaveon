# Deploy Kaveon — Vercel + Azure Container Apps + Neon

The production cloud stack:

```
Browser ──► Vercel (kaveon-web, NextAuth: GitHub / Microsoft Entra)
               │  same-origin /api/kaveon proxy (injects X-User-* + secret)
               ▼
            Azure Container Apps (kaveon-api, FastAPI)
               │  psycopg2 (Neon) or DefaultAzureCredential (Fabric/Azure SQL)
               ▼
            Neon Postgres (metadata) + your registered data sources
```

IaC for all Azure resources lives in `infra/bicep/`. The Container App pulls its image from `kaveonacr.azurecr.io`.

---

## 1 · Neon — metadata database

1. Create a project at **https://neon.tech** → note the connection string from the console.
2. Apply the schema via the Neon SQL Editor or `psql`:
   ```
   \i apps/kaveon-api/schema_postgresql.sql
   ```
3. Set `METADATA_SSLMODE=require` (already the default).

---

## 2 · Azure Container Registry

Build and push the API image:

```bash
az acr build \
  --registry kaveonacr \
  --image kaveon-api:latest \
  apps/kaveon-api
```

Or push via GitHub Actions (see `.github/workflows/ci.yml`).

---

## 3 · Azure Container Apps

Deploy the Bicep environment:

```bash
az deployment group create \
  --resource-group kaveon-rg \
  --template-file infra/bicep/environments/prod.bicep \
  --parameters @infra/bicep/environments/prod.parameters.json
```

Set the required secrets on the Container App:

| Secret / env var | Value |
|---|---|
| `KAVEON_PROXY_SECRET` | `openssl rand -hex 24` |
| `METADATA_HOST` | Neon host (e.g. `ep-xxx.neon.tech`) |
| `METADATA_DATABASE` | Neon database name |
| `METADATA_USER` | Neon role |
| `METADATA_PASSWORD` | Neon password |
| `METADATA_DB_TYPE` | `postgresql` |
| `METADATA_SSLMODE` | `require` |
| `WEB_URL` | your Vercel URL (CORS) |

Health check: `GET https://kaveon-api.calmbeach-fe7df67b.westus2.azurecontainerapps.io/api/health` → `{"status":"ok"}`.

---

## 4 · Vercel — frontend

1. **https://vercel.com** → **New Project** → import this repo.
2. **Root Directory:** `apps/kaveon-web`.
3. **Environment Variables:**

   | Variable | Value |
   |---|---|
   | `API_URL` | your Container Apps URL |
   | `KAVEON_PROXY_SECRET` | **same** value set on the Container App |
   | `AUTH_SECRET` | `openssl rand -base64 32` |
   | `AUTH_URL` | your Vercel URL |
   | `AUTH_ADMIN_EMAILS` | your email (gets Admin) |
   | `GITHUB_ID` / `GITHUB_SECRET` | GitHub OAuth App (callback `https://<vercel-url>/api/auth/callback/github`) |
   | `AUTH_MICROSOFT_ENTRA_ID_ID` / `_SECRET` / `_ISSUER` | Microsoft Entra App Registration |

---

## 5 · Wire them together

1. **Container App** → set `WEB_URL` to your Vercel URL → redeploy.
2. **GitHub OAuth App** → add callback `https://<vercel-url>/api/auth/callback/github`.
3. **Microsoft Entra App Registration** → add redirect URI `https://<vercel-url>/api/auth/callback/microsoft-entra-id`.

---

## 6 · Verify

1. Open your Vercel URL → sign in → land as Admin.
2. **Data Sources → + Add Data Source** → add your Neon DB as a PostgreSQL source.
3. **SQL Lab** → pick the source → `SELECT 1` to confirm connectivity.
4. **Homepage** → type a question → confirm NL→SQL chart renders.

---

### Auth flow

The browser only ever talks to Vercel. `/api/kaveon/*` reads the NextAuth session server-side and forwards `X-User-*` headers to the Container App, stamped with `KAVEON_PROXY_SECRET`. The API trusts those headers only when the secret matches — nothing is spoofable and no token is handled in the browser.
