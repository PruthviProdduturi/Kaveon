# Deploying kaveon-web to Vercel

`kaveon-web` (Next.js 15) deploys to Vercel. `kaveon-api` runs on Azure Container Apps — see [deploy-vercel-azure-neon.md](deploy-vercel-azure-neon.md) for the full two-service deploy.

## Prerequisites

- Vercel account linked to the `Kaveon` GitHub repo
- A reachable `kaveon-api` over HTTPS (Azure Container Apps)
- At least one OAuth provider configured (GitHub and/or Microsoft Entra ID)

## 1 · Link the project

```bash
cd apps/kaveon-web
vercel link --yes
```

Set **Root Directory** to `apps/kaveon-web` — Vercel detects the pnpm workspace and installs from the repo root.

## 2 · Set environment variables

```bash
# Auth (required)
vercel env add AUTH_SECRET production               # openssl rand -base64 32
vercel env add AUTH_URL production                  # https://<your-project>.vercel.app
vercel env add AUTH_ADMIN_EMAILS production         # comma-separated

# OAuth providers (configure at least one)
vercel env add GITHUB_ID production
vercel env add GITHUB_SECRET production
vercel env add AUTH_MICROSOFT_ENTRA_ID_ID production
vercel env add AUTH_MICROSOFT_ENTRA_ID_SECRET production
vercel env add AUTH_MICROSOFT_ENTRA_ID_ISSUER production  # https://login.microsoftonline.com/<tenant>/v2.0

# API proxy (required)
vercel env add API_URL production                   # https://kaveon-api.<env>.azurecontainerapps.io
vercel env add KAVEON_PROXY_SECRET production       # must match kaveon-api
```

## 3 · Deploy

### Manual

```bash
cd apps/kaveon-web
vercel --prod
```

### Auto-deploy from GitHub

1. Vercel dashboard → **Settings → Git → Connect Git Repository** → select `Kaveon`
2. **Root Directory:** `apps/kaveon-web`
3. **Production Branch:** `dev`

Every push to `dev` triggers a production deploy. Pull requests get preview deployments.

Config: [`apps/kaveon-web/vercel.json`](../../apps/kaveon-web/vercel.json).

## 4 · Wire up OAuth callbacks

**GitHub OAuth App:**
```
https://<your-project>.vercel.app/api/auth/callback/github
```

**Microsoft Entra App Registration → Authentication → Web → Redirect URIs:**
```
https://<your-project>.vercel.app/api/auth/callback/microsoft-entra-id
```

## 5 · Wire up CORS on kaveon-api

Set `WEB_URL` on the Container App to `https://<your-project>.vercel.app` and redeploy.

## Architecture

```
Browser ──► Vercel (kaveon-web)
               │  NextAuth session (server-side)
               │  /api/kaveon/[...path] proxy → stamps X-User-* + KAVEON_PROXY_SECRET
               ▼
            Azure Container Apps (kaveon-api)
```

The API is not on Vercel. Serverless functions cannot hold a persistent pyodbc connection pool; the API needs a long-lived process for the warm-pool + heartbeat behaviour.
