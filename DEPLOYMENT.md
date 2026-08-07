# Kaveon — Deployment Guide

> **Stack:** Vercel (kaveon-web) + Azure Container Apps (kaveon-api) + Neon (Postgres)
> **Live:** [kaveon.vercel.app](https://kaveon.vercel.app)

---

## Architecture

```
Browser ──► Vercel (kaveon-web, NextAuth: GitHub / Microsoft Entra)
               │  same-origin /api/kaveon proxy (injects X-User-* + secret)
               ▼
            Azure Container Apps (kaveon-api, FastAPI)
               │  psycopg2 (Neon) or DefaultAzureCredential (Fabric/Azure SQL)
               ▼
            Neon (Postgres — metadata) + your registered data sources
```

Vercel hosts the Next.js frontend. Azure Container Apps hosts the FastAPI backend (persistent process + connection pool). Neon provides serverless Postgres for app metadata. IaC for Azure resources lives in `infra/bicep/`.

## Full Walkthrough

See **[docs/guides/deploy-vercel-azure-neon.md](docs/guides/deploy-vercel-azure-neon.md)** for the complete step-by-step setup guide covering:

1. **Neon** — create the database, apply the schema
2. **Azure Container Registry** — build and push the API image
3. **Azure Container Apps** — deploy kaveon-api via Bicep
4. **Vercel** — deploy kaveon-web, configure NextAuth providers + env vars
5. **Wire together** — back-fill CORS, callback URLs, secrets
6. **Verify** — sign in, add a data source, build a chart

## Auth Flow

The browser only talks to Vercel. `kaveon-web`'s `/api/kaveon/*` route reads the NextAuth session server-side and forwards `X-User-*` headers to the Container App, stamped with `KAVEON_PROXY_SECRET`. `kaveon-api` trusts those headers only when the secret matches. No token is handled in the browser.

---

*See also: [ARCHITECTURE.md](ARCHITECTURE.md) · [infra/bicep/](infra/bicep/) · [apps/kaveon-web/vercel.json](apps/kaveon-web/vercel.json)*
