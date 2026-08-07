# Kaveon — Deployment Guide

> **Stack:** Vercel (kaveon-web) + Render (kaveon-api) + Neon (Postgres)
> **Live:** [lens-analytics.vercel.app](https://lens-analytics.vercel.app)

---

## Architecture

```
Browser ──► Vercel (kaveon-web, NextAuth: GitHub/Google/Microsoft)
               │  same-origin /api/kaveon proxy (injects X-User-* + secret)
               ▼
            Render (kaveon-api, FastAPI Docker)
               │  psycopg2 (user/password + SSL)
               ▼
            Neon (Postgres — metadata + your data)
```

Vercel hosts the Next.js frontend. Render hosts the FastAPI backend (persistent process + connection pool). Neon provides serverless Postgres for both app metadata and demo data. All three have free tiers.

## Full Walkthrough

See **[docs/guides/deploy-vercel-render-neon.md](docs/guides/deploy-vercel-render-neon.md)** for the complete step-by-step setup guide covering:

1. **Neon** — create the database, apply the schema
2. **Render** — deploy kaveon-api via Blueprint (`render.yaml`)
3. **Vercel** — deploy kaveon-web, configure NextAuth providers + env vars
4. **Wire together** — back-fill CORS, callback URLs, secrets
5. **Verify** — sign in, add a data source, build a chart

## Auth Flow

The browser only talks to Vercel. `kaveon-web`'s `/api/kaveon/*` route reads the NextAuth session server-side and forwards `X-User-*` headers to Render, stamped with `KAVEON_PROXY_SECRET`. `kaveon-api` trusts those headers only when the secret matches. No token is handled in the browser.

## Scope

This is a **demo/showcase deployment** — zero-cost, all open source. Not hardened for production. For production, use private networking, managed secrets, and a dedicated warehouse.

---

*See also: [ARCHITECTURE.md](ARCHITECTURE.md) · [render.yaml](render.yaml) · [vercel.json](apps/kaveon-web/vercel.json)*
