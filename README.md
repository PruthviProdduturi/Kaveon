<div align="center">

# ✦ LoomX

### **Live Operational Outcomes & Metrics eXperience**

*A self-hosted, enterprise-grade data exploration and visualization platform<br>built natively for Microsoft Fabric SQL — secured by Azure AD, zero compromises.*

*Owned by Pruthvi Prodduturi*

<br>

[![Next.js](https://img.shields.io/badge/Next.js-15-black?style=for-the-badge&logo=next.js&logoColor=white)](https://nextjs.org/)
[![TypeScript](https://img.shields.io/badge/TypeScript-5.x-3178C6?style=for-the-badge&logo=typescript&logoColor=white)](https://www.typescriptlang.org/)
[![Python](https://img.shields.io/badge/Python-3.10+-3776AB?style=for-the-badge&logo=python&logoColor=white)](https://www.python.org/)
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
- Delegated user token auth — every user sees only what their AD permissions allow
- No service accounts, no password storage
- Session persistence with automatic token refresh

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
- Drag-and-drop canvas with resizable chart tiles
- Cross-chart filter bar — one filter slices all charts at once
- Tab support for multi-page dashboards
- Share with your whole team instantly

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
- Connection pool warming at proxy startup — sub-second queries after cold start
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

LoomX is a **monorepo** with three services that work together:

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
│  loomx-api  ·  Express.js  ·  TypeScript                        │
│                                                                  │
│  · JWT validation · Route handlers · Query history logging       │
│  · Semantic SQL generation · Dataset / chart / dashboard CRUD    │
└──────────────────────────────┬──────────────────────────────────┘
                               │  HTTP (localhost:5001)
                               ▼
┌─────────────────────────────────────────────────────────────────┐
│  loomx-python-proxy  ·  Flask  ·  Python 3.10+                  │
│                                                                  │
│  · pyodbc + ODBC Driver 18 for SQL Server                        │
│  · Azure AD token injection via SQL_COPT_SS_ACCESS_TOKEN         │
│  · Connection pool per database · Startup warmup · Heartbeat     │
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

### Why three services?

Node.js cannot authenticate to Fabric SQL using Azure AD tokens natively. The Python proxy bridges this gap using `pyodbc` + `ODBC Driver 18`, the only driver that supports Azure AD interactive token injection (`SQL_COPT_SS_ACCESS_TOKEN`) required by Microsoft Fabric.

### Two Fabric databases

| Database | Purpose | Configured via |
|---|---|---|
| **Metadata DB** | Stores LoomX app data: datasets, charts, dashboards, query history, themes | `.env` |
| **Your Data Sources** | Your actual Fabric warehouses and lakehouses | UI at `/data-sources` |

---

## 📋 Prerequisites

Install **all** of the following before you begin:

| Requirement | Version | Check | Install |
|---|---|---|---|
| **Node.js** | 20.x+ | `node -v` | [nodejs.org](https://nodejs.org/) |
| **pnpm** | 9.x+ | `pnpm -v` | `npm install -g pnpm` |
| **Python** | 3.10+ | `python --version` | [python.org](https://www.python.org/) |
| **ODBC Driver 18** | 18.x | See below | [Microsoft docs](https://learn.microsoft.com/sql/connect/odbc/download-odbc-driver-for-sql-server) |
| **Git** | Any | `git --version` | [git-scm.com](https://git-scm.com/) |
| **Azure Data Studio** | Any | — | [Download](https://azure.microsoft.com/products/data-studio) |
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

</details>

---

## 🚀 Quick Start

```bash
# 1. Clone
git clone <your-repo-url>
cd LoomX

# 2. Install Node.js dependencies (all workspaces in one command)
pnpm install

# 3. Configure environment
cp .env.example .env
# → Edit .env with your Azure AD and Fabric SQL values

# 4. Set up Python proxy
cd apps/loomx-python-proxy
python -m venv venv
venv\Scripts\activate        # Windows
# source venv/bin/activate   # macOS / Linux
pip install -r requirements.txt
cd ../..

# 5. Apply database schema (run schema.sql in Azure Data Studio against your metadata DB)

# 6. Start all services — three terminals
python apps/loomx-python-proxy/proxy.py    # Terminal 1
pnpm --filter loomx-api dev                # Terminal 2
pnpm --filter loomx-web dev                # Terminal 3

# Or start everything at once (Python proxy still needs its own terminal)
pnpm dev
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

This installs dependencies for all three workspaces in one command.

### 3 · Configure Environment Variables

LoomX uses a **single `.env` file at the repository root**. All three services read from it.

```bash
cp .env.example .env
```

```env
# ── Azure AD ─────────────────────────────────────────────────────────────────
AZURE_TENANT_ID=xxxxxxxx-xxxx-xxxx-xxxx-xxxxxxxxxxxx
AZURE_CLIENT_ID=xxxxxxxx-xxxx-xxxx-xxxx-xxxxxxxxxxxx

# ── Metadata Database (optional — setup wizard configures this on first login) ─
# FABRIC_METADATA_ENDPOINT=your-workspace.msit-database.fabric.microsoft.com
# FABRIC_METADATA_DATABASE=YourMetadataDatabase

# ── Ports ────────────────────────────────────────────────────────────────────
API_PORT=8080
WEB_PORT=3000
PYTHON_PROXY_PORT=5001

# ── Internal URLs ─────────────────────────────────────────────────────────────
API_URL=http://localhost:8080
WEB_URL=http://localhost:3000
PYTHON_PROXY_URL=http://localhost:5001

# ── Environment ───────────────────────────────────────────────────────────────
NODE_ENV=development
```

> 💡 **How to find your Fabric SQL endpoint:** Open your Fabric workspace → open your SQL warehouse → **Settings** gear → copy the **Server** value (e.g. `xxxxxxxx.msit-database.fabric.microsoft.com`)

### 4 · Set Up the Python Proxy

```bash
cd apps/loomx-python-proxy

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

> **One-time step.** Once the tables exist, never run this again.

1. Open **Azure Data Studio**
2. Connect to your metadata database (use the `FABRIC_METADATA_ENDPOINT` and `FABRIC_METADATA_DATABASE` values from your `.env`)
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

> The schema uses `IF OBJECT_ID(...) IS NULL` guards — safe to re-run.

### 6 · Start All Services

You need three terminal windows:

**Terminal 1 — Python Proxy**
```bash
cd apps/loomx-python-proxy
venv\Scripts\activate   # Windows  |  source venv/bin/activate  (macOS/Linux)
python proxy.py
```
```
============================================
LOOMX Python Proxy
============================================
Server: http://localhost:5001
Health: http://localhost:5001/health
============================================
[Proxy] Connection pool warmup started at server startup.
```

**Terminal 2 — API**
```bash
pnpm --filter loomx-api dev
```
```
============================================
LoomX API · http://localhost:8080
============================================
```

**Terminal 3 — Web**
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
| Python Proxy | http://localhost:5001/health | `{"status": "healthy"}` |
| API | http://localhost:8080/api/v1/health | `{"status": "ok"}` |
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
│   │   │   ├── lab/                ← SQL Lab (Monaco editor)
│   │   │   └── layout.tsx          ← Root layout with theme + auth
│   │   ├── auth/                   ← MSAL Azure AD configuration
│   │   ├── components/             ← Reusable React components
│   │   ├── contexts/               ← Theme, auth context providers
│   │   └── utils/                  ← MSAL fetch, colour utilities
│   │
│   ├── loomx-api/                  ← Express.js REST API (TypeScript)
│   │   ├── src/
│   │   │   ├── routes/             ← API route handlers (one file per domain)
│   │   │   ├── services/           ← Business logic, SQL generation
│   │   │   ├── middleware/         ← Auth, error handling, user context
│   │   │   └── server.ts           ← Express entry point
│   │   └── schema.sql              ← Run once in your metadata database
│   │
│   └── loomx-python-proxy/         ← Flask ODBC proxy (Python)
│       ├── proxy.py                ← Connection pool + query execution
│       ├── requirements.txt        ← Python dependencies
│       └── start_proxy.bat         ← Windows quick-start helper
│
└── packages/
    └── types/                      ← Shared TypeScript type definitions
```

---

## ⚙️ Environment Variable Reference

| Variable | Required | Default | Description |
|---|---|---|---|
| `AZURE_TENANT_ID` | ✅ | — | Azure AD / Entra ID tenant (directory) ID |
| `AZURE_CLIENT_ID` | ✅ | — | App Registration application (client) ID |
| `FABRIC_METADATA_ENDPOINT` | ⬜ | — | SQL endpoint of your Fabric metadata database (UI-configurable via setup wizard) |
| `FABRIC_METADATA_DATABASE` | ⬜ | — | Database name in that Fabric endpoint (UI-configurable via setup wizard) |
| `API_PORT` | ✅ | `8080` | Port for the Node.js API |
| `WEB_PORT` | ✅ | `3000` | Port for the Next.js web app |
| `PYTHON_PROXY_PORT` | ✅ | `5001` | Port for the Python proxy |
| `API_URL` | ✅ | `http://localhost:8080` | Full URL of the API, used by the web app |
| `WEB_URL` | ✅ | `http://localhost:3000` | Full URL of the web app, used for CORS |
| `PYTHON_PROXY_URL` | ✅ | `http://localhost:5001` | Full URL of the Python proxy |
| `NODE_ENV` | ✅ | `development` | `development` or `production` |
| `PYTHON_PROXY_TIMEOUT_MS` | ⬜ | `120000` | Max ms to wait for a proxy query (2 min default) |

> Data warehouse endpoints are **not** configured here. Register them through the UI at `/data-sources` after first run.

---

## 🛠️ Available Scripts

Run from the **repository root**:

| Command | Description |
|---|---|
| `pnpm dev` | Start all services in development mode |
| `pnpm build` | Build all services for production |
| `pnpm start` | Start all services in production mode |
| `pnpm check-types` | TypeScript type checking across the monorepo |
| `pnpm clean` | Delete all build artifacts |
| `pnpm --filter loomx-api dev` | Start only the API |
| `pnpm --filter loomx-web dev` | Start only the web app |

**Python proxy** (from `apps/loomx-python-proxy/`):

| Command | Description |
|---|---|
| `python proxy.py` | Start proxy (requires venv activated) |
| `start_proxy.bat` | Windows: activates venv and starts proxy in one step |

---

## 🔧 Troubleshooting

<details>
<summary><strong>🔴 ODBC Driver not found / proxy fails to connect</strong></summary>

- Verify ODBC Driver 18 is installed (see [Prerequisites](#-prerequisites))
- Make sure the Python virtual environment is activated before starting the proxy
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
- If you changed `.env`, restart the API service (it does not hot-reload env vars)

</details>

<details>
<summary><strong>🔴 API cannot reach the Python proxy</strong></summary>

- Confirm the proxy is running: `curl http://localhost:5001/health`
- Check `PYTHON_PROXY_URL` in `.env` matches the proxy port
- Check for firewall rules blocking `localhost:5001`

</details>

<details>
<summary><strong>🔴 Cannot connect to metadata database</strong></summary>

- Double-check `FABRIC_METADATA_ENDPOINT` and `FABRIC_METADATA_DATABASE` in `.env`
- Confirm your Azure AD account has `db_datareader` + `db_datawriter` + `db_ddladmin` on the metadata database
- Test the connection directly in Azure Data Studio to rule out network issues

</details>

<details>
<summary><strong>🔴 Queries time out on first run</strong></summary>

Fabric serverless endpoints have a cold start (~10s on first connection). LoomX warms the connection pool at proxy startup to minimise this. If you're still hitting timeouts:

```env
PYTHON_PROXY_TIMEOUT_MS=180000
```

Restart the API after changing `.env`.

</details>

<details>
<summary><strong>🔴 Build errors after pulling new changes</strong></summary>

```bash
# Remove all node_modules and reinstall
rm -rf node_modules apps/*/node_modules packages/*/node_modules
pnpm install

# Clear Next.js cache
rm -rf apps/loomx-web/.next

# Clear API build
rm -rf apps/loomx-api/dist

# Re-check types
pnpm check-types
```

</details>

<details>
<summary><strong>🔴 Port already in use</strong></summary>

**Windows:**
```bash
netstat -ano | findstr :3000
taskkill /PID <PID> /F
```

**macOS / Linux:**
```bash
lsof -ti :3000 | xargs kill -9
```

Or change the port in `.env` and update all three port variables consistently.

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
| [Express.js](https://expressjs.com/) | 4.x | HTTP server and routing |
| [TypeScript](https://www.typescriptlang.org/) | 5.x | Type safety |
| [@azure/identity](https://github.com/Azure/azure-sdk-for-js) | 4.x | Azure AD token validation |
| [axios](https://axios-http.com/) | 1.x | HTTP client for Python proxy |
| [helmet](https://helmetjs.github.io/) | 7.x | HTTP security headers |

### Python Proxy — `loomx-python-proxy`

| Technology | Version | Role |
|---|---|---|
| [Python](https://www.python.org/) | 3.10+ | Runtime |
| [Flask](https://flask.palletsprojects.com/) | 3.0 | HTTP server |
| [pyodbc](https://github.com/mkleehammer/pyodbc) | 5.0 | ODBC driver wrapper |
| [azure-identity](https://pypi.org/project/azure-identity/) | 1.15+ | Azure AD credential provider |
| [python-dotenv](https://pypi.org/project/python-dotenv/) | 1.x | Reads root `.env` file |

### Monorepo Tooling

| Technology | Role |
|---|---|
| [pnpm](https://pnpm.io/) | Fast, disk-efficient package manager with workspace support |
| [Turborepo](https://turbo.build/) | Monorepo task runner with incremental builds |

---

<div align="center">

**Built for Microsoft Fabric. Secured by Azure AD. Owned by you.**

*LoomX — Live Operational Outcomes & Metrics eXperience*

</div>
