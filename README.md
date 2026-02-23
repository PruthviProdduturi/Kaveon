# LoomX

**Live Operational Outcomes & Metrics eXperience**

> A modern, fast, and beautiful data exploration platform for Microsoft Fabric — enterprise-grade performance, Azure AD security, and real-time visualization built on Next.js 15, Express.js, and Python.

---

## Table of Contents

1. [What is LoomX?](#what-is-loomx)
2. [Architecture Overview](#architecture-overview)
3. [Prerequisites](#prerequisites)
4. [Azure AD App Registration](#azure-ad-app-registration)
5. [Step-by-Step Setup](#step-by-step-setup)
   - [1. Clone the Repository](#1-clone-the-repository)
   - [2. Install Node.js Dependencies](#2-install-nodejs-dependencies)
   - [3. Configure Environment Variables](#3-configure-environment-variables)
   - [4. Set Up the Python Proxy](#4-set-up-the-python-proxy)
   - [5. Apply the Database Schema](#5-apply-the-database-schema)
   - [6. Start All Services](#6-start-all-services)
   - [7. Verify Everything is Running](#7-verify-everything-is-running)
6. [Using LoomX](#using-loomx)
7. [Project Structure](#project-structure)
8. [Environment Variable Reference](#environment-variable-reference)
9. [Available Scripts](#available-scripts)
10. [Troubleshooting](#troubleshooting)
11. [Tech Stack](#tech-stack)

---

## What is LoomX?

LoomX is a self-hosted data exploration and visualization platform built specifically for **Microsoft Fabric SQL**. Think of it as your team's own private analytics tool where you can:

- Connect to any number of Fabric warehouses and lakehouses
- Write and save SQL queries in an interactive SQL Lab
- Build charts (bar, line, pie, scatter, table, and 20+ more types)
- Assemble reusable dashboards with drag-and-drop layout
- Share datasets, charts, and dashboards across your team
- Customize your personal color theme

All authentication is handled via **Azure Active Directory** — no separate username/password system.

---

## Architecture Overview

LoomX is a **monorepo** with three services that run together:

```
Browser (port 3000)
    │
    ▼
loomx-web         Next.js 15 frontend — UI, Azure AD login (MSAL)
    │  REST
    ▼
loomx-api         Express.js API — business logic, caching, auth validation
    │  HTTP
    ▼
loomx-python-proxy   Flask proxy — ODBC/pyodbc connection to Fabric SQL
    │  ODBC
    ▼
Microsoft Fabric SQL  (your warehouses and lakehouses)
```

**Why three services?**
Node.js cannot authenticate to Fabric SQL using Azure AD tokens natively. The Python proxy bridges this gap using `pyodbc` + `ODBC Driver 18`, which supports Azure AD interactive token auth out of the box.

**One metadata database** (also Fabric SQL) stores all LoomX application data: datasets, charts, dashboards, saved queries, data sources, user themes, and audit logs. You point LoomX at this database via `.env`. All your actual data warehouses and lakehouses are then registered through the UI at `/data-sources` — no need to touch `.env` for each one.

---

## Prerequisites

Install **all** of the following before you begin. Each one is required.

| Requirement | Minimum Version | How to Check | Download |
|---|---|---|---|
| Node.js | **20.x** | `node -v` | [nodejs.org](https://nodejs.org/) |
| pnpm | **9.x** | `pnpm -v` | `npm install -g pnpm` |
| Python | **3.10+** | `python --version` | [python.org](https://www.python.org/) |
| ODBC Driver 18 | 18.x | See below | [Microsoft docs](https://learn.microsoft.com/sql/connect/odbc/download-odbc-driver-for-sql-server) |
| Git | Any | `git --version` | [git-scm.com](https://git-scm.com/) |
| Azure Data Studio | Any | — | [Azure Data Studio](https://azure.microsoft.com/products/data-studio) |
| Microsoft Fabric SQL access | — | — | Contact your Fabric workspace admin |

### Verify ODBC Driver 18 is installed

**Windows:** Open `ODBC Data Sources (64-bit)` from the Start menu → click the **Drivers** tab → look for `ODBC Driver 18 for SQL Server`.

**macOS/Linux:** Run:
```bash
odbcinst -q -d -n "ODBC Driver 18 for SQL Server"
```

If it is not listed, download it from:
https://learn.microsoft.com/en-us/sql/connect/odbc/download-odbc-driver-for-sql-server

> **Important:** ODBC Driver 18 is the specific version required. Driver 17 and older will not work with Fabric SQL's authentication.

---

## Azure AD App Registration

LoomX uses Azure AD for authentication. You (or your Azure admin) need to create an **App Registration** in Entra ID. This is a one-time setup.

### Steps

1. Go to [portal.azure.com](https://portal.azure.com) → **Microsoft Entra ID** → **App registrations** → **New registration**.

2. Fill in the form:
   - **Name:** `LoomX` (or any name you like)
   - **Supported account types:** Select `Accounts in this organizational directory only`
   - **Redirect URI:** Choose `Single-page application (SPA)` and enter:
     ```
     http://localhost:3000
     ```

3. Click **Register**. Note down:
   - **Application (client) ID** → this is your `AZURE_CLIENT_ID`
   - **Directory (tenant) ID** → this is your `AZURE_TENANT_ID`

4. In your new app registration, go to **Authentication** → confirm the redirect URI `http://localhost:3000` is listed under **Single-page application**.

5. Go to **API permissions** → **Add a permission** → **Azure SQL Database** → **Delegated** → select `user_impersonation` → **Add permissions**.

6. If you are an admin, click **Grant admin consent for [your org]**. If not, ask your Azure admin to do this.

7. Go to **Expose an API** → click **Add a scope** → accept the default Application ID URI → add a scope called `access_as_user`. This allows the frontend to request tokens scoped to the API.

> Your Azure admin will need to ensure that the Fabric SQL endpoint also trusts this app registration. This is normally configured at the Fabric workspace level.

---

## Step-by-Step Setup

Follow these steps **in order**. Do not skip any.

### 1. Clone the Repository

```bash
git clone <your-github-repo-url>
cd LoomX
```

### 2. Install Node.js Dependencies

From the **root** of the repository (not inside any `apps/` folder):

```bash
pnpm install
```

This installs dependencies for all three workspaces (`loomx-api`, `loomx-web`, and shared packages) in one command thanks to pnpm workspaces.

> If `pnpm` is not found, install it first: `npm install -g pnpm`

### 3. Configure Environment Variables

LoomX uses a **single `.env` file at the repository root**. All three services read from it.

**Copy the template:**

```bash
cp .env.example .env
```

**Open `.env` in your editor and fill in your values:**

```env
# ── Server Ports (leave as-is unless something conflicts) ──────────────────
API_PORT=8080
WEB_PORT=3000
PYTHON_PROXY_PORT=5001

# ── Azure AD ────────────────────────────────────────────────────────────────
# Get these from your App Registration in portal.azure.com → Entra ID
AZURE_TENANT_ID=xxxxxxxx-xxxx-xxxx-xxxx-xxxxxxxxxxxx
AZURE_CLIENT_ID=xxxxxxxx-xxxx-xxxx-xxxx-xxxxxxxxxxxx

# ── Metadata Database ────────────────────────────────────────────────────────
# This Fabric SQL database stores LoomX app data (datasets, charts, etc.)
# It is NOT where your actual data lives — that is configured via the UI
FABRIC_METADATA_ENDPOINT=your-workspace.msit-database.fabric.microsoft.com
FABRIC_METADATA_DATABASE=YourMetadataDatabase

# ── Internal Service URLs (do not change unless you changed ports above) ──
API_URL=http://localhost:8080
WEB_URL=http://localhost:3000
PYTHON_PROXY_URL=http://localhost:5001

# ── Environment ──────────────────────────────────────────────────────────────
NODE_ENV=development
```

**How to find your Fabric SQL endpoint:**
1. Open your Microsoft Fabric workspace.
2. Open your SQL warehouse or lakehouse.
3. Click the **Settings** (gear) icon → copy the **SQL connection string** or **Server** value.
4. The endpoint looks like: `xxxxxxxx-xxxx.msit-database.fabric.microsoft.com`

> **Note:** The `.env` file is in `.gitignore` and will never be committed. Your credentials are safe.

### 4. Set Up the Python Proxy

The Python proxy is what actually connects to Fabric SQL. It must be set up separately from the Node.js stack.

#### 4a. Create a Python virtual environment

```bash
cd apps/loomx-python-proxy
python -m venv venv
```

#### 4b. Activate the virtual environment

**Windows (Command Prompt / PowerShell):**
```bash
venv\Scripts\activate
```

**macOS / Linux:**
```bash
source venv/bin/activate
```

Your terminal prompt will change to show `(venv)` when activated.

#### 4c. Install Python dependencies

```bash
pip install -r requirements.txt
```

This installs:
- `flask` — web framework for the proxy API
- `flask-cors` — cross-origin request support
- `pyodbc` — ODBC driver wrapper for Python
- `azure-identity` — Azure AD token acquisition
- `python-dotenv` — reads the root `.env` file

#### 4d. Verify ODBC connectivity (optional but recommended)

With the virtual environment still active, run a quick check:

```python
python -c "import pyodbc; print(pyodbc.drivers())"
```

You should see `ODBC Driver 18 for SQL Server` in the output list. If not, install the driver (see [Prerequisites](#prerequisites)).

#### 4e. Return to the root

```bash
cd ../..
```

### 5. Apply the Database Schema

LoomX needs a set of tables in your Fabric SQL metadata database before it can run. This is a **one-time step** — once the tables exist, you never need to run this again.

1. Open **Azure Data Studio** (or Azure Data Studio in Fabric).
2. Connect to your metadata database using:
   - **Server:** the value of `FABRIC_METADATA_ENDPOINT` from your `.env`
   - **Database:** the value of `FABRIC_METADATA_DATABASE` from your `.env`
   - **Authentication:** Azure Active Directory (your personal account)
3. Open the file: `apps/loomx-api/schema.sql`
4. Click **Run** (or press `F5`).

You should see output like:
```
Table datasets created successfully
Table charts created successfully
Table dashboards created successfully
...
LoomX Production Schema Created Successfully
```

**Tables created:**

| Table | Purpose |
|---|---|
| `datasets` | Semantic layer definitions — dimensions, metrics, filters |
| `charts` | Chart configurations (query + visualization settings) |
| `dashboards` | Dashboard layouts and filter settings |
| `saved_queries` | User-saved SQL queries from SQL Lab |
| `query_history` | Audit log of every executed query |
| `favorites` | Per-user favorites for any object type |
| `activity` | Audit trail of create/update/delete actions |
| `data_sources` | Registered Fabric warehouses and lakehouses |
| `user_themes` | Per-user color theme preferences |

> The schema uses `IF OBJECT_ID(...) IS NULL` guards, so running it multiple times is safe — it will only create tables that do not already exist.

### 6. Start All Services

You need **three terminal windows** open — one for each service.

---

**Terminal 1 — Python Proxy**

```bash
cd apps/loomx-python-proxy
venv\Scripts\activate        # Windows
# source venv/bin/activate   # macOS / Linux
python proxy.py
```

Expected output:
```
 * Running on http://127.0.0.1:5001
 * Debug mode: on
```

---

**Terminal 2 — API (Node.js)**

From the repository root:

```bash
pnpm --filter loomx-api dev
```

Or navigate into the folder:

```bash
cd apps/loomx-api
pnpm dev
```

Expected output:
```
[loomx-api] Server running on http://localhost:8080
[loomx-api] Environment: development
```

---

**Terminal 3 — Web (Next.js)**

From the repository root:

```bash
pnpm --filter loomx-web dev
```

Or navigate into the folder:

```bash
cd apps/loomx-web
pnpm dev
```

Expected output:
```
  ▲ Next.js 15.x.x
  - Local:        http://localhost:3000
  - Ready in Xs
```

---

**Alternative: Start everything from root with one command**

```bash
pnpm dev
```

This uses Turborepo to start all three Node.js services in parallel. Note: the Python proxy must still be started manually in a separate terminal (it is not a Node.js process).

### 7. Verify Everything is Running

Open these URLs in your browser or a tool like curl to confirm each service is healthy:

| Service | URL | Expected Response |
|---|---|---|
| Python Proxy | http://localhost:5001/health | `{"status": "healthy"}` |
| API | http://localhost:8080/api/v1/health | `{"status": "ok"}` |
| Web UI | http://localhost:3000 | LoomX login page |

**Full verification commands:**

```bash
# Check Python proxy
curl http://localhost:5001/health

# Check API
curl http://localhost:8080/api/v1/health

# Check web (just verify it returns HTML)
curl -I http://localhost:3000
```

---

## Using LoomX

Once all services are running:

1. **Open** http://localhost:3000 in your browser.
2. **Sign in** with your Azure AD (Microsoft) account. You will be redirected to Microsoft's login page.
3. **Add a Data Source** — go to `/data-sources` in the sidebar and register your Fabric warehouse or lakehouse. You will need:
   - A display name
   - The SQL endpoint (e.g., `abc123.msit-database.fabric.microsoft.com`)
   - The database name
   - The region (WW or EU)
4. **Explore your data** — use SQL Lab at `/lab` to write and run queries.
5. **Build a dataset** — go to `/datasets` → New Dataset. A dataset defines the semantic layer: which table, which columns are dimensions vs metrics.
6. **Create charts** — go to `/charts` → New Chart. Pick a dataset, configure your query, choose a visualization type.
7. **Build dashboards** — go to `/dashboards` → New Dashboard. Drag charts onto the canvas, resize them, and apply dashboard-level filters.

---

## Project Structure

```
LoomX/
├── .env.example                  ← Copy to .env and fill in your values
├── .env                          ← Your local config (gitignored, never committed)
├── .nvmrc                        ← Node.js version pin (20)
├── pnpm-workspace.yaml           ← pnpm monorepo workspace config
├── turbo.json                    ← Turborepo task pipeline
├── start.ps1                     ← Windows PowerShell all-in-one startup helper
│
├── apps/
│   ├── loomx-api/                ← Express.js REST API (TypeScript)
│   │   ├── src/
│   │   │   ├── adapters/         ← Data format transformation layer
│   │   │   ├── db/               ← Fabric SQL connection via Python proxy
│   │   │   ├── middleware/       ← Auth (JWT), caching, error handling
│   │   │   ├── routes/           ← API route handlers (one file per domain)
│   │   │   ├── services/         ← Business logic, orchestration
│   │   │   └── server.ts         ← Express entry point
│   │   ├── schema.sql            ← Run once in your metadata database
│   │   └── package.json
│   │
│   ├── loomx-web/                ← Next.js 15 frontend (TypeScript, App Router)
│   │   ├── app/                  ← Next.js App Router pages
│   │   │   ├── charts/           ← Chart builder and list
│   │   │   ├── dashboards/       ← Dashboard builder and list
│   │   │   ├── datasets/         ← Dataset configuration
│   │   │   ├── lab/              ← SQL Lab (Monaco editor)
│   │   │   └── layout.tsx        ← Root layout (nav, theme, auth)
│   │   ├── auth/                 ← MSAL Azure AD setup
│   │   ├── components/           ← Reusable React components
│   │   ├── contexts/             ← React context providers (Theme, etc.)
│   │   ├── services/             ← API client functions
│   │   └── utils/                ← Shared utilities
│   │
│   └── loomx-python-proxy/       ← Flask ODBC proxy (Python)
│       ├── proxy.py              ← Connection pool + query execution
│       ├── requirements.txt      ← Python dependencies
│       ├── start_proxy.bat       ← Windows quick-start script
│       └── venv/                 ← Python virtual env (gitignored)
│
└── packages/
    ├── config/                   ← Shared ESLint / TypeScript configs
    └── types/                    ← Shared TypeScript type definitions
        └── src/index.ts
```

---

## Environment Variable Reference

All variables live in a **single `.env` file at the repository root**. Every service reads from this one file.

| Variable | Required | Description |
|---|---|---|
| `AZURE_TENANT_ID` | Yes | Your Azure AD / Entra ID tenant (directory) ID |
| `AZURE_CLIENT_ID` | Yes | Your App Registration application (client) ID |
| `FABRIC_METADATA_ENDPOINT` | Yes | SQL endpoint of your Fabric metadata database |
| `FABRIC_METADATA_DATABASE` | Yes | Database name in that Fabric endpoint |
| `API_PORT` | Yes | Port for the Node.js API (default: `8080`) |
| `WEB_PORT` | Yes | Port for the Next.js web app (default: `3000`) |
| `PYTHON_PROXY_PORT` | Yes | Port for the Python proxy (default: `5001`) |
| `API_URL` | Yes | Full URL of the API, used by the web app (default: `http://localhost:8080`) |
| `WEB_URL` | Yes | Full URL of the web app, used for CORS (default: `http://localhost:3000`) |
| `PYTHON_PROXY_URL` | Yes | Full URL of the Python proxy, used by the API (default: `http://localhost:5001`) |
| `NODE_ENV` | Yes | `development` or `production` |
| `PYTHON_PROXY_TIMEOUT_MS` | No | Max milliseconds to wait for a proxy query (default: `120000` = 2 min) |

> **Data warehouse endpoints are NOT configured here.** After first run, go to `/data-sources` in the UI to register your Fabric warehouses and lakehouses. LoomX stores them in the `data_sources` table and retrieves them dynamically.

---

## Available Scripts

Run these from the **repository root** unless noted otherwise.

| Command | Description |
|---|---|
| `pnpm dev` | Start all services in development mode (auto-reload on save) |
| `pnpm build` | Build all services for production |
| `pnpm start` | Start all services in production mode (requires `pnpm build` first) |
| `pnpm check-types` | Run TypeScript type checking across the whole monorepo |
| `pnpm clean` | Delete all build artifacts (`dist/`, `.next/`) |
| `pnpm --filter loomx-api dev` | Start only the API service |
| `pnpm --filter loomx-web dev` | Start only the web service |

**Python proxy scripts (run from `apps/loomx-python-proxy/`):**

| Command | Description |
|---|---|
| `python proxy.py` | Start the proxy (requires virtual env to be activated) |
| `start_proxy.bat` | Windows shortcut: activates venv and starts proxy |

---

## Troubleshooting

### "ODBC Driver not found" or proxy fails to connect

- Verify ODBC Driver 18 is installed (see [Prerequisites](#prerequisites)).
- Make sure you activated the Python virtual environment before starting the proxy.
- On Windows, try running the terminal **as Administrator** when first setting up the virtual environment.

### Login page redirects to an error or blank screen

- Confirm the redirect URI `http://localhost:3000` is registered in your App Registration under **Authentication → Single-page application**.
- Clear browser cookies and localStorage for `localhost:3000`, then try again.
- Make sure `AZURE_CLIENT_ID` and `AZURE_TENANT_ID` in `.env` match your App Registration exactly.

### API returns 401 Unauthorized

- Your Azure AD token may have expired. Sign out from the web app and sign back in.
- If you changed `.env`, restart the API service — it does not hot-reload environment variables.

### API cannot reach the Python proxy

- Make sure the Python proxy is running: visit http://localhost:5001/health.
- Check `PYTHON_PROXY_URL` in `.env` matches the port the proxy is listening on (default: `5001`).
- If `PYTHON_PROXY_URL` is correct but it still fails, check for firewall rules blocking `localhost:5001`.

### "Cannot connect to metadata database"

- Double-check `FABRIC_METADATA_ENDPOINT` and `FABRIC_METADATA_DATABASE` in `.env`.
- Confirm your Azure AD account has at least `db_datareader` + `db_datawriter` access to the metadata database.
- Try connecting to the metadata database directly in Azure Data Studio to rule out network issues.

### Schema tables already exist / need to update

- The schema script uses `IF OBJECT_ID(...) IS NULL` guards — you can safely re-run `schema.sql` without losing data.
- If a table is missing a new column after a repo update, check `schema.sql` for any `ALTER TABLE` statements at the bottom.

### Queries time out on first run

Fabric Delta Lake tables can be slow on cold start (first query after idle). This is normal.

- Increase the timeout in `.env`:
  ```env
  PYTHON_PROXY_TIMEOUT_MS=180000
  ```
- Restart the API after changing `.env`.

### Build errors after pulling new changes

```bash
# Remove all node_modules and reinstall
rm -rf node_modules apps/*/node_modules packages/*/node_modules
pnpm install

# Remove Next.js cache
rm -rf apps/loomx-web/.next

# Remove API build artifacts
rm -rf apps/loomx-api/dist

# Re-run type checks
pnpm check-types
```

### Port already in use

If `localhost:3000`, `localhost:8080`, or `localhost:5001` is already in use:

**Find and kill the process (Windows):**
```bash
netstat -ano | findstr :3000
taskkill /PID <PID> /F
```

**macOS / Linux:**
```bash
lsof -ti :3000 | xargs kill -9
```

Or change the port in `.env` and update all three port variables consistently.

---

## Tech Stack

### Frontend — `loomx-web`
| Technology | Version | Purpose |
|---|---|---|
| Next.js | 15.x | React framework, App Router, SSR |
| React | 19.x | UI component library |
| TypeScript | 5.x | End-to-end type safety |
| @azure/msal-browser | 5.x | Azure AD authentication in browser |
| ECharts | 5.x | Data visualization (20+ chart types) |
| Monaco Editor | 0.52.x | VS Code-grade SQL editor |
| react-grid-layout | 2.x | Drag-and-drop dashboard builder |
| react-colorful | 5.x | Color picker for user themes |

### Backend — `loomx-api`
| Technology | Version | Purpose |
|---|---|---|
| Express.js | 4.x | HTTP server and routing |
| TypeScript | 5.x | Type safety |
| @azure/identity | 4.x | Azure AD token validation |
| tedious | 18.x | Native SQL Server driver (fallback path) |
| node-cache | 5.x | In-memory multi-tier caching |
| helmet | 7.x | HTTP security headers |
| axios | 1.x | HTTP client for Python proxy communication |

### Python Proxy — `loomx-python-proxy`
| Technology | Version | Purpose |
|---|---|---|
| Python | 3.10+ | Runtime |
| Flask | 3.0 | HTTP server |
| pyodbc | 5.0 | ODBC driver wrapper |
| azure-identity | 1.15 | Azure AD credential provider |
| python-dotenv | 1.x | Reads root `.env` file |

### Monorepo Tooling
| Technology | Purpose |
|---|---|
| pnpm | Fast, disk-efficient package manager |
| Turborepo | Monorepo task runner with caching |

---

## License

Proprietary — Internal use only.

---

*Built with modern web technologies and a relentless focus on developer experience.*
