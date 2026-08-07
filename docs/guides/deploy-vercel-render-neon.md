# Deploy Kaveon — Vercel + Render + Neon (all free, open-source data)

> **This is a demo / showcase deployment** — a zero-cost, fully-open-source way to
> show off what Kaveon can do (multi-provider sign-in, semantic datasets, 20+ chart
> types, dashboards, SQL Lab) end-to-end against a real database. It is **not** a
> hardened production setup: it uses free tiers (cold starts), a single shared
> Postgres for both app metadata and demo data, and dev-grade secrets. For
> production, use private networking, managed secrets, and a dedicated warehouse.

The fully-working, all-open-source cloud stack:

```
Browser ──► Vercel (kaveon-web, NextAuth: GitHub/Google/Microsoft)
               │  same-origin /api/kaveon proxy (injects X-User-* + secret)
               ▼
            Render (kaveon-api, FastAPI Docker)
               │  psycopg2 (user/password + SSL)
               ▼
            Neon (Postgres — metadata + your data)  ← open source, free tier
```

Vercel can't run `kaveon-api` (persistent process) so it lives on **Render**; the
DB is **Neon** (serverless Postgres). All three have free tiers.

---

## 1 · Neon — the database (metadata + data source)

1. Create a project at **https://neon.tech** → note the connection details:
   `host` (`<id>.neon.tech`), `database` (e.g. `lens`), `role` (user), `password`.
2. Apply the schema — open the Neon **SQL Editor** (or `psql`) and run the
   contents of [`apps/kaveon-api/schema_postgresql.sql`](../../apps/kaveon-api/schema_postgresql.sql).
   This creates the `datasets`, `charts`, `dashboards`, … tables.
3. (Optional, for charts) load some data — create a table and insert rows, or
   import a sample CSV via the Neon console. You'll register this same Neon DB as
   a **data source** in Kaveon once you're signed in.

> Neon requires TLS — `METADATA_SSLMODE=require` (already the default).

## 2 · Render — the API

1. **https://render.com** → **New → Blueprint** → connect this repo. Render reads
   [`render.yaml`](../../render.yaml) and provisions `kaveon-api` from its Dockerfile.
2. When prompted, fill the secrets:
   - `METADATA_HOST`, `METADATA_DATABASE`, `METADATA_USER`, `METADATA_PASSWORD` → your Neon values
   - `KAVEON_PROXY_SECRET` → generate one: `openssl rand -hex 24` (you'll reuse it on Vercel)
   - `WEB_URL` → leave a placeholder for now (set to your Vercel URL in step 4)
3. Deploy. Note the service URL, e.g. `https://kaveon-api.onrender.com`.
   Check `https://kaveon-api.onrender.com/api/health` → `{"status":"ok"}` once Neon is reachable.

> Free tier sleeps after ~15 min idle; first request cold-starts (~50s). The
> connection-pool warmup + heartbeat smooth this once awake.

## 3 · Vercel — the frontend

1. **https://vercel.com** → **New Project** → import this repo.
2. **Root Directory:** `apps/kaveon-web` (Vercel auto-detects the pnpm workspace).
3. **Environment Variables:**

   | Variable | Value |
   |---|---|
   | `API_URL` | your Render URL, e.g. `https://kaveon-api.onrender.com` |
   | `KAVEON_PROXY_SECRET` | **same** value you set on Render |
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

## 5 · Verify — build a chart on Neon (the demo)

1. Open your Vercel URL → **Sign in with GitHub** → land in the workspace as Admin.
2. **Data Sources → + Add Data Source:**
   - **Type:** `PostgreSQL`
   - **Connection string:** your full Neon URL, e.g.
     `postgresql://user:password@ep-xxx.neon.tech/lens?sslmode=require`
   - **Database name:** the Neon database (e.g. `lens`) — this is how Kaveon keys the source
   - **Region:** `WW`
3. **SQL Lab** → pick the source → `SELECT * FROM <your_table> LIMIT 10` to confirm it queries Neon.
4. **Datasets → + New Dataset** → pick the source + table, set a dimension and a metric.
5. **Charts → + New Chart** → pick the dataset + a chart type → **Run**. The chart
   query runs against Neon and renders.
6. **Dashboards → + New Dashboard** → drop the chart in. Everything persists in Neon.

> Kaveon generates chart SQL in T-SQL and translates the common idioms (identifier
> quoting, `TOP`→`LIMIT`, `GETDATE`→`NOW`, `ISNULL`→`COALESCE`) to Postgres at
> execution — so standard aggregation charts work on Neon out of the box.

---

### How auth flows in production

The browser only ever talks to Vercel. `kaveon-web`'s `/api/kaveon/*` route reads the
NextAuth session **server-side** and forwards `X-User-*` to Render, stamped with
`KAVEON_PROXY_SECRET`. `kaveon-api` trusts those headers only when the secret matches,
so nothing is spoofable and no token is handled in the browser. (Same contract as
Forge's proxy — see [`docs/guides/vercel-deployment.md`](vercel-deployment.md).)

### Notes / limits (demo scope)
- **PostgreSQL/MySQL data sources are supported** — Kaveon builds a native pool from
  the source's connection string and translates generated T-SQL to the dialect at
  run time. Register the source with a full connection URL (credentials included).
- **Dialect caveat:** simple aggregation charts (GROUP BY dimension, SUM/COUNT/AVG)
  work on Postgres. Charts using **date grains / date formatting** emit MSSQL date
  functions (`FORMAT`, `DATEPART`, `CONVERT`, `DATEADD`) that aren't translated yet —
  those may fail on Postgres. Fine for the demo; a dialect-specific date layer is
  future work.
- The demo reuses **one Neon database** for both app metadata and the data you chart.
  In production, separate them (dedicated warehouse for data).
- The registered connection string holds credentials — it's stored in the
  `data_sources` table and never returned to the browser, but treat the demo DB as
  disposable and rotate any secret shared in plaintext.
