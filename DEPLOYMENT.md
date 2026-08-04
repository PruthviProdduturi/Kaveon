# Lens — Deployment Guide

> **Stack:** Vercel (lens-web) + Render (lens-api) + Neon (Postgres)
> **Live:** [lens-analytics.vercel.app](https://lens-analytics.vercel.app)

---

## Architecture

```
Browser ──► Vercel (lens-web, NextAuth: GitHub/Google/Microsoft)
               │  same-origin /api/lens proxy (injects X-User-* + secret)
               ▼
            Render (lens-api, FastAPI Docker)
               │  psycopg2 (user/password + SSL)
               ▼
            Neon (Postgres — metadata + your data)
```

Vercel hosts the Next.js frontend. Render hosts the FastAPI backend (persistent process + connection pool). Neon provides serverless Postgres for both app metadata and demo data. All three have free tiers.

## Full Walkthrough

See **[docs/guides/deploy-vercel-render-neon.md](docs/guides/deploy-vercel-render-neon.md)** for the complete step-by-step setup guide covering:

1. **Neon** — create the database, apply the schema
2. **Render** — deploy lens-api via Blueprint (`render.yaml`)
3. **Vercel** — deploy lens-web, configure NextAuth providers + env vars
4. **Wire together** — back-fill CORS, callback URLs, secrets
5. **Verify** — sign in, add a data source, build a chart

## Auth Flow

The browser only talks to Vercel. `lens-web`'s `/api/lens/*` route reads the NextAuth session server-side and forwards `X-User-*` headers to Render, stamped with `LENS_PROXY_SECRET`. `lens-api` trusts those headers only when the secret matches. No token is handled in the browser.

## Scope

This is a **demo/showcase deployment** — zero-cost, all open source. Not hardened for production. For production, use private networking, managed secrets, and a dedicated warehouse.

---

*See also: [ARCHITECTURE.md](ARCHITECTURE.md) · [render.yaml](render.yaml) · [vercel.json](apps/lens-web/vercel.json)*
