<div align="center">

# ✦ LoomX

### **Live Operational Outcomes & Metrics eXperience**

*Built for Advanced Analytics. Secured by Azure AD. Owned by Pruthvi Prodduturi.*

<br>

[![Next.js](https://img.shields.io/badge/Next.js-15-black?style=for-the-badge&logo=next.js&logoColor=white)](https://nextjs.org/)
[![TypeScript](https://img.shields.io/badge/TypeScript-5.x-3178C6?style=for-the-badge&logo=typescript&logoColor=white)](https://www.typescriptlang.org/)
[![Python](https://img.shields.io/badge/Python-3.11+-3776AB?style=for-the-badge&logo=python&logoColor=white)](https://www.python.org/)
[![FastAPI](https://img.shields.io/badge/FastAPI-0.115+-009688?style=for-the-badge&logo=fastapi&logoColor=white)](https://fastapi.tiangolo.com/)
[![Azure AD](https://img.shields.io/badge/Azure_AD-Auth-0078D4?style=for-the-badge&logo=microsoftazure&logoColor=white)](https://azure.microsoft.com/en-us/products/active-directory)
[![Microsoft Fabric](https://img.shields.io/badge/Microsoft_Fabric-SQL-742774?style=for-the-badge&logo=microsoft&logoColor=white)](https://learn.microsoft.com/en-us/fabric/)
[![License](https://img.shields.io/badge/License-Proprietary-red?style=for-the-badge)](./LICENSE)

<br>

[**Quick Start**](#-quick-start) · [**Architecture**](#-architecture) · [**Features**](#-features) · [**Setup Guide**](#-step-by-step-setup) · [**Troubleshooting**](#-troubleshooting)

</div>

---

## 🌟 What is LoomX?

LoomX is a **self-hosted analytics platform** built specifically for teams running on **Microsoft Fabric SQL**. Think of it as your team's private data command centre — where everyone queries live data, builds charts, and assembles dashboards, all secured through your existing **Azure Active Directory**.

No separate login system. No data leaves your tenant. No vendor lock-in.

> *LoomX sits between your team and your Fabric data — making it fast to explore, easy to visualise, and safe to share.*

---

## ⚡ Features

<table>
<tr>
<td width="50%">

### 🔐 Enterprise Security
- Azure AD / Entra ID single sign-on
- Full JWT signature verification (RS256 via JWKS)
- Delegated user token auth — every user sees only what their AD permissions allow
- No service accounts, no password storage
- Role-Based Access Control: 4 Azure AD App Roles (Viewer, Analyst, Editor, Admin)
- Content visibility model: private / internal / published per dataset, chart, and dashboard
- S360-compliant: security headers, hardened CORS, parameterized queries throughout

</td>
<td width="50%">

### 🚀 SQL Lab
- Monaco Editor (VS Code-grade) with SQL syntax highlighting
- Multi-tab query sessions
- Real-time results with column sorting and search
- Save queries, browse full history with source tracking

</td>
</tr>
<tr>
<td width="50%">

### 📊 Chart Builder
- 20+ chart types: bar, line, area, pie, scatter, heatmap, table, and more
- Drag-and-drop metric and dimension configuration
- Smart JOIN generation from your semantic dataset definition
- Live filter dropdowns sourced directly from your data

</td>
<td width="50%">

### 🗂️ Semantic Datasets
- Define dimensions, metrics, and filter columns once
- Automatic SQL generation with multi-table JOIN support
- COALESCE-based role-playing dimension handling
- Reusable across unlimited charts and dashboards

</td>
</tr>
<tr>
<td width="50%">

### 🖥️ Dashboards
- Drag-and-drop canvas with resizable chart tiles and row/column containers
- Superset-style content blocks: rich **markdown text** (bold, italic, links, lists, code, blockquote), section headers (H1/H2/H3 with alignment + colour picker), and styled dividers
- Per-chart **⋯ context menu**: refresh, full-screen, view query, view as table, download CSV/PNG, share, duplicate, remove
- **Cross-chart filtering** — click a bar/slice to instantly filter all related charts
- Dashboard-level filter bar — shared filters slice every chart simultaneously
- Parallel chart preloading on open — all charts fetch in one pass for instant rendering
- Publish, favourite, and inline-rename dashboards

</td>
<td width="50%">

### 🎨 Personalisation
- Per-user colour theme (full palette picker)
- Theme applied instantly — no page reload, no flash
- Favourites system for datasets, charts, and dashboards
- Query history per user with full audit trail

</td>
</tr>
<tr>
<td width="50%">

### ⚡ Performance
- Connection pool warming at API startup — sub-second queries after cold start
- Parallel metadata fetching on page load
- 5-minute heartbeat keeps Fabric serverless connections alive
- All data is live — no stale cache, ever

</td>
<td width="50%">

### 🏗️ Multi-Source
- Connect unlimited Fabric warehouses and lakehouses
- Register data sources through the UI — no `.env` changes needed per source
- Endpoint discovery from metadata — zero hardcoding
- Per-database connection pooling

</td>
</tr>
</table>

---

## 🏛️ Architecture

LoomX is a **monorepo** with two services:

```
┌─────────────────────────────────────────────────────────────────┐
│                        Your Browser                              │
│                    http://localhost:3000                          │
└──────────────────────────────┬──────────────────────────────────┘
                               │  HTTPS / MSAL
                               ▼
┌─────────────────────────────────────────────────────────────────┐
│  loomx-web  ·  Next.js 15  ·  TypeScript  ·  React 19          │
│                                                                  │
│  · Azure AD login (MSAL redirect flow)                           │
│  · Chart builder, dashboard builder, SQL Lab                     │
│  · ECharts visualisations · Monaco SQL editor                    │
└──────────────────────────────┬──────────────────────────────────┘
                               │  REST API (Bearer token)
                               ▼
┌─────────────────────────────────────────────────────────────────┐
│  loomx-api  ·  FastAPI  ·  Python 3.11                          │
│                                                                  │
│  · JWT signature verification (RS256 / JWKS)                     │
│  · Route handlers · Query history logging                        │
│  · Semantic SQL generation · Dataset / chart / dashboard CRUD    │
│  · pyodbc + ODBC Driver 18 connection pool (in-process)          │
│  · Azure AD token injection via SQL_COPT_SS_ACCESS_TOKEN         │
│  · Per-database pool · Startup warmup · 5-min heartbeat          │
└──────────────────────────────┬──────────────────────────────────┘
                               │  ODBC (TLS)
                               ▼
┌─────────────────────────────────────────────────────────────────┐
│  Microsoft Fabric SQL                                            │
│                                                                  │
│  ┌─────────────────────┐   ┌──────────────────────────────────┐ │
│  │  Metadata Database  │   │  Your Warehouses & Lakehouses    │ │
│  │  (LoomX app data)   │   │  (your business data — 1 to N)   │ │
│  └─────────────────────┘   └──────────────────────────────────┘ │
└─────────────────────────────────────────────────────────────────┘
```

### Why two services?

The Python/FastAPI API handles everything in a single process — JWT validation, business logic, and direct ODBC connections to Fabric SQL. Python's `pyodbc` is the only driver that supports Azure AD interactive token injection (`SQL_COPT_SS_ACCESS_TOKEN`) required by Microsoft Fabric. Eliminating the former Node.js→Python inter-service HTTP hop reduces latency and simplifies deployment.

### Two Fabric databases

| Database | Purpose | Configured via |
|---|---|---|
| **Metadata DB** | Stores LoomX app data: datasets, charts, dashboards, query history, themes | `.env` or first-run setup wizard |
| **Your Data Sources** | Your actual Fabric warehouses and lakehouses | UI at `/data-sources` |

---

## 📋 Prerequisites

Install **all** of the following before you begin:

| Requirement | Version | Check | Install |
|---|---|---|---|
| **Python** | 3.11+ | `python --version` | [python.org](https://www.python.org/) |
| **ODBC Driver 18** | 18.x | See below | [Microsoft docs](https://learn.microsoft.com/sql/connect/odbc/download-odbc-driver-for-sql-server) |
| **Node.js** | 20.x+ | `node -v` | [nodejs.org](https://nodejs.org/) |
| **pnpm** | 9.x+ | `pnpm -v` | `npm install -g pnpm` |
| **Git** | Any | `git --version` | [git-scm.com](https://git-scm.com/) |
| **Fabric SQL access** | — | — | Contact your Fabric workspace admin |

#### Verify ODBC Driver 18

**Windows:** Start → `ODBC Data Sources (64-bit)` → **Drivers** tab → look for `ODBC Driver 18 for SQL Server`

**macOS / Linux:**
```bash
odbcinst -q -d -n "ODBC Driver 18 for SQL Server"
```

> ⚠️ ODBC Driver 18 is required. Driver 17 and older will not work with Fabric SQL's Azure AD authentication.

---

## 🔑 Azure AD App Registration

LoomX uses Azure AD for all authentication. This is a **one-time setup** by your Azure admin.

<details>
<summary><strong>Click to expand — App Registration steps</strong></summary>

<br>

1. Go to [portal.azure.com](https://portal.azure.com) → **Microsoft Entra ID** → **App registrations** → **New registration**

2. Fill in:
   - **Name:** `LoomX`
   - **Supported account types:** `Accounts in this organizational directory only`
   - **Redirect URI:** `Single-page application (SPA)` → `http://localhost:3000`

3. Click **Register**. Note down:
   - **Application (client) ID** → your `AZURE_CLIENT_ID`
   - **Directory (tenant) ID** → your `AZURE_TENANT_ID`

4. **API permissions** → **Add a permission** → **Azure SQL Database** → **Delegated** → `user_impersonation` → **Add permissions**

5. If you are an admin: **Grant admin consent for [your org]**

6. **Expose an API** → **Add a scope** → accept the default App ID URI → add scope `access_as_user`

7. **App roles** → **Create app role** → create all four roles below. These are the roles users will be assigned in **Enterprise Applications → [your app] → Users and groups**:

   | Display name | Value | Description |
   |---|---|---|
   | LoomX Viewer | `LoomX.Viewer` | Read-only access to published dashboards and charts |
   | LoomX Analyst | `LoomX.Analyst` | Run ad-hoc SQL, build charts and datasets |
   | LoomX Editor | `LoomX.Editor` | All Analyst permissions + publish content |
   | LoomX Admin | `LoomX.Admin` | Full access including user role management and data source configuration |

   > Any authenticated user without an assigned role defaults to **Viewer** automatically.

</details>

---

## 🚀 Quick Start

```bash
# 1. Clone
git clone <your-repo-url>
cd LoomX

# 2. Install Node.js dependencies (frontend only)
pnpm install

# 3. Configure environment
cp .env.example .env
# → Edit .env with your Azure AD and Fabric SQL values

# 4. Set up Python API
cd apps/loomx-api
python -m venv venv
venv\Scripts\activate        # Windows
# source venv/bin/activate   # macOS / Linux
pip install -r requirements.txt
cd ../..

# 5. Start both services — two terminals
python apps/loomx-api/main.py    # Terminal 1 (API)
pnpm --filter loomx-web dev      # Terminal 2 (Web)
```

Open **http://localhost:3000** and sign in with your Azure AD account. ✓

---

## 📖 Step-by-Step Setup

### 1 · Clone the Repository

```bash
git clone <your-github-repo-url>
cd LoomX
```

### 2 · Install Node.js Dependencies

```bash
pnpm install
```

This installs dependencies for the frontend workspace.

### 3 · Configure Environment Variables

LoomX uses a **single `.env` file at the repository root**. Both services read from it.

```bash
cp .env.example .env
```

```env
# ── Azure AD ─────────────────────────────────────────────────────────────────
# Shared by both the API (JWT verification) and the Next.js frontend (MSAL)
AZURE_TENANT_ID=xxxxxxxx-xxxx-xxxx-xxxx-xxxxxxxxxxxx
AZURE_CLIENT_ID=xxxxxxxx-xxxx-xxxx-xxxx-xxxxxxxxxxxx

# ── Metadata Database (optional — setup wizard configures this on first login) ─
# FABRIC_METADATA_ENDPOINT=your-workspace.msit-database.fabric.microsoft.com
# FABRIC_METADATA_DATABASE=YourMetadataDatabase

# ── Ports ────────────────────────────────────────────────────────────────────
API_PORT=8080
WEB_PORT=3000

# ── Internal URLs ─────────────────────────────────────────────────────────────
API_URL=http://localhost:8080
WEB_URL=http://localhost:3000
```

> 💡 **How to find your Fabric SQL endpoint:** Open your Fabric workspace → open your SQL warehouse → **Settings** gear → copy the **Server** value (e.g. `xxxxxxxx.msit-database.fabric.microsoft.com`)

### 4 · Set Up the Python API

```bash
cd apps/loomx-api

# Create virtual environment
python -m venv venv

# Activate (Windows)
venv\Scripts\activate

# Activate (macOS / Linux)
# source venv/bin/activate

# Install dependencies
pip install -r requirements.txt

cd ../..
```

#### Verify ODBC is ready

```python
python -c "import pyodbc; print([d for d in pyodbc.drivers() if '18' in d])"
# Expected: ['ODBC Driver 18 for SQL Server']
```

### 5 · Apply the Database Schema

> The setup wizard can apply the schema automatically from the browser (recommended). Manual steps below are optional.

**Option A — Setup Wizard (recommended):** Leave `FABRIC_METADATA_ENDPOINT` and `FABRIC_METADATA_DATABASE` commented out in `.env`. The wizard will appear on first login and apply the schema for you.

**Option B — Manual apply:**

1. Open **Azure Data Studio**
2. Connect to your metadata database
3. Open `apps/loomx-api/schema.sql`
4. Press **F5** to run

**Tables created:**

| Table | Purpose |
|---|---|
| `datasets` | Semantic layer definitions — dimensions, metrics, filters |
| `charts` | Chart configurations (query + visualization settings) |
| `dashboards` | Dashboard layouts and filter settings |
| `saved_queries` | User-saved SQL queries from SQL Lab |
| `query_history` | Full audit log of every executed query |
| `favorites` | Per-user favourites for any object type |
| `data_sources` | Registered Fabric warehouses and lakehouses |
| `user_themes` | Per-user colour theme preferences |
| `user_roles` | DB-level role assignments (email → role, with who granted it and when) |

> The schema uses `IF OBJECT_ID(...) IS NULL` guards — safe to re-run.

> Schema migration `apps/loomx-api/migrations/001_access_control.sql` adds the `user_roles` table and `visibility` column (DEFAULT `'internal'`) to `datasets`, `charts`, and `dashboards`. Run it after the initial `schema.sql`.

### 6 · Start Both Services

**Terminal 1 — API (Python/FastAPI)**
```bash
cd apps/loomx-api
venv\Scripts\activate   # Windows  |  source venv/bin/activate  (macOS/Linux)
python main.py
```
```
============================================
LoomX API
============================================
Server: http://localhost:8080
Health: http://localhost:8080/api/health
Docs:   http://localhost:8080/docs
============================================
[API] Connection pool warmup started.
```

**Terminal 2 — Web (Next.js)**
```bash
pnpm --filter loomx-web dev
```
```
▲ Next.js 15.x.x
  Local: http://localhost:3000
  Ready in 2s
```

### 7 · Verify

| Service | URL | Expected |
|---|---|---|
| API | http://localhost:8080/api/health | `{"status": "ok"}` or `{"status": "degraded"}` |
| API Docs | http://localhost:8080/docs | Swagger UI |
| Web | http://localhost:3000 | LoomX login page |

---

## 🎯 Your First 15 Minutes

Once all services are running:

```
A  Sign in with Microsoft  →  Azure AD popup / redirect
B  Add a Data Source       →  /data-sources  →  + Add Data Source
C  Verify in SQL Lab       →  /lab  →  pick database  →  run SELECT TOP 10 *
D  Create a Dataset        →  /datasets  →  + New Dataset  →  set dimensions + metrics
E  Build a Chart           →  /charts  →  + New Chart  →  pick dataset + chart type
F  Assemble a Dashboard    →  /dashboards  →  + New Dashboard  →  drag charts
```

After 15 minutes you will have:

```
✓ Signed in with Azure AD
✓ Data source registered and connection verified
✓ Semantic dataset defined
✓ Chart rendering live Fabric data
✓ Dashboard assembled with cross-chart filters
```

---

## 🗺️ Navigation Reference

| Page | URL | Purpose |
|---|---|---|
| **Home** | `/` | Overview, recent activity, quick access |
| **SQL Lab** | `/lab` | Write and run ad-hoc SQL with Monaco editor |
| **Saved Queries** | `/lab/queries` | Browse and reopen saved SQL queries |
| **Query History** | `/lab/queries?view=history` | Full audit log with source, duration, tables |
| **Datasets** | `/datasets` | Create and manage semantic datasets |
| **Charts** | `/charts` | Build and manage charts |
| **Dashboards** | `/dashboards` | Build and view dashboards |
| **Data Sources** | `/data-sources` | Register Fabric warehouses and lakehouses |
| **User Management** | `/settings/users` | Admin-only: assign and manage user roles |

---

## 📁 Project Structure

```
LoomX/
├── .env.example                    ← Copy to .env and fill in your values
├── .env                            ← Your local config (gitignored)
├── pnpm-workspace.yaml             ← pnpm monorepo workspace config
├── turbo.json                      ← Turborepo task pipeline
│
├── apps/
│   │
│   ├── loomx-web/                  ← Next.js 15 frontend (TypeScript)
│   │   ├── app/                    ← App Router pages
│   │   │   ├── charts/             ← Chart builder and list
│   │   │   ├── dashboards/         ← Dashboard builder and viewer
│   │   │   ├── datasets/           ← Dataset configuration
│   │   │   ├── data-sources/       ← Data source registration
│   │   │   ├── lab/                ← SQL Lab (Monaco editor)
│   │   │   ├── settings/users/page.tsx ← Admin: assign and manage user roles
│   │   │   └── layout.tsx          ← Root layout with theme + auth
│   │   ├── auth/                   ← MSAL Azure AD configuration
│   │   ├── components/             ← Reusable React components
│   │   │   ├── ConfirmModal.tsx    ← Portal-based confirm dialog (ReactDOM.createPortal)
│   │   │   ├── RoleGate.tsx        ← Render-gate component: hides UI by required role
│   │   │   ├── charts/             ← Chart builder components
│   │   │   │   └── ChartPreview.tsx← ECharts/table/KPI/map renderer + export hooks
│   │   │   └── dashboards/         ← Dashboard canvas + item components
│   │   │       ├── DashboardContext.tsx        ← Flat layout state, cross-filters, preload cache
│   │   │       ├── DashboardCanvas.tsx         ← react-grid-layout canvas + row drag handles
│   │   │       ├── DashboardItem.tsx           ← Type router → self-managed or chart card
│   │   │       └── components/
│   │   │           ├── ChartActionsOverlay.tsx ← ⋯ context menu (refresh/query/download/share)
│   │   │           ├── DashboardChartComponent.tsx
│   │   │           ├── DashboardRowComponent.tsx
│   │   │           ├── DashboardColumnComponent.tsx ← Nested children + drag-to-reorder
│   │   │           ├── DashboardTextComponent.tsx   ← Markdown renderer + formatting toolbar
│   │   │           ├── DashboardHeaderComponent.tsx ← H1/H2/H3 + alignment + colour picker
│   │   │           ├── DashboardDividerComponent.tsx
│   │   │           └── DashboardTabsComponent.tsx
│   │   ├── contexts/               ← Theme, auth context providers
│   │   ├── hooks/                  ← Custom React hooks
│   │   │   └── useRole.ts          ← Returns the current user's resolved RBAC role
│   │   └── utils/                  ← MSAL fetch, colour utilities
│   │
│   └── loomx-api/                  ← Python/FastAPI REST API
│       ├── main.py                 ← FastAPI entry point
│       ├── config.py               ← Pydantic settings (reads root .env)
│       ├── requirements.txt        ← Python dependencies
│       ├── schema.sql              ← Run once in your metadata database
│       ├── database/               ← Connection pool + metadata queries
│       │   ├── pool.py             ← pyodbc pool with Azure AD token auth
│       │   ├── metadata.py         ← Parameterized metadata DB helpers
│       │   └── warmup.py           ← Startup warmup + 5-min heartbeat
│       ├── middleware/             ← Auth and error handling
│       │   ├── auth.py             ← JWT RS256 verification (PyJWT + JWKS)
│       │   ├── permissions.py      ← RBAC dependency: resolves role, enforces minimum role
│       │   ├── rate_limit.py       ← Per-user in-memory rate limiter
│       │   └── errors.py           ← Exception handlers (no detail leakage)
│       ├── routers/                ← API route handlers (one file per domain)
│       │   ├── auth.py, health.py
│       │   ├── datasets.py, charts.py, dashboards.py
│       │   ├── data_sources.py, favorites.py
│       │   ├── lab.py, sql.py
│       │   ├── theme.py, metadata_summary.py
│       │   ├── users.py            ← Admin endpoints: list users, assign/revoke roles
│       │   └── setup.py
│       └── services/               ← Business logic
│           ├── query_generator.py  ← Star-schema SQL builder
│           ├── datasets.py, charts.py, dashboards.py
│           ├── favorites.py, saved_queries.py
│           ├── query_history.py, theme.py
│           ├── users.py            ← Role lookup, grant/revoke, JWT claims resolution
│           └── sql_table_extractor.py
│
└── packages/
    └── types/                      ← Shared TypeScript type definitions
```

---

## ⚙️ Environment Variable Reference

| Variable | Required | Default | Description |
|---|---|---|---|
| `AZURE_TENANT_ID` | ✅ | — | Azure AD tenant ID — used by the API for JWT verification and by the frontend (MSAL) |
| `AZURE_CLIENT_ID` | ✅ | — | App Registration client ID — used by the API for JWT audience check and by the frontend (MSAL) |
| `FABRIC_METADATA_ENDPOINT` | ⬜ | — | SQL endpoint of your Fabric metadata database (UI-configurable via setup wizard) |
| `FABRIC_METADATA_DATABASE` | ⬜ | — | Database name in that Fabric endpoint (UI-configurable via setup wizard) |
| `API_PORT` | ⬜ | `8080` | Port for the FastAPI service |
| `WEB_PORT` | ⬜ | `3000` | Port for the Next.js web app |
| `API_URL` | ✅ | `http://localhost:8080` | Full URL of the API, used by the web app |
| `WEB_URL` | ✅ | `http://localhost:3000` | Full URL of the web app, used for CORS |

> Data warehouse endpoints are **not** configured here. Register them through the UI at `/data-sources` after first run.

---

## 🛠️ Available Scripts

Run from the **repository root**:

| Command | Description |
|---|---|
| `pnpm dev` | Start Next.js frontend in development mode |
| `pnpm build` | Build the frontend for production |
| `pnpm check-types` | TypeScript type checking |
| `pnpm clean` | Delete all build artifacts |
| `pnpm --filter loomx-web dev` | Start only the web app |

**Python API** (from `apps/loomx-api/`):

| Command | Description |
|---|---|
| `python main.py` | Start API in development mode (uvicorn with reload) |
| `gunicorn main:app -w 4 -k uvicorn.workers.UvicornWorker` | Production start |

---

## 🔧 Troubleshooting

<details>
<summary><strong>🔴 ODBC Driver not found / API fails to connect</strong></summary>

- Verify ODBC Driver 18 is installed (see [Prerequisites](#-prerequisites))
- Make sure the Python virtual environment is activated before starting the API
- On Windows, try running the terminal **as Administrator** during initial setup

</details>

<details>
<summary><strong>🔴 Login redirects to error or blank screen</strong></summary>

- Confirm `http://localhost:3000` is registered as a redirect URI in your App Registration under **Authentication → Single-page application**
- Clear browser cookies and localStorage for `localhost:3000`
- Double-check `AZURE_CLIENT_ID` and `AZURE_TENANT_ID` in `.env`

</details>

<details>
<summary><strong>🔴 API returns 401 Unauthorized</strong></summary>

- Your Azure AD token may have expired — sign out and sign back in
- If `AZURE_CLIENT_ID` / `AZURE_TENANT_ID` are not set, the API falls back to unverified JWT decode (setup mode only)
- If you changed `.env`, restart the API service (it does not hot-reload env vars)

</details>

<details>
<summary><strong>🔴 Cannot connect to metadata database</strong></summary>

- Double-check `FABRIC_METADATA_ENDPOINT` and `FABRIC_METADATA_DATABASE` in `.env`
- Confirm your Azure AD account (or Managed Identity in production) has `db_datareader` + `db_datawriter` + `db_ddladmin` on the metadata database
- Test the connection directly in Azure Data Studio to rule out network issues

</details>

<details>
<summary><strong>🔴 Queries time out on first run</strong></summary>

Fabric serverless endpoints have a cold start (~10s on first connection). LoomX warms the connection pool at API startup. If you're still hitting timeouts, the pool warmup may not have had time to complete before your first request. Wait a few seconds after seeing `[API] Connection pool warmup started.` in the API logs.

</details>

<details>
<summary><strong>🔴 Build errors after pulling new changes</strong></summary>

```bash
# Remove all node_modules and reinstall
rm -rf node_modules apps/*/node_modules packages/*/node_modules
pnpm install

# Clear Next.js cache
rm -rf apps/loomx-web/.next

# Reinstall Python dependencies
cd apps/loomx-api && pip install -r requirements.txt

# Re-check types
pnpm check-types
```

</details>

<details>
<summary><strong>🔴 Port already in use</strong></summary>

**Windows:**
```bash
netstat -ano | findstr :8080
taskkill /PID <PID> /F
```

**macOS / Linux:**
```bash
lsof -ti :8080 | xargs kill -9
```

Or change `API_PORT` in `.env` and restart.

</details>

---

## 🧰 Tech Stack

### Frontend — `loomx-web`

| Technology | Version | Role |
|---|---|---|
| [Next.js](https://nextjs.org/) | 15.x | React framework, App Router, SSR |
| [React](https://react.dev/) | 19.x | UI component library |
| [TypeScript](https://www.typescriptlang.org/) | 5.x | End-to-end type safety |
| [@azure/msal-browser](https://github.com/AzureAD/microsoft-authentication-library-for-js) | 5.x | Azure AD authentication |
| [ECharts](https://echarts.apache.org/) | 5.x | Data visualisation (20+ chart types) |
| [Monaco Editor](https://microsoft.github.io/monaco-editor/) | 0.52.x | VS Code-grade SQL editor |
| [react-grid-layout](https://github.com/react-grid-layout/react-grid-layout) | 2.x | Drag-and-drop dashboard builder |
| [react-colorful](https://omgovich.github.io/react-colorful/) | 5.x | Colour picker for user themes |

### Backend — `loomx-api`

| Technology | Version | Role |
|---|---|---|
| [FastAPI](https://fastapi.tiangolo.com/) | 0.115+ | ASGI HTTP framework with dependency injection |
| [Python](https://www.python.org/) | 3.11+ | Runtime |
| [Uvicorn](https://www.uvicorn.org/) | 0.30+ | ASGI server (development + production worker) |
| [Gunicorn](https://gunicorn.org/) | 22+ | Process manager (production) |
| [pyodbc](https://github.com/mkleehammer/pyodbc) | 5.x | ODBC Driver 18 wrapper for Fabric SQL |
| [azure-identity](https://pypi.org/project/azure-identity/) | 1.x | DefaultAzureCredential for Managed Identity |
| [PyJWT](https://pyjwt.readthedocs.io/) | 2.x | JWT decoding and RS256 signature verification |
| [pydantic-settings](https://docs.pydantic.dev/latest/concepts/pydantic_settings/) | 2.x | Typed configuration from environment variables |
| [python-dotenv](https://pypi.org/project/python-dotenv/) | 1.x | Reads root `.env` file |

### Monorepo Tooling

| Technology | Role |
|---|---|
| [pnpm](https://pnpm.io/) | Fast, disk-efficient package manager with workspace support |
| [Turborepo](https://turbo.build/) | Monorepo task runner with incremental builds |

---

<div align="center">

**Built for Advanced Analytics. Secured by Azure AD. Owned by Pruthvi Prodduturi.**

*LoomX — Live Operational Outcomes & Metrics eXperience*

</div>
