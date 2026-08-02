# Deploy Lens — Vercel + Render + Neon (all free, open-source data)

The fully-working, all-open-source cloud stack:

```
Browser ──► Vercel (lens-web, NextAuth: GitHub/Google/Microsoft)
               │  same-origin /api/lens proxy (injects X-User-* + secret)
               ▼
            Render (lens-api, FastAPI Docker)
               │  psycopg2 (user/password + SSL)
               ▼
            Neon (Postgres — metadata + your data)  ← open source, free tier
```

Vercel can't run `lens-api` (persistent process) so it lives on **Render**; the
DB is **Neon** (serverless Postgres). All three have free tiers.

---

## 1 · Neon — the database (metadata + data source)

1. Create a project at **https://neon.tech** → note the connection details:
   `host` (`<id>.neon.tech`), `database` (e.g. `lens`), `role` (user), `password`.
2. Apply the schema — open the Neon **SQL Editor** (or `psql`) and run the
   contents of [`apps/lens-api/schema_postgresql.sql`](../../apps/lens-api/schema_postgresql.sql).
   This creates the `datasets`, `charts`, `dashboards`, … tables.
3. (Optional, for charts) load some data — create a table and insert rows, or
   import a sample CSV via the Neon console. You'll register this same Neon DB as
   a **data source** in Lens once you're signed in.

> Neon requires TLS — `METADATA_SSLMODE=require` (already the default).

## 2 · Render — the API

1. **https://render.com** → **New → Blueprint** → connect this repo. Render reads
   [`render.yaml`](../../render.yaml) and provisions `lens-api` from its Dockerfile.
2. When prompted, fill the secrets:
   - `METADATA_HOST`, `METADATA_DATABASE`, `METADATA_USER`, `METADATA_PASSWORD` → your Neon values
   - `LENS_PROXY_SECRET` → generate one: `openssl rand -hex 24` (you'll reuse it on Vercel)
   - `WEB_URL` → leave a placeholder for now (set to your Vercel URL in step 4)
3. Deploy. Note the service URL, e.g. `https://lens-api.onrender.com`.
   Check `https://lens-api.onrender.com/api/health` → `{"status":"ok"}` once Neon is reachable.

> Free tier sleeps after ~15 min idle; first request cold-starts (~50s). The
> connection-pool warmup + heartbeat smooth this once awake.

## 3 · Vercel — the frontend

1. **https://vercel.com** → **New Project** → import this repo.
2. **Root Directory:** `apps/lens-web` (Vercel auto-detects the pnpm workspace).
3. **Environment Variables:**

   | Variable | Value |
   |---|---|
   | `API_URL` | your Render URL, e.g. `https://lens-api.onrender.com` |
   | `LENS_PROXY_SECRET` | **same** value you set on Render |
   | `AUTH_SECRET` | `openssl rand -base64 32` |
   | `AUTH_URL` | your Vercel URL, e.g. `https://lens.vercel.app` |
   | `AUTH_ADMIN_EMAILS` | your email (gets Admin) |
   | `GITHUB_ID` / `GITHUB_SECRET` | GitHub OAuth App (callback `https://<vercel-url>/api/auth/callback/github`) |
   | `GOOGLE_ID` / `GOOGLE_SECRET` | *(optional)* |
   | `AUTH_MICROSOFT_ENTRA_ID_ID` / `_SECRET` / `_ISSUER` | *(optional)* |

4. Deploy → note your Vercel URL.

## 4 · Wire them together (back-fill)

1. **Render** → set `WEB_URL` to your Vercel URL (CORS) → redeploy.
2. **GitHub OAuth App** → set Authorization callback to `https://<vercel-url>/api/auth/callback/github`.
3. **Vercel** → confirm `AUTH_URL` is the final Vercel URL → redeploy.

## 5 · Verify

1. Open your Vercel URL → **Sign in with GitHub** → land in the workspace as Admin.
2. **Data Sources** → add your Neon DB as a PostgreSQL source.
3. **Datasets → Charts → Dashboards** — build one; it persists in Neon.

---

### How auth flows in production

The browser only ever talks to Vercel. `lens-web`'s `/api/lens/*` route reads the
NextAuth session **server-side** and forwards `X-User-*` to Render, stamped with
`LENS_PROXY_SECRET`. `lens-api` trusts those headers only when the secret matches,
so nothing is spoofable and no token is handled in the browser. (Same contract as
Forge's proxy — see [`docs/guides/vercel-deployment.md`](vercel-deployment.md).)

### Notes / limits
- **Data sources** other than Fabric/Azure SQL: metadata-on-Postgres works today.
  Querying a **Postgres/MySQL data source** for chart data may need the data-source
  pool path extended (currently optimised for Fabric/Azure SQL) — track separately.
- Rotate any secret that was shared in plaintext.
