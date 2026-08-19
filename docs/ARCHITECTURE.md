# Kaveon — System Architecture

## Overview

Kaveon is a monorepo containing two applications:

| Application | Path | Runtime |
|-------------|------|---------|
| `kaveon-web` | `apps/kaveon-web` | Next.js 15, React 19, TypeScript |
| `kaveon-api` | `apps/kaveon-api` | FastAPI, Python 3.11 |

The package manager is **pnpm workspaces**. Shared packages live under `packages/`.

The conversational layer is the **DLM (Data Language Model)** — a per-dataset compiled context artifact that answers natural-language questions deterministically, with no hosted LLM, and for common questions **from precomputed context with no database scan** (see [DLM](#dlm-conversational-layer)). Metadata + context live in a small `kaveonmeta` plane, physically separate from the `kaveon` data warehouse.

---

## Request Flow

```mermaid
flowchart TD
    B["🌐 Browser<br/><small>session cookie · same-origin</small>"]
    W["▲ kaveon-web<br/><small>Next.js 15 · React 19 · Vercel</small>"]
    P{{"/api/kaveon/[...path] proxy<br/><small>stamps X-User-* + X-Proxy-Secret</small>"}}
    A["⚙️ kaveon-api<br/><small>FastAPI · Azure Container Apps</small>"]
    M[("🗄️ kaveonmeta<br/><small>control + context plane<br/>users · datasets · charts · dashboards<br/>dlm_* · context_*</small>")]
    D[("📊 kaveon warehouse<br/><small>the rows<br/>usage 10M · climate · ai_benchmarks…</small>")]
    X[("🔌 External sources<br/><small>Fabric SQL · Azure SQL<br/>PostgreSQL · MySQL · StarRocks</small>")]

    B ==>|"React Server Components"| W
    W ==> P
    P ==>|"trust boundary"| A
    A -->|"metadata + DLM answers"| M
    A -->|"live query fallback"| D
    A -->|"registered data sources"| X

    classDef web fill:#4A9EE8,stroke:#2b6cb0,color:#fff;
    classDef api fill:#38a169,stroke:#276749,color:#fff;
    classDef store fill:#2d3748,stroke:#1a202c,color:#fff;
    class W,P web;
    class A api;
    class M,D,X store;
```

All browser traffic hits the Next.js app. The API is never exposed to the browser directly; the proxy route is the only ingress. Metadata and precomputed **DLM** answers live in the small `kaveonmeta` plane; the `kaveon` warehouse holds the rows and is touched only on a live-query fallback.

---

## Frontend: kaveon-web

**Stack**: Next.js 15, React 19, TypeScript (strict mode), `echarts-for-react`, Monaco Editor, react-grid-layout.

### App Router structure

```mermaid
graph LR
    root["📁 app/"]
    root --> page["page.tsx<br/><small>Homepage · NL→SQL chat</small>"]
    root --> lab["lab/<br/><small>SQL Lab · Monaco</small>"]
    root --> charts["charts/<br/><small>Chart list + builder</small>"]
    root --> dash["dashboards/<br/><small>Dashboard builder</small>"]
    root --> dsrc["data-sources/"]
    root --> dsets["datasets/"]
    root --> ws["workspace/"]
    root --> settings["settings/"]
    root --> login["login/"]
    root --> api["📁 api/"]
    api --> proxy["kaveon/[...path]/<br/><small>API proxy · route.ts</small>"]
    api --> auth["auth/<br/><small>NextAuth (Auth.js v5)</small>"]

    classDef dir fill:#4A9EE8,stroke:#2b6cb0,color:#fff;
    class root,api dir;
```

### Key utilities

| File | Purpose |
|------|---------|
| `utils/nlToSql.ts` | Template-based NL→SQL engine — **fallback** path (the DLM is primary) |
| `components/ContextBanner.tsx` | Homepage banner: what context/DLM coverage exists, with hover detail |
| `components/DatasetContextPanel.tsx` | Dataset page: last-generated, duration, indexed dims/metrics, Regenerate |
| `utils/echartsTheme.ts` | Dark/light theme application for ECharts |
| `utils/msalFetch.ts` | Authenticated fetch wrapper (legacy MSAL name; auth is NextAuth) |
| `utils/querySemaphore.ts` | Client-side query concurrency control |
| `components/charts/ChartBuilderContext.tsx` | Chart type registry, SQL generation, builder state |
| `components/charts/chartPluginRegistry.ts` | Plugin system for custom chart types |
| `components/chat/InlineChart.tsx` | Chat-embedded ECharts renderer |
| `components/dashboards/DashboardCanvas.tsx` | react-grid-layout canvas |

### Theming

CSS variables drive all colors. No hardcoded values in components.

```css
var(--primary)          /* #4A9EE8 — brand blue */
var(--bg-surface)       /* card / panel background */
var(--bg-elevated)      /* modal / dropdown background */
var(--text-primary)
var(--text-muted)
var(--border)
var(--shadow-sm)
```

ECharts charts receive theme tokens through `applyChartTheme(option, isDark)` in `utils/echartsTheme.ts`. This merges axis, legend, tooltip, and background styles before the option is passed to `ReactECharts`.

---

## Backend: kaveon-api

**Stack**: FastAPI, Python 3.11, pyodbc, psycopg2, pymysql, azure-identity.

### Router layout

```mermaid
graph LR
    r["📁 routers/"]
    r --> dlm["dlm.py<br/><small>DLM · ask / route / coverage / generate</small>"]
    r --> ai["ai.py<br/><small>AI / NL→SQL assist</small>"]
    r --> auth["auth.py<br/><small>Auth info · proxy-verified</small>"]
    r --> charts["charts.py<br/><small>Chart CRUD</small>"]
    r --> dash["dashboards.py<br/><small>Dashboard CRUD</small>"]
    r --> dsrc["data_sources.py<br/><small>CRUD + test + favorites</small>"]
    r --> dsets["datasets.py<br/><small>CRUD + schema</small>"]
    r --> fav["favorites.py"]
    r --> health["health.py<br/><small>/health · /ready</small>"]
    r --> lab["lab.py<br/><small>SQL Lab · execute / history / saved</small>"]
    r --> sql["sql.py<br/><small>execute + execute-async</small>"]
    r --> users["users.py"]
    r --> setup["setup.py<br/><small>First-run wizard</small>"]

    classDef dir fill:#38a169,stroke:#276749,color:#fff;
    classDef hot fill:#d69e2e,stroke:#975a16,color:#fff;
    class r dir;
    class dlm hot;
```

### Middleware

| Module | Role |
|--------|------|
| `middleware/auth.py` | Reads `X-User-*` headers, validates `X-Proxy-Secret` |
| `middleware/permissions.py` | Role gate (`require_min_role("Analyst")`) |
| `middleware/rate_limit.py` | Sliding-window rate limiter (120 SQL/min per user, Redis-backed when `REDIS_URL` is set) |

### Connection pool

`database/pool.py` maintains per-database `ConnectionPool` instances:

- **Fabric SQL / Azure SQL** — `FabricSQLConnection` (pyodbc + Azure AD token via `DefaultAzureCredential`)
- **PostgreSQL** — `PostgreSQLConnection` (psycopg2, password or Azure AD Managed Identity)
- **MySQL / StarRocks** — `MySQLConnection` (pymysql, password or Azure AD Managed Identity)

Stale connections are detected by error pattern matching and discarded rather than recycled. The pool retries once on transient socket errors (`08S01`, `10054`, `ssl connection has been closed`, etc.).

SQL generated in T-SQL dialect is translated for PostgreSQL/MySQL targets by `adapt_sql()` before execution (handles `TOP N → LIMIT`, `ISNULL → COALESCE`, bracket quoting, etc.).

---

## DLM: Conversational Layer

The **DLM (Data Language Model)** is the primary NL→SQL path on the homepage. It is a
per-dataset compiled context artifact, built with **no hosted LLM**, on top of the
statistics-based context engine. For common questions it answers **from precomputed
context with no fact-table scan**; only novel slices fall to a single live query.

```mermaid
flowchart TD
    Q["❓ NL question<br/><small>POST /api/v1/dlm/ask</small>"]
    R["🧭 Route + resolve<br/><small>which dataset · value index<br/>terms → columns/values</small>"]
    C{"Precomputed<br/>answer shape?"}
    CTX["⚡ Answer from context<br/><small>in-memory dict hit · no DB scan</small>"]
    LIVE["🗄️ Assemble ONE live query<br/><small>execute on warehouse → cache</small>"]
    Q --> R --> C
    C -->|"total · by-dim · single-dim filter"| CTX
    C -->|"year-slice · multi-filter combo"| LIVE
    classDef ctx fill:#38a169,stroke:#276749,color:#fff;
    classDef live fill:#d69e2e,stroke:#975a16,color:#fff;
    class CTX ctx;
    class LIVE live;
```

**Engine** (`services/dlm.py`, router `routers/dlm.py`; built on `services/context_profiler.py`
+ `context_validity.py` + `context_router.py`, router `routers/context.py`). Self-migrating
tables in the `kaveonmeta` plane:

| Table | Holds |
|-------|-------|
| `dlm_artifact` | per-dataset compiled manifest + stats/usage rollups (generation timing) |
| `dlm_value_index` | value → column/filter resolution (`"anthropic"` → `provider='Anthropic'`) |
| `dlm_router` | cross-dataset routing summary/terms |
| `dlm_answers` | **precomputed answers** — each metric's grand total + per-dimension breakdown |
| `context_snapshots` | per-element profile + captured change counters (staleness signal) |

`generate_dlm()` precomputes all answers at build time (one scan for totals, one per
dimension). `ask()` serves totals, per-dimension breakdowns, and single-dimension equality
filters from an in-memory cache warmed once per dataset — microsecond dict hits, no scan.
Every answer is badged **"⚡ From context · no DB scan"** or **"Live query · Xs"** with real
timing. Endpoints: `/dlm/ask`, `/dlm/route`, `/dlm/coverage`, `/datasets/{id}/dlm[/generate]`.

---

## Auth: Identity Layer

### NextAuth (Auth.js v5)

Configured in `auth.ts`. Providers activate via environment variables:

| Env var prefix | Provider | Status |
|----------------|---------|--------|
| `GITHUB_ID` / `GITHUB_SECRET` | GitHub OAuth | Production |
| `AUTH_MICROSOFT_ENTRA_ID_ID` / `_SECRET` / `_ISSUER` | Microsoft Entra ID | Local + Production |
| `GOOGLE_ID` / `GOOGLE_SECRET` | Google OAuth | Not configured |

All routes except `/login`, `/api/auth`, and Next.js internals are protected by the middleware in `middleware.ts`.

Role assignment is environment-driven: emails listed in `AUTH_ADMIN_EMAILS` (comma-separated) receive the `Admin` role; everyone else gets `Viewer`.

### Proxy identity model

The Next.js proxy route (`/api/kaveon/[...path]/route.ts`) is the trust boundary:

1. Browser sends a request with the session cookie.
2. The route handler calls `auth()` server-side to read the verified session.
3. It stamps the FastAPI request with:
   - `X-User-Email`
   - `X-User-Name`
   - `X-User-Role`
   - `X-Proxy-Secret` (shared secret from `KAVEON_PROXY_SECRET`)
4. FastAPI validates the secret and extracts identity from these headers.

The browser cannot forge identity because it cannot set the proxy secret, and kaveon-api rejects requests that lack a valid secret.

---

## Database: Azure PostgreSQL — two planes

The platform runs on **Azure Database for PostgreSQL Flexible Server (PG 18)**, split into
two databases on the same server (Neon has been retired):

| Database | Plane | Contents |
|----------|-------|----------|
| `kaveonmeta` | **control + context** | users, data sources, datasets, charts, dashboards, favorites, query history, saved queries, **and** the `dlm_*` + `context_*` tables |
| `kaveon` | **data warehouse** | the actual rows (`kaveon_usage_daily` 10M, `climate_energy.*`, `ai_benchmarks.*`, `covid_global`, …) |

The split means context/DLM answers are served from a small, fast store that never contends
with a multi-million-row scan. Both databases authenticate over **Entra ID / Managed Identity
tokens — no stored password in production** (prod role `kaveon_api`; local dev uses the
az-login user). `database/pool.py` routes the metadata DB and any database listed in
`AAD_DATABASES` through the token-auth path, and sizes the metadata pool independently from the
warehouse pool.

Schema files (all Postgres now):
- `apps/kaveon-api/schema_postgresql.sql` — Postgres platform schema (primary)
- `apps/kaveon-api/schema.sql`, `schema_mysql.sql` — legacy SQL Server / MySQL variants

Configured via `METADATA_DATABASE` (= `kaveonmeta`), `AAD_DATABASES` (= `kaveon`),
`METADATA_HOST`, `METADATA_PORT`, `METADATA_SSLMODE`, `METADATA_DB_TYPE=postgresql`. In prod
`METADATA_USER`/`METADATA_PASSWORD` are unset — auth is via `DefaultAzureCredential`.

---

## Deployment

| Service | Platform | Config |
|---------|----------|--------|
| `kaveon-web` | Vercel (kaveon.vercel.app) | `apps/kaveon-web/vercel.json` |
| `kaveon-api` | Azure Container Apps (kaveon-api.calmbeach-fe7df67b.westus2.azurecontainerapps.io) | `infra/bicep/` |
| Database | Azure PostgreSQL Flexible Server (PG 18) — `kaveonmeta` + `kaveon` | Entra ID / Managed Identity auth |
| Container Registry | Azure Container Registry (kaveonacr.azurecr.io) | `infra/bicep/` |

### Key environment variables

**kaveon-web (Vercel)**

```
AUTH_SECRET              # openssl rand -base64 32
GITHUB_ID / GITHUB_SECRET
AUTH_MICROSOFT_ENTRA_ID_ID / _SECRET / _ISSUER
AUTH_ADMIN_EMAILS        # comma-separated
API_URL                  # kaveon-api public URL
KAVEON_PROXY_SECRET      # shared secret with kaveon-api
```

**kaveon-api (Azure Container Apps)**

```
KAVEON_PROXY_SECRET
METADATA_DATABASE        # kaveonmeta (control + context plane)
AAD_DATABASES            # kaveon — route the warehouse through token auth
METADATA_HOST            # Azure PG hostname
METADATA_PORT
METADATA_SSLMODE         # require
METADATA_DB_TYPE         # postgresql
# METADATA_USER / METADATA_PASSWORD — unset in prod; auth via Managed Identity
DATAWAREHOUSE_ENDPOINT   # fallback for unregistered data sources
REDIS_URL                # optional: enables Redis-backed rate limiting
```

---

## Azure Infrastructure

IaC templates live in `infra/bicep/`. Modules cover:

| Resource | Module |
|----------|--------|
| Azure Container Registry (`kaveonacr.azurecr.io`) | `infra/bicep/modules/acr.bicep` |
| Azure Container Apps (kaveon-api) | `infra/bicep/modules/container-apps.bicep` |
| Azure PostgreSQL Flexible Server | `infra/bicep/modules/postgresql.bicep` (migration target) |
| Azure Key Vault | `infra/bicep/modules/keyvault.bicep` |
| Log Analytics Workspace | `infra/bicep/modules/log-analytics.bicep` |

Auth uses Azure App Registration (`Kaveon`) with Managed Identity on the Container App for passwordless access to Key Vault and Azure SQL.

---

## Health Endpoints

```
GET /health    # checks metadata DB connectivity
GET /ready     # returns 200 only if the API can serve traffic
```

These are distinct intentionally: `/health` reports dependency state; `/ready` signals load-balancer readiness.
