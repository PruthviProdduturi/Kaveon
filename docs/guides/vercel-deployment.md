# Deploying the Frontend to Vercel

This guide covers deploying `kaveon-web` (the Next.js frontend) to Vercel for a public demo or staging environment. `kaveon-api` stays on Azure Container Apps (or any HTTPS host) — see [DEPLOYMENT.md](../../DEPLOYMENT.md) for the full two-service Azure deploy.

> Unlike Forge's portal (which uses NextAuth on Vercel), Kaveon keeps its **native multi-provider auth** — Local / Azure AD (MSAL) / Google — handled by `kaveon-api`. Vercel hosts the frontend only; no NextAuth setup is required.

## Prerequisites

- [Vercel account](https://vercel.com) (sign up with GitHub)
- Vercel CLI: `npm install -g vercel`
- A reachable `kaveon-api` over HTTPS (Container Apps, or a tunnel for testing)

## 1. Create a Vercel Project

```bash
cd apps/kaveon-web
vercel link --yes
```

When prompted, select your Vercel team/account. This creates `.vercel/project.json` (gitignored).

## 2. Set Environment Variables

All `NEXT_PUBLIC_*` variables are baked into the client bundle **at build time** — set them before deploying and re-deploy after any change.

```bash
# Required — point the frontend at your API
vercel env add NEXT_PUBLIC_API_BASE_URL production   # https://<your-kaveon-api-fqdn>
vercel env add NEXT_PUBLIC_API_URL production        # same value

# Optional — only when using Azure AD (MSAL) sign-in
vercel env add NEXT_PUBLIC_AZURE_CLIENT_ID production
vercel env add NEXT_PUBLIC_AZURE_TENANT_ID production
vercel env add NEXT_PUBLIC_AAD_REDIRECT_URI production   # https://<your-project>.vercel.app
```

Never commit secrets. Local login needs no env vars at all — the JWT signing key is auto-generated and stored encrypted in `auth_config`.

## 3. Deploy

### Manual deploy

```bash
cd apps/kaveon-web
vercel --prod
```

### Auto-deploy from GitHub

1. Vercel dashboard → **Settings → Git → Connect Git Repository**
2. Select the `Kaveon` repo
3. Set **Root Directory** to `apps/kaveon-web` — Vercel detects the pnpm workspace and installs from the repo root automatically
4. **Production Branch**: `dev`

Every push to `dev` auto-deploys; pull requests get preview deployments.

Config lives in [`apps/kaveon-web/vercel.json`](../../apps/kaveon-web/vercel.json) (framework + security headers).

## 4. Wire Up Auth Redirects & CORS

1. Add `https://<your-project>.vercel.app` as a **redirect URI** in the MSAL App Registration → **Authentication → Single-page application** (only if using Azure AD).
2. Add the same URL to `kaveon-api`'s `WEB_URL` env var so CORS allows the origin.

## 5. Disable Deployment Protection (public access)

By default, Vercel team deployments require SSO to view.

1. **Settings → Deployment Protection**
2. Set Standard Protection to **Disabled** (or "Only Preview Deployments")

## Architecture on Vercel

```
Vercel (frontend only — kaveon-web)
├── /                    — home / workspace (requires sign-in)
├── /lab, /charts, …     — call kaveon-api over HTTPS (NEXT_PUBLIC_API_BASE_URL)
└── auth                 — Local / Azure AD (MSAL) / Google, via kaveon-api

kaveon-api (Azure Container Apps)  ← not on Vercel
└── holds the pyodbc connection pool + 5-min heartbeat to Fabric SQL
```

> The API is **not** deployed to Vercel — serverless functions can't hold a persistent pyodbc connection pool, which Kaveon relies on for the warm-pool + heartbeat behaviour against Fabric serverless. Keep `kaveon-api` on Container Apps.
