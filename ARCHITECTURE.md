# LoomX Architecture

**Live Operational Outcomes & Metrics eXperience**

> Comprehensive technical architecture documentation for the LoomX platform

---

## Table of Contents

1. [System Overview](#system-overview)
2. [Configuration Architecture](#configuration-architecture)
3. [Architecture Layers](#architecture-layers)
4. [Data Flow](#data-flow)
5. [Authentication](#authentication)
6. [Middleware](#middleware)
7. [Database Schema](#database-schema)
8. [Feature Architecture](#feature-architecture)
   - [Datasets: The Semantic Layer](#1-datasets-the-semantic-layer)
   - [Charts: Visualization Engine](#2-charts-visualization-engine)
   - [Dashboards: Multi-Chart Composition](#3-dashboards-multi-chart-composition)
   - [SQL Lab: Direct Query Interface](#4-sql-lab-direct-query-interface)
   - [How Everything Connects](#how-everything-connects)
9. [Caching Strategy](#caching-strategy)
10. [Component Architecture](#component-architecture)
11. [API Design](#api-design)
12. [Frontend Architecture](#frontend-architecture)
13. [Deployment](#deployment)

---

## System Overview

LoomX is a modern data exploration platform built with a clean, two-tier service architecture. The system consists of two main services:

```
┌─────────────────────────────────────────────────────────────┐
│                         Browser                              │
│  ┌──────────────────────────────────────────────────────┐   │
│  │           Next.js 15 Frontend (Port 3000)            │   │
│  │   - React 19 with App Router                         │   │
│  │   - MSAL Authentication                              │   │
│  │   - ECharts Visualization                            │   │
│  │   - Monaco SQL Editor                                │   │
│  └──────────────────────────────────────────────────────┘   │
└─────────────────────────────────────────────────────────────┘
                            ↓ HTTPS/REST (Bearer token)
┌─────────────────────────────────────────────────────────────┐
│            FastAPI Backend (Port 8080)                       │
│  ┌──────────────────────────────────────────────────────┐   │
│  │  Routers → Services → Database Layer                 │   │
│  │  - JWT RS256 signature verification (PyJWT + JWKS)   │   │
│  │  - Semantic SQL generation (star-schema aware)       │   │
│  │  - Dataset / chart / dashboard CRUD                  │   │
│  │  - pyodbc connection pool (in-process, per database) │   │
│  │  - Azure AD token injection (SQL_COPT_SS_ACCESS_TOKEN│   │
│  └──────────────────────────────────────────────────────┘   │
└─────────────────────────────────────────────────────────────┘
                            ↓ ODBC/TDS (TLS 1.2+)
┌─────────────────────────────────────────────────────────────┐
│                Microsoft Fabric SQL Endpoints                │
│  ┌──────────────────────────────────────────────────────┐   │
│  │  Metadata Database (LoomX tables)                    │   │
│  │  - stores: datasets, charts, dashboards, etc.        │   │
│  │  - stores: data_sources table (warehouse configs)    │   │
│  └──────────────────────────────────────────────────────┘   │
│  ┌──────────────────────────────────────────────────────┐   │
│  │  Data Warehouses (User Data — Dynamic)               │   │
│  │  - Configured via UI at /data-sources                │   │
│  │  - Connection info retrieved from data_sources table │   │
│  │  - API creates ODBC connections dynamically per DB   │   │
│  └──────────────────────────────────────────────────────┘   │
└─────────────────────────────────────────────────────────────┘
```

### Why Python/FastAPI?

Python is the only runtime with a mature ODBC driver (`pyodbc`) that supports Azure AD interactive token injection via `SQL_COPT_SS_ACCESS_TOKEN` — the mechanism required by Microsoft Fabric SQL. The former architecture used a Node.js/Express API proxying HTTP calls to a separate Python/Flask sidecar. Consolidating both tiers into a single Python/FastAPI process eliminates the inter-service HTTP hop, halves the container count, and simplifies the deployment surface.

---

## Configuration Architecture

### Single Root `.env` File Pattern

LoomX uses a **centralized configuration approach** with a single `.env` file at the repository root. Both services load their configuration from this shared file.

```
LoomX/
├── .env                           ← Single configuration file
├── apps/
│   ├── loomx-api/
│   │   └── config.py              → Loads ../../.env via python-dotenv
│   └── loomx-web/
│       └── next.config.ts         → Reads NEXT_PUBLIC_* env vars at build time
```

### Environment Variable Ownership

| Variable | Read by | Purpose |
|---|---|---|
| `AZURE_CLIENT_ID` | API | JWT audience verification (RS256) |
| `AZURE_TENANT_ID` | API | JWKS endpoint construction |
| `AZURE_CLIENT_ID` | Web | MSAL browser sign-in |
| `AZURE_TENANT_ID` | Web | MSAL browser sign-in |
| `FABRIC_METADATA_ENDPOINT` | API | ODBC hostname for metadata DB |
| `FABRIC_METADATA_DATABASE` | API | Database name for metadata DB |
| `WEB_URL` | API | CORS allowed origin |
| `API_URL` | Web | Base URL for all API calls |
| `API_PORT` | API | Uvicorn/Gunicorn listen port |

---

## Architecture Layers

### Backend Layers

```
HTTP Request
     │
     ▼
┌─────────────────────────────────────────────────────────┐
│  FastAPI Middleware Stack (main.py)                      │
│  1. security_headers — adds X-Frame-Options, HSTS, etc.  │
│  2. log_requests — logs method, path, status, duration   │
│  3. CORSMiddleware — allow_origins=[WEB_URL] only        │
└─────────────────────────────────────────────────────────┘
     │
     ▼
┌─────────────────────────────────────────────────────────┐
│  Auth Dependency (middleware/auth.py)                    │
│  get_current_user() → require_auth() Depends injection  │
│  - Extracts Bearer token from Authorization header       │
│  - Verifies RS256 signature via JWKS (PyJWKClient)       │
│  - Extracts email from preferred_username / email / upn  │
│  - Returns 401 if token missing/invalid/expired          │
└─────────────────────────────────────────────────────────┘
     │
     ▼
┌─────────────────────────────────────────────────────────┐
│  Routers (routers/*.py)                                  │
│  12 route modules, all mounted under /api or /api/v1     │
│  - auth, health, setup                                   │
│  - datasets, charts, dashboards                          │
│  - data_sources, favorites, theme                        │
│  - lab, sql, metadata_summary                            │
└─────────────────────────────────────────────────────────┘
     │
     ▼
┌─────────────────────────────────────────────────────────┐
│  Services (services/*.py)                                │
│  Business logic layer — no direct HTTP awareness         │
│  - datasets, charts, dashboards, favorites               │
│  - query_generator (star-schema SQL builder)             │
│  - saved_queries, query_history, theme                   │
│  - sql_table_extractor                                   │
└─────────────────────────────────────────────────────────┘
     │
     ▼
┌─────────────────────────────────────────────────────────┐
│  Database Layer (database/*.py)                          │
│  - pool.py: pyodbc connection pool, one per database     │
│    FabricSQLConnection → ConnectionPool → global registry│
│    Token auth via SQL_COPT_SS_ACCESS_TOKEN               │
│    Retry-once on stale connections (08S01 / 10054)       │
│  - metadata.py: parameterized @paramN query helpers      │
│  - warmup.py: startup warmup + 5-min heartbeat thread    │
└─────────────────────────────────────────────────────────┘
     │
     ▼
Microsoft Fabric SQL (ODBC / TDS)
```

### Frontend Layers

```
Browser Request
     │
     ▼
┌─────────────────────────────────────────────────────────┐
│  Next.js App Router (app/layout.tsx)                     │
│  - MsalProvider — MSAL context for all pages             │
│  - ThemeProvider — per-user colour theme context         │
│  - Navbar — authenticated user display, nav links        │
└─────────────────────────────────────────────────────────┘
     │
     ▼
┌─────────────────────────────────────────────────────────┐
│  Page Components (app/**/ page.tsx)                      │
│  - charts/, dashboards/, datasets/, data-sources/        │
│  - lab/ (Monaco SQL editor)                              │
│  - favorites/, workspace-activity/                       │
└─────────────────────────────────────────────────────────┘
     │
     ▼
┌─────────────────────────────────────────────────────────┐
│  Services Layer (services/*.ts)                          │
│  API client layer — wraps msalFetch with base URL        │
│  All calls include Authorization: Bearer <AAD token>     │
└─────────────────────────────────────────────────────────┘
     │
     ▼
LoomX FastAPI (Bearer token authenticated)
```

---

## Data Flow

### Query Execution Flow (SQL Lab)

```
1. User writes SQL in Monaco editor
2. Frontend calls POST /api/v1/lab/query with {query, database}
3. require_auth extracts and verifies the Bearer JWT
4. lab.py router calls pool.execute_query(sql, database)
5. pool.get_connection_pool(database) looks up the pool:
   - If first time: resolves endpoint from data_sources table
   - Creates ConnectionPool if not exists
6. FabricSQLConnection.connect() if not already connected:
   - DefaultAzureCredential.get_token("https://database.windows.net/.default")
   - Injects token via SQL_COPT_SS_ACCESS_TOKEN attribute
7. cursor.execute(sql, params) → fetchall()
8. Results serialized: datetime → ISO string, large int → string
9. history_svc.create_history() logs the execution
10. Response returned with columns, rows, rowCount, executionTime
```

### Chart Render Flow

```
1. Dashboard/chart page loads
2. Frontend calls POST /api/v1/sql/execute with {sql_text, database, source, ...}
3. require_auth validates Bearer JWT
4. sql.py calls pool.execute_query(sql_text, database)
5. Results returned as {columns, rows}
6. Frontend renders results via ECharts
7. Execution logged to query_history with trigger_source, dataset_id
```

### Semantic SQL Generation Flow

```
1. Chart builder calls POST /api/v1/sql/generate
2. sql.py loads dataset from datasets_svc.get_dataset_by_id()
3. Dataset includes: fact table, dimensions, columns, metrics
4. build_chart_preview_query(params) in query_generator.py:
   a. Builds SELECT clause: metrics with aggregation functions
   b. Builds GROUP BY: dimension display columns
   c. Builds JOINs: fact table LEFT JOIN each dimension table
      - COALESCE for role-playing dimensions (shared fact key)
   d. Builds WHERE: time range + active filters
   e. Builds ORDER BY: sorted metric or dimension
   f. Wraps in SELECT TOP <limit>
5. Generated SQL returned to frontend
6. Frontend calls /api/v1/sql/execute to run it
```

---

## Authentication

### Token Verification Flow

```
Browser (MSAL)
    │
    │  GET/POST /api/v1/...
    │  Authorization: Bearer eyJ...  (Azure AD access token)
    ▼
FastAPI — get_current_user() (middleware/auth.py)
    │
    ├─ When AZURE_CLIENT_ID + AZURE_TENANT_ID are set (production):
    │   │
    │   ├─ PyJWKClient.get_signing_key_from_jwt(token)
    │   │    → Fetches JWKS from:
    │   │      https://login.microsoftonline.com/{tenant}/discovery/v2.0/keys
    │   │    → Keys are cached in memory
    │   │
    │   └─ jwt.decode(token, key, algorithms=["RS256"],
    │                 audience=AZURE_CLIENT_ID,
    │                 options={"verify_exp": True})
    │         → Verifies: signature, expiry, audience
    │         → Extracts: preferred_username / email / upn
    │
    └─ When AAD not configured (first-run setup mode only):
          → Unverified JWT decode (no signature check)
          → x-user-email header accepted as fallback
          → This path is only reachable before metadata DB is configured

result: user email string or None
    │
    ▼
require_auth(user) dependency
    → Returns user email if present
    → Raises HTTP 401 if None
```

### Managed Identity — Fabric SQL Access

```
FastAPI container (Azure Container Apps)
    │
    ├─ DefaultAzureCredential.get_token(
    │      "https://database.windows.net/.default"
    │  )
    │  → Automatically uses User-Assigned Managed Identity
    │  → Token cached and refreshed automatically
    │
    ▼
pyodbc.connect(
    conn_str,
    attrs_before={SQL_COPT_SS_ACCESS_TOKEN: token_struct}
)
    │
    ▼
Microsoft Fabric SQL (no SQL username/password ever used)
```

---

## Middleware

### Security Headers (main.py)

Applied to every response via `@app.middleware("http")`:

| Header | Value | Protection |
|---|---|---|
| `X-Content-Type-Options` | `nosniff` | Prevents MIME-type sniffing |
| `X-Frame-Options` | `DENY` | Prevents clickjacking |
| `Strict-Transport-Security` | `max-age=31536000; includeSubDomains` | Forces HTTPS |
| `Referrer-Policy` | `strict-origin-when-cross-origin` | Controls referrer exposure |
| `X-Permitted-Cross-Domain-Policies` | `none` | Blocks Flash/PDF cross-domain |

### CORS (main.py)

```python
CORSMiddleware(
    allow_origins=[settings.WEB_URL],        # exact origin only, no wildcard
    allow_credentials=True,
    allow_methods=["GET", "POST", "PUT", "PATCH", "DELETE", "OPTIONS"],
    allow_headers=["Authorization", "Content-Type", "x-user-email", "Cache-Control"],
)
```

### Rate Limiting (middleware/rate_limit.py)

`RateLimitMiddleware` enforces a per-user sliding-window limit using an in-memory counter keyed on the authenticated user's email. Requests that exceed the limit receive HTTP 429. The limit applies only to authenticated routes — unauthenticated setup/health endpoints are excluded.

### Error Handling (middleware/errors.py)

- `AppError` / subclasses (`ValidationError`, `NotFoundError`, etc.) → structured JSON error with appropriate status code
- All unhandled exceptions → HTTP 500 with generic message; full traceback printed to server logs; **no** exception details returned to clients

---

## Database Schema

The metadata database stores all LoomX application state. Schema is in `apps/loomx-api/schema.sql`.

| Table | Key Columns | Purpose |
|---|---|---|
| `datasets` | `id`, `name`, `table_name`, `schema_name`, `database_name`, `created_by` | Semantic layer definitions |
| `dataset_dimensions` | `dataset_id`, `dimension_table`, `fact_key`, `join_key` | Star-schema JOIN relationships |
| `dataset_columns` | `dataset_id`, `column_name`, `display_name`, `data_type`, `role` | Column metadata (dimensions, metrics, filters) |
| `charts` | `id`, `name`, `dataset_id`, `chart_type`, `config_json`, `created_by` | Chart configurations |
| `dashboards` | `id`, `name`, `layout_json`, `filter_config`, `created_by` | Dashboard layouts |
| `saved_queries` | `id`, `name`, `sql_text`, `database`, `user_id` | User-saved SQL queries |
| `query_history` | `id`, `sql_text`, `duration_ms`, `status`, `trigger_source`, `executed_by` | Full execution audit log |
| `favorites` | `id`, `user_email`, `object_type`, `object_id`, `object_name` | Per-user favourites |
| `data_sources` | `id`, `name`, `type`, `connection_string`, `database_name`, `region`, `is_active` | Registered Fabric endpoints |
| `user_themes` | `id`, `user_email`, `primary_color`, `theme_config` | Per-user colour themes |

> `connection_string` in `data_sources` is never returned in API responses — it is excluded from all SELECT queries via the `_PUBLIC_FIELDS` constant in `routers/data_sources.py`.

---

## Feature Architecture

### 1. Datasets: The Semantic Layer

Datasets are the core abstraction. They define:
- **Fact table**: the primary table (e.g., `dbo.SalesOrders`)
- **Dimensions**: related lookup tables with JOIN configuration (`dimension_table`, `fact_key`, `join_key`)
- **Columns**: all queryable columns with roles (`dimension`, `metric`, `filter`)
- **Metrics**: aggregation definitions (SUM, AVG, COUNT, etc.)

#### Role-Playing Dimensions

A "role-playing dimension" is when two different foreign keys in the fact table both reference the same dimension table (e.g., `order_date_key` and `ship_date_key` both reference `dim_date`). LoomX handles this via `COALESCE` in JOIN conditions:

```sql
LEFT JOIN [dim_date] AS [dim_date_1]
  ON COALESCE([fact].[order_date_key], [fact].[ship_date_key]) = [dim_date_1].[date_key]
```

#### Dimension Expansion

`services/datasets.py:_expand_columns_from_dimensions()` creates column entries for each dimension registered to the dataset, linking dimension table columns back through the JOIN path. This allows the chart builder to know which dimension table to JOIN for any selected column.

---

### 2. Charts: Visualization Engine

#### Query Generation (`services/query_generator.py`)

`build_chart_preview_query(params)` accepts:
- `datasource`: fact table (schema.table)
- `dimensions`: list of dimension JOIN specs
- `columns`: dataset column metadata
- `xAxis`, `yAxis`: selected axis columns
- `filters`: active filter values
- `timeRange`, `timeColumn`: optional date range
- `limit`: row cap (default 500)

Output: a complete T-SQL SELECT statement with:
- `SELECT TOP <limit>` with `[bracket-quoted]` identifiers
- Star-schema LEFT JOINs per dimension
- Date formatting for Month/Quarter/Year/Week time columns
- WHERE clause from time range + filter values
- GROUP BY the x-axis column
- ORDER BY the metric column

#### Filter Value Generation (`build_distinct_filter_values_query`)

Three-tier optimization:
- **Tier 1** — Fact key column → no JOIN needed, query the fact table directly
- **Tier 2** — Dimension key column → JOIN dimension, filter on key
- **Tier 3** — Dimension display column → JOIN dimension, filter on display value

Returns `{sql, keyColumn, filteringTier}`. The frontend always sends the `keyColumn` value as the filter parameter, ensuring consistent filter behavior regardless of display values.

---

### 3. Dashboards: Multi-Chart Composition

#### Layout Model

Dashboards store a **flat layout array** (serialised as `layout_json`) where each item is a `DashboardLayoutItem`:

```typescript
interface DashboardLayoutItem {
  i: string;          // unique id
  type: ComponentType; // 'chart' | 'text' | 'header' | 'divider' | 'row' | 'column' | 'tabs'
  x, y, w, h: number; // react-grid-layout grid position/size
  parentId?: string;   // set when item is nested inside a row/column
  children?: DashboardLayoutItem[]; // for row/column containers
  chartId?: number;
  textConfig?: { content, alignment, fontSize, color };
  headerConfig?: { content, size, alignment, color };
  dividerConfig?: { style, color, thickness };
}
```

Root items (no `parentId`) sit on the main react-grid-layout canvas. Container items (`row`, `column`) own their children via the `children` array — children are sized and positioned by the container, not by react-grid-layout.

#### State Management — `DashboardContext`

All dashboard state lives in `DashboardContext` (React context + `useState`):

| State / method | Purpose |
|---|---|
| `layout` / `setLayout` | The flat layout array; drives react-grid-layout |
| `addLayoutItem` | Appends a new root item |
| `updateLayoutItem` | Patches any item by id (config changes, resize) |
| `removeLayoutItem` | Removes a root item |
| `addItemToContainer` | Appends a child to a row/column's `children` array |
| `removeItemFromContainer` | Removes a child from a container |
| `moveItemToRoot` | Lifts a nested item out to the root canvas |
| `duplicateLayoutItem` | Deep-clones an item with fresh ids |
| `crossFilters` | Map of `itemId → {column, value}` for cross-filtering |
| `setCrossFilter / clearCrossFilter` | Update/remove a cross-filter entry |
| `getCrossFilterFilters` | Returns filters from other charts applicable to a given chart |
| `dashboardFilters` | Dashboard-level shared filters (filter bar) |
| `getEffectiveFilters` | Merges dashboard filters + cross-filters for a chart |
| `preloadAllCharts` | Fetches all chart configs in parallel and caches them |
| `getChartConfig` | Returns a cached chart config from the preload cache |

#### Component Types and Rendering

`DashboardItem` is the type router — it receives a `DashboardLayoutItem` and delegates rendering:

```
DashboardItem
 ├── 'chart'   → DashboardChartComponent (chart card with ⋯ menu)
 ├── 'text'    → DashboardTextComponent  (self-managed, no card wrapper)
 ├── 'header'  → DashboardHeaderComponent (self-managed, no card wrapper)
 ├── 'divider' → DashboardDividerComponent (self-managed, no card wrapper)
 ├── 'row'     → DashboardRowComponent    (self-managed container)
 ├── 'column'  → DashboardColumnComponent (self-managed container)
 └── 'tabs'    → DashboardTabsComponent   (tabbed container)
```

**Self-managed components** (`text`, `header`, `divider`, `row`, `column`) bypass the default card wrapper — they render borderless, Superset-style, and handle their own hover toolbars, inline editing, and confirm dialogs. They receive `onRemove` as a **direct removal function** (not a re-open-confirm wrapper) because they own their own confirm modals.

**Chart cards** (`chart` type) get a card wrapper (border, shadow) and are rendered via:

```
DashboardChartComponent
  └── DashboardChartLoader
        └── ChartBuilderProvider       (scoped context for this chart)
              ├── ChartHydrator        (populates context from config — renders null)
              └── <div flex:1>
                    ├── ChartPreview   (ECharts / table / KPI / map renderer)
                    └── ChartActionsOverlay  (⋯ button + dropdown menu)
```

`ChartBuilderProvider` is scoped **per chart card** so `ChartActionsOverlay` can access `useChartBuilder()` for the "View Query" and "View as Table" features.

#### Chart Actions Overlay (`ChartActionsOverlay`)

The always-visible `⋯` button on each chart card opens a dropdown with:

| Action | Notes |
|---|---|
| Refresh | Increments `refreshKey` → forces `ChartHydrator` remount |
| Full screen | Portal modal (ReactDOM.createPortal) over the page |
| Edit chart | Navigate to `/charts/:id` (edit mode only) |
| View query | Shows last executed SQL in a portal modal |
| View as table | Shows raw data rows/columns in a portal modal |
| Download CSV | Calls `exportsRef.current.downloadCsv()` registered by `ChartPreview` |
| Download PNG | Calls `exportsRef.current.downloadPng()` registered by `ChartPreview` |
| Share dashboard | Copies dashboard URL to clipboard |
| Duplicate | Calls `duplicateLayoutItem` (edit mode only) |
| Remove | Portal confirm modal → `directRemove` (edit mode only) |

`ChartPreview` registers its download functions via the `onRegisterExports` callback pattern:
```typescript
// ChartPreview calls this once, passing download functions to the parent
onRegisterExports?.({ downloadPng, downloadCsv })
```

#### Cross-Filter System

Clicking a chart element (bar, pie slice, etc.) in view mode emits a cross-filter:

```
User clicks chart element
  → ChartPreview.onCrossFilter(value)
  → DashboardChartComponent.handleCrossFilter(column, value)
  → DashboardContext.setCrossFilter(itemId, column, value)
  → All other charts re-render with the new filter injected into their WHERE clause
```

Each chart uses `getCrossFilterFilters(itemId)` to get filters emitted by **other** charts. Only filters whose column matches this chart's time column or groupby columns are applied — preventing nonsensical filter application across unrelated charts.

#### Parallel Chart Preloading

On dashboard open, `preloadAllCharts(apiBase, fetchFn)` fetches all chart configs in parallel:

```typescript
await Promise.all(chartIds.map(id => fetch(`/api/v1/charts/${id}`)))
// Results stored in chartConfigCache: Map<number, ChartConfig>
```

Each `DashboardChartLoader` checks `getChartConfig(chartId)` first — if cached, no network call is made. This means all charts render immediately from cache without any sequential waterfall.

---

### 4. SQL Lab: Direct Query Interface

SQL Lab is a developer-focused interface:
- Monaco Editor with SQL syntax highlighting and auto-complete
- Database selector (populated from `data_sources` table)
- Results grid with column sorting, search, and CSV export
- Save queries → stored in `saved_queries` table
- Query history → every execution logged with duration, row count, status, and trigger source

All Lab endpoints enforce `require_auth`. SQL size is capped at 64 KB.

---

### How Everything Connects

```
data_sources table
    │
    ├─ provides endpoint + database_name
    │
    └─► ConnectionPool (database/pool.py)
            │
            └─► datasets (query against user's warehouse)
                    │
                    ├─► query_generator.py → SQL text
                    │
                    └─► /sql/execute → results → ECharts
```

---

## Caching Strategy

LoomX intentionally avoids server-side caching of query results — all data is fetched live from Fabric SQL on every request. This ensures users always see the most current data.

What IS cached:
- **JWKS public keys**: `PyJWKClient(cache_keys=True)` — avoids repeated HTTPS calls to Azure AD
- **Azure AD tokens**: `DefaultAzureCredential` caches tokens internally with automatic refresh
- **Connection pool**: each `(endpoint, database)` pair has a persistent pool of up to N reusable ODBC connections — avoids per-request TLS handshakes and auth overhead

All API responses that return mutable data include:
```
Cache-Control: no-cache, no-store, must-revalidate
Pragma: no-cache
Expires: 0
```

---

## Component Architecture

### Key Backend Components

| Component | File | Responsibility |
|---|---|---|
| `ConnectionPool` | `database/pool.py` | Thread-safe queue-based ODBC pool per database |
| `FabricSQLConnection` | `database/pool.py` | Single pyodbc connection with Azure AD token auth |
| `LargeIntResponse` | `database/pool.py` | FastAPI response class: `datetime→ISO`, large `int→str` |
| `get_current_user` | `middleware/auth.py` | JWT verification dependency |
| `require_auth` | `middleware/auth.py` | Enforcing dependency — 401 if unauthenticated |
| `RateLimitMiddleware` | `middleware/rate_limit.py` | Per-user in-memory sliding-window rate limiter |
| `build_chart_preview_query` | `services/query_generator.py` | Star-schema T-SQL generation |
| `build_distinct_filter_values_query` | `services/query_generator.py` | Filter dropdown query generation |
| `quote_identifier` | `services/query_generator.py` | SQL identifier sanitization |
| `start_warmup_and_heartbeat` | `database/warmup.py` | Background warmup + 5-min keepalive |

### Key Frontend Components

| Component | File | Responsibility |
|---|---|---|
| `MsalProvider` | `auth/` | Azure AD MSAL browser context |
| `msalFetch` | `utils/` | Authenticated fetch wrapper (injects Bearer token) |
| `ThemeContext` | `contexts/` | Per-user colour theme state |
| `ConfirmModal` | `components/ConfirmModal.tsx` | Portal-based confirm dialog — renders at `document.body` via `ReactDOM.createPortal` to avoid clipping |
| Chart Builder | `app/charts/new/` | Drag-drop chart config UI |
| `ChartPreview` | `components/charts/ChartPreview.tsx` | Renders ECharts, table, KPI, world map; exposes `onRegisterExports` for download callbacks |
| `ChartBuilderProvider` | `components/charts/ChartBuilderContext.tsx` | Per-chart scoped context — runs query, holds results, exposes `sqlPreview` |
| Dashboard Canvas | `components/dashboards/DashboardCanvas.tsx` | react-grid-layout grid with row drag handles (small draggable strip + `dataTransfer`) |
| `DashboardContext` | `components/dashboards/DashboardContext.tsx` | Flat layout state, cross-filter map, preload cache, filter merging |
| `DashboardItem` | `components/dashboards/DashboardItem.tsx` | Routes each layout item to the correct component; chart items get card wrapper |
| `ChartActionsOverlay` | `components/dashboards/components/ChartActionsOverlay.tsx` | Always-visible ⋯ button + dropdown; uses `useChartBuilder()` for query/table modals |
| `DashboardChartComponent` | `components/dashboards/components/DashboardChartComponent.tsx` | Scopes `ChartBuilderProvider`, wires preload cache, cross-filter emission, refresh key |
| `DashboardTextComponent` | `components/dashboards/components/DashboardTextComponent.tsx` | Markdown renderer + formatting toolbar (B/I/code/link/lists/quote); inline edit with Markdown/Preview tabs |
| `DashboardHeaderComponent` | `components/dashboards/components/DashboardHeaderComponent.tsx` | H1/H2/H3 inline-editable header with alignment and `react-colorful` colour picker |
| `DashboardColumnComponent` | `components/dashboards/components/DashboardColumnComponent.tsx` | Vertical container with child drag-to-reorder and add-block dropdown (Chart/Text/Header/Divider) |
| Monaco Editor | `app/lab/` | SQL editor with syntax highlighting |

---

## API Design

### URL Structure

```
/api/health                      — health check (unauthenticated)
/api/connect                     — connect (noop, for MSAL compat)
/api/v1/setup/status             — setup wizard status (unauthenticated)
/api/v1/setup/test               — test connection (setup mode only)
/api/v1/setup/initialize         — apply schema.sql (setup mode only)
/api/v1/datasets                 — datasets CRUD
/api/v1/charts                   — charts CRUD
/api/v1/dashboards               — dashboards CRUD
/api/v1/data-sources             — data sources CRUD
/api/v1/favorites                — user favourites
/api/v1/theme                    — user colour theme
/api/v1/lab/*                    — SQL Lab (queries, history, tables, execute)
/api/v1/sql/generate             — semantic SQL generation
/api/v1/sql/distinct-filter-values — filter dropdown values
/api/v1/sql/execute              — SQL execution with history logging
/api/v1/metadata/summary         — parallel metadata summary
```

### Response Conventions

- All success responses: `{"success": true, ...data}` or direct object
- All error responses: `{"error": {"code": "...", "message": "...", "details": ...}}`
- No-cache headers on all mutable GET endpoints
- `connection_string` never included in any data-source response

### Parameterized Query Convention

All metadata DB queries use `@param0`, `@param1`, ... placeholders which are replaced with `?` by `database/metadata.py` before execution:

```python
db.query("SELECT * FROM datasets WHERE created_by = @param0", [user_email])
```

Direct pool queries use native `?` placeholders:

```python
conn.execute_query("SELECT * FROM table WHERE id = ?", [record_id])
```

---

## Frontend Architecture

### Next.js App Router

LoomX uses the **Next.js 15 App Router** with all interactive pages rendered client-side (no SSR data fetching — all data comes from the authenticated API).

```
app/
├── layout.tsx              — Root: MsalProvider + ThemeProvider + Navbar
├── page.tsx                — Home: recent activity, quick links
├── charts/
│   ├── page.tsx            — Charts list
│   ├── [id]/page.tsx       — Chart detail / inline rename
│   └── new/page.tsx        — Chart builder (drag-drop config + live preview)
├── dashboards/
│   ├── page.tsx            — Dashboards list
│   ├── [id]/
│   │   ├── edit/page.tsx   — Dashboard editor (react-grid-layout, edit mode)
│   │   └── view/page.tsx   — Dashboard viewer (read-only, filter bar, publish)
├── datasets/
│   ├── page.tsx            — Datasets list
│   └── [id]/page.tsx       — Dataset detail + inline rename
├── data-sources/page.tsx   — Data source registration
├── lab/
│   ├── page.tsx            — SQL Lab (Monaco + multi-tab + results grid)
│   └── queries/page.tsx    — Saved queries + query history
├── favorites/page.tsx      — Favourites list
└── workspace-activity/     — Team query history
```

### MSAL Authentication Flow

```
1. User visits any page
2. MsalProvider checks for active account
3. If no account → redirect to Azure AD login (PKCE flow)
4. After login → Azure AD redirects back to /
5. MSAL stores access token in sessionStorage
6. Every API call uses msalFetch():
   a. acquireTokenSilent() — get token from cache or refresh
   b. If silent fails → acquireTokenRedirect()
   c. Adds Authorization: Bearer <token> header
```

---

## Deployment

See [DEPLOYMENT.md](./DEPLOYMENT.md) for the full Azure Container Apps deployment guide.

### Summary

| Concern | Approach |
|---|---|
| Container builds | Docker multi-stage (Python 3.11-slim + ODBC 18 for API; Node 20 Alpine for web) |
| Image registry | Azure Container Registry (ACR) — no admin credentials |
| Image pull auth | User-Assigned Managed Identity with AcrPull role |
| Fabric SQL auth | Managed Identity via `DefaultAzureCredential` — no passwords |
| CI/CD auth | OIDC Workload Identity Federation — no GitHub secrets |
| API runtime | Gunicorn + Uvicorn workers (4 workers × 8 threads) |
| Scaling | Azure Container Apps — min 1 replica, auto-scale under load |
| Cold starts | Connection pool warmup at startup; 5-min heartbeat to keep Fabric serverless warm |
| First run | Setup wizard at `/api/v1/setup/*` — disabled once configured |
