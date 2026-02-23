# LOOMX Architecture

**Live Operational Outcomes & Metrics eXperience**

> Comprehensive technical architecture documentation for the LOOMX platform

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

LOOMX is a modern data exploration platform built with a clean, layered architecture. The system consists of three main components:

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
                            ↓ HTTPS/REST
┌─────────────────────────────────────────────────────────────┐
│              Express.js API Backend (Port 8080)              │
│  ┌──────────────────────────────────────────────────────┐   │
│  │  Routes → Services → Adapters → Database             │   │
│  │  - Multi-level caching                               │   │
│  │  - Azure AD validation                               │   │
│  │  - FabricExplorer compatibility                      │   │
│  └──────────────────────────────────────────────────────┘   │
└─────────────────────────────────────────────────────────────┘
                            ↓ HTTP
┌─────────────────────────────────────────────────────────────┐
│            Python ODBC Proxy (Port 5001)                     │
│  ┌──────────────────────────────────────────────────────┐   │
│  │  - ODBC connection pooling                           │   │
│  │  - Query execution                                   │   │
│  │  - Result serialization                              │   │
│  └──────────────────────────────────────────────────────┘   │
└─────────────────────────────────────────────────────────────┘
                            ↓ ODBC/TDS
┌─────────────────────────────────────────────────────────────┐
│                Microsoft Fabric SQL Endpoints                │
│  ┌──────────────────────────────────────────────────────┐   │
│  │  Metadata Database (LOOMX tables)                    │   │
│  │  - stores: datasets, charts, dashboards, etc.       │   │
│  │  - stores: data_sources table (warehouse configs)   │   │
│  └──────────────────────────────────────────────────────┘   │
│  ┌──────────────────────────────────────────────────────┐   │
│  │  Data Warehouses (User Data - Dynamic)              │   │
│  │  - Configured via UI at /data-sources               │   │
│  │  - Connection info retrieved from data_sources table│   │
│  │  - Python proxy creates connections dynamically     │   │
│  └──────────────────────────────────────────────────────┘   │
└─────────────────────────────────────────────────────────────┘
```

---

## Configuration Architecture

### Single Root `.env` File Pattern

LOOMX uses a **centralized configuration approach** with a single `.env` file at the repository root. All services (API, Web, Python Proxy) load their configuration from this shared file.

```
D:\Repos\IDEASFabric\sources\dev\Tools\LOOMX\
├── .env                           ← Single configuration file
├── apps/
│   ├── loomx-api/
│   │   └── src/server.ts          → Loads ../../.env
│   ├── loomx-web/
│   │   └── next.config.ts         → Loads ../../.env
│   └── loomx-python-proxy/
│       └── proxy.py               → Loads ../../.env
```

### Design Benefits

- **Single source of truth** - One file contains all environment configuration
- **Consistency** - All services use identical configuration values
- **Simplified setup** - One file to copy and configure
- **Reduced maintenance** - Update configuration in one place
- **Environment parity** - Easy to replicate configuration across environments

### Configuration Separation

**What Goes in .env**:
- ✅ Metadata database connection (stores LOOMX data)
- ✅ Azure AD credentials
- ✅ Service URLs and ports

**What DOES NOT Go in .env**:
- ❌ Data warehouse endpoints
- ❌ Lakehouse endpoints
- ❌ User data source connections

**Why?**
- Data sources are **dynamic** and user-managed
- Stored in `data_sources` table in metadata database
- Configured through UI at `/data-sources`
- No code/config changes needed to add warehouses

### Dynamic Data Source Discovery Flow

```
┌──────────────────────────────────────────────────────────────┐
│ 1. User adds warehouse via UI (/data-sources)               │
│    - Name: "Sales Warehouse"                                 │
│    - Endpoint: "sales-endpoint.fabric.microsoft.com"        │
│    - Database: "SalesDB"                                     │
└──────────────────────────────────────────────────────────────┘
                            ↓
┌──────────────────────────────────────────────────────────────┐
│ 2. Saved to metadata database → data_sources table          │
│    INSERT INTO data_sources (name, endpoint, database_name) │
└──────────────────────────────────────────────────────────────┘
                            ↓
┌──────────────────────────────────────────────────────────────┐
│ 3. User runs query with database="SalesDB"                  │
│    Frontend → API → Python Proxy                            │
└──────────────────────────────────────────────────────────────┘
                            ↓
┌──────────────────────────────────────────────────────────────┐
│ 4. Python proxy queries metadata database                   │
│    SELECT endpoint FROM data_sources                         │
│    WHERE database_name = 'SalesDB'                          │
└──────────────────────────────────────────────────────────────┘
                            ↓
┌──────────────────────────────────────────────────────────────┐
│ 5. Python proxy creates/reuses connection pool              │
│    - Caches endpoint info (5 min TTL)                       │
│    - Creates ODBC connection pool for that database         │
│    - Executes query                                          │
└──────────────────────────────────────────────────────────────┘
```

### Benefits of This Architecture

1. **Scalability**: Support unlimited data sources without config changes
2. **User Empowerment**: Non-technical users can add warehouses via UI
3. **No Downtime**: Add/remove data sources without restarting services
4. **Centralized Management**: All connection info in metadata database
5. **Audit Trail**: Track who added/modified data sources
6. **Environment Parity**: Dev/staging/prod use same codebase, different metadata DB

---

## Architecture Layers

### 1. Presentation Layer (Next.js Frontend)

**Location**: `apps/loomx-web/`

**Responsibilities**:
- User interface rendering
- Client-side routing (Next.js App Router)
- State management (React Context)
- Azure AD authentication (MSAL)
- Data visualization (ECharts)
- SQL editing (Monaco Editor)

**Key Technologies**:
- Next.js 15 (App Router, React Server Components)
- React 19 (Client Components, Hooks)
- TypeScript (strict mode)
- CSS (global styles, component-scoped)

**Key Features**:
- **Server-Side Rendering (SSR)** - For better SEO and initial load performance
- **Dynamic Routes** - `/charts/[id]`, `/dashboards/[id]`, `/datasets/[id]`
- **Theme System** - User-specific color themes with localStorage persistence
- **Favicon Generation** - Dynamic favicons using Next.js ImageResponse API

### 2. API Layer (Express.js Backend)

**Location**: `apps/loomx-api/`

**Responsibilities**:
- RESTful API endpoints
- Request validation
- Business logic orchestration
- Authentication/authorization
- Caching (query, metadata, response)
- Error handling

**Architecture Pattern**: **Layered Architecture**

```
┌────────────────────────────────────────────────────────┐
│                      Routes Layer                       │
│  - HTTP request/response handling                       │
│  - Input validation                                     │
│  - Error handling middleware                            │
│  Files: src/routes/*.ts                                 │
└────────────────────────────────────────────────────────┘
                         ↓
┌────────────────────────────────────────────────────────┐
│                    Services Layer                       │
│  - Business logic                                       │
│  - Data orchestration                                   │
│  - Transaction management                               │
│  Files: src/services/*.service.ts                       │
└────────────────────────────────────────────────────────┘
                         ↓
┌────────────────────────────────────────────────────────┐
│                    Adapters Layer                       │
│  - FabricExplorer compatibility                         │
│  - Data transformation                                  │
│  - Format conversion                                    │
│  Files: src/adapters/*.adapters.ts                      │
└────────────────────────────────────────────────────────┘
                         ↓
┌────────────────────────────────────────────────────────┐
│                    Database Layer                       │
│  - Connection pooling                                   │
│  - Query execution                                      │
│  - Transaction handling                                 │
│  Files: src/db/*.ts                                     │
└────────────────────────────────────────────────────────┘
```

### 3. Data Access Layer (Python Proxy)

**Location**: `apps/loomx-python-proxy/`

**Responsibilities**:
- **Dynamic data source discovery** - Queries `data_sources` table from metadata DB
- **ODBC connection pooling** (max 10 connections per data source)
- **Connection pool management** - Creates pools dynamically per database
- SQL query execution
- Result serialization (JSON)
- Connection lifecycle management

**Why Python?**
- Native ODBC support (pyodbc)
- Better Windows authentication support
- Connection pooling for performance

**Dynamic Data Source Loading**:
1. On startup: Connects to metadata database (from `.env`)
2. On query request: Checks `data_sources` table for warehouse endpoint
3. Creates/reuses connection pool for that specific database
4. Caches data source info (5 min TTL) to reduce metadata queries
5. Supports unlimited warehouses/lakehouses configured via UI

**Timeout Configuration**:

The Node.js API enforces a per-request timeout when calling the Python proxy. This is configurable to accommodate long-running Fabric Delta Lake scans:

| Variable | Default | Description |
|----------|---------|-------------|
| `PYTHON_PROXY_TIMEOUT_MS` | `120000` (120 s) | Max ms to wait for a proxy query response |

Set in root `.env`:
```env
PYTHON_PROXY_TIMEOUT_MS=120000   # 120 s — increase for slow Delta Lake scans
```

---

## Data Flow

### Read Operation (GET Request)

```
1. User Action (Frontend)
   ↓
2. MSAL Token Acquisition
   ↓
3. HTTP Request with Authorization header
   ↓
4. API: Check Response Cache (1 min TTL)
   ├─ HIT → Return cached response
   └─ MISS ↓
5. API: Route Handler validates request
   ↓
6. API: Service Layer processes logic
   ↓
7. API: Check Query/Metadata Cache
   ├─ HIT → Return cached data
   └─ MISS ↓
8. API: Database Layer executes query
   ↓
9. Response → Cache → User
```

### Write Operation (POST/PUT/DELETE Request)

```
1. User Action (Frontend)
   ↓
2. MSAL Token Acquisition
   ↓
3. HTTP Request with Authorization header
   ↓
4. API: Route Handler validates request
   ↓
5. API: Service Layer processes business logic
   ↓
6. API: Database Layer executes transaction
   ↓
7. API: Invalidate relevant caches
   ↓
8. Response → User
```

### SQL Query Execution Flow

```
1. User writes SQL in Monaco Editor
   ↓
2. Frontend sends query to /api/v1/sql/execute
   {
     "sql": "SELECT * FROM Products",
     "database": "SalesDB"
   }
   ↓
3. API validates query (syntax, permissions)
   ↓
4. API checks query cache (SQL hash)
   ├─ HIT → Return cached results
   └─ MISS ↓
5. API forwards to Python Proxy (with database parameter)
   ↓
6. Python Proxy: Query data_sources table
   ├─ Check cache for database endpoint (5 min TTL)
   └─ If not cached: SELECT endpoint FROM data_sources
                      WHERE database_name = 'SalesDB'
   ↓
7. Python Proxy: Get/create connection pool for endpoint
   ├─ Pool exists → Reuse connection
   └─ Pool missing → Create new pool (max 10 connections)
   ↓
8. Python Proxy: Execute via ODBC
   ↓
9. Results → Serialize to JSON → Cache → User
   ↓
10. Frontend renders in data grid
```

---

## Authentication

### Azure AD Authentication Flow (MSAL)

```
┌─────────────┐
│   Browser   │
└─────────────┘
       │
       │ 1. User clicks "Sign In"
       ↓
┌─────────────────────┐
│   MSAL Library      │
│   (Frontend)        │
└─────────────────────┘
       │
       │ 2. Redirect to Azure AD
       ↓
┌─────────────────────┐
│   Azure AD          │
│   Login Page        │
└─────────────────────┘
       │
       │ 3. User authenticates
       │ 4. Azure AD returns code
       ↓
┌─────────────────────┐
│   MSAL Library      │
│   Exchanges code    │
│   for tokens        │
└─────────────────────┘
       │
       │ 5. Store access token
       │    (memory + localStorage)
       ↓
┌─────────────────────┐
│   API Requests      │
│   Authorization:    │
│   Bearer <token>    │
└─────────────────────┘
       │
       │ 6. API validates token
       ↓
┌─────────────────────┐
│   Express API       │
│   - Validates token │
│   - Extracts claims │
│   - Authorizes req  │
└─────────────────────┘
```

### Token Validation (API)

**File**: `apps/loomx-api/src/middleware/authMiddleware.ts`

Applied globally via `app.use(extractUser)` in `server.ts`.

1. Extract `Authorization: Bearer <token>` header
2. Base64url-decode the JWT payload (no network round-trip)
3. Validate `exp` claim — reject expired tokens
4. Validate `iss` claim — must start with `https://login.microsoftonline.com/`
5. Extract identity from `preferred_username` → `email` → `upn` (in that priority)
6. Attach `{ email }` to `req.user` — available to all downstream route handlers
7. Continue to route handler (non-blocking; missing/invalid token sets `req.user = undefined`)

> **Note**: JWT signature verification is not yet performed server-side (no `jsonwebtoken` / `jwks-rsa` dependency). The current implementation trusts the Azure AD–signed token passed by the MSAL-authenticated frontend. Full JWKS verification is documented as a future hardening step in `SECURITY.md`.

### User Identity Resolution

**File**: `apps/loomx-api/src/middleware/userContext.ts`

All route handlers call `getCurrentUserId(req)` for a consistent, priority-ordered identity:

```
1. req.user.email         ← JWT Bearer token (highest trust)
2. x-user-email header    ← Client header (validated: must contain @)
3. 'anonymous'            ← Unauthenticated fallback (never 'system')
```

### Session Management

- **Frontend**: Tokens stored in memory and localStorage
- **Backend**: Stateless (no server-side sessions)
- **Token Refresh**: MSAL handles automatic refresh using refresh tokens
- **Logout**: Clear localStorage and memory, revoke token with Azure AD

---

## Middleware

The API middleware stack is applied in the following order in `server.ts`:

```
app.use(cors(...))
app.use(express.json())
app.use(extractUser)          ← JWT decode, sets req.user
app.use('/api/v1/...', router)
app.use(errorHandler)         ← Centralised error shape + production sanitisation
```

### `extractUser` — Identity Extraction

**File**: `apps/loomx-api/src/middleware/authMiddleware.ts`

| Behaviour | Detail |
|-----------|--------|
| Reads | `Authorization: Bearer <jwt>` header |
| Validates | `exp` (not expired), `iss` (Azure AD prefix) |
| Sets | `req.user = { email: string }` on success |
| On failure | Passes through — `req.user` remains `undefined` |
| Non-blocking | Routes work without auth; use `requireAuth` to enforce |

### `requireAuth` — Route Guard

Exported from the same file. Returns `401 Unauthorized` if `req.user` is not set. Apply to routes that must be authenticated:

```typescript
import { requireAuth } from '../middleware/authMiddleware';
router.post('/sensitive', requireAuth, asyncHandler(...));
```

### `getCurrentUserId` — Identity Helper

**File**: `apps/loomx-api/src/middleware/userContext.ts`

Single canonical function used by **all four** route files. Eliminates repeated inline copies and ensures consistent fallback behaviour:

```typescript
export function getCurrentUserId(req: any): string {
  if (req.user?.email) return req.user.email;           // JWT — highest trust
  const h = req.headers['x-user-email'] as string;
  if (h && h.includes('@')) return h;                   // Validated header
  return 'anonymous';                                   // Safe fallback
}
```

### `errorHandler` — Centralised Error Responses

**File**: `apps/loomx-api/src/middleware/errorHandler.ts`

- Maps `ValidationError` → 400, `NotFoundError` → 404, `UnauthorizedError` → 401
- In `NODE_ENV=production`: strips internal `error.message` from responses to prevent information leakage
- Exports `asyncHandler(fn)` wrapper — converts async route errors to `next(err)` calls

---

## Database Schema

### Metadata Database (LOOMX Storage)

**Location**: Configured via `FABRIC_METADATA_ENDPOINT` and `FABRIC_METADATA_DATABASE`

#### Core Tables

**`datasets`**
- Semantic layer definitions
- Fact table, schema, database name
- Date column for time-based filtering
- JSON field for tables_used (includes filters)

**`dataset_dimensions`**
- Dimension table relationships
- Join conditions (e.g., `fact.ProductKey = dim.ProductKey`)
- Display names for UI

**`dataset_columns`**
- Column metadata (table, name, data type)
- Semantic types (e.g., "Product", "Customer")
- Flags: is_dimension, is_metric

**`charts`**
- Chart configurations
- `query_config` (JSON): metric, groupby, filters, time_range
- `viz_config` (JSON): ECharts options (colors, fonts, legend)
- Dataset foreign key

**`dashboards`**
- Dashboard layouts
- `layout` (JSON): Grid layout with positions and sizes
- `filters` (JSON): Dashboard-level filters
- `charts` (JSON): Array of chart IDs

**`favorites`**
- User favorites (polymorphic)
- Supports: datasets, charts, dashboards, data_sources
- `object_id` + `object_type` + `user_email` unique constraint

**`user_themes`**
- Per-user theme colors
- `user_email` + `theme_color` (hex string)

**`data_sources`**
- Dynamic data source configurations
- Warehouse/lakehouse endpoints
- Connection strings

**`saved_queries`**
- User-saved SQL queries
- Query text, name, description

**`query_history`**
- Query execution history
- Execution time, row count, status

**`activity`**
- Audit log for user actions
- Action type, object type, object ID, user email

### Data Warehouses (User Data)

**Location**: **Dynamically discovered from `data_sources` table** (managed in UI at `/data-sources`)

**Architecture**:
- ✅ **Single source of truth**: `data_sources` table in metadata database
- ✅ **No hardcoded endpoints**: All warehouse connections configured dynamically
- ✅ **UI management**: Users add/edit warehouses through web interface
- ✅ **Python proxy discovery**: Queries metadata DB for endpoint info on-demand
- ✅ **Connection pooling**: Creates pools per database dynamically

**data_sources Table Schema**:
```sql
CREATE TABLE data_sources (
  id INT PRIMARY KEY IDENTITY(1,1),
  name NVARCHAR(255) NOT NULL,
  endpoint NVARCHAR(255) NOT NULL,      -- Fabric SQL endpoint
  database_name NVARCHAR(255) NOT NULL, -- Database name
  is_active BIT DEFAULT 1,
  created_at DATETIME2 DEFAULT GETDATE(),
  created_by NVARCHAR(255)
);
```

**Data Flow**:
1. User configures warehouse in UI → Saved to `data_sources` table
2. User runs query with `database=MyWarehouse`
3. Python proxy queries `SELECT endpoint FROM data_sources WHERE database_name = 'MyWarehouse'`
4. Python proxy creates/reuses connection pool for that endpoint
5. Query executes on discovered endpoint

**Benefits**:
- No environment variable updates needed for new warehouses
- Supports unlimited data sources
- User business data (sales, customers, products, etc.)
- Accessed via Python ODBC proxy
- Not modified by LOOMX (read-only for most operations)

---

## Feature Architecture

### Overview: How Features Connect

```
┌──────────────────────────────────────────────────────────────┐
│                     LOOMX Feature Stack                      │
├──────────────────────────────────────────────────────────────┤
│                                                              │
│  1. Datasets (Semantic Layer)                                │
│     - Define business entities (Products, Sales, etc.)       │
│     - Map to database tables and columns                     │
│     - Provide dimensions (what to group by)                  │
│     - Provide metrics (what to measure)                      │
│                                                              │
│  2. Charts (Visualization)                                   │
│     - Built on top of Datasets                               │
│     - Select metrics and dimensions                          │
│     - Apply filters and time ranges                          │
│     - Configure visualization (colors, chart type, etc.)     │
│     - Generate SQL → Execute → Visualize with ECharts        │
│                                                              │
│  3. Dashboards (Composition)                                 │
│     - Compose multiple Charts                                │
│     - Arrange in grid layout                                 │
│     - Apply dashboard-level filters (affects all charts)     │
│     - Interactive drill-down and filtering                   │
│                                                              │
│  4. SQL Lab (Direct Query)                                   │
│     - Direct SQL editor (Monaco)                             │
│     - Execute against any configured data source             │
│     - Save queries for reuse                                 │
│     - View query history                                     │
│                                                              │
└──────────────────────────────────────────────────────────────┘
```

---

### 1. Datasets: The Semantic Layer

**Purpose**: Datasets provide a business-friendly abstraction over database tables, defining what data means and how it should be used.

#### Dataset Components

**Fact Table**:
- The main table containing measurable data (e.g., `SalesTransactions`)
- Stored as: `database_name.schema_name.table_name`

**Dimensions** (What to group by):
- Related dimension tables (e.g., `Products`, `Customers`, `DateDim`)
- Join conditions: `fact.ProductKey = dim.ProductKey`
- Display names for UI

**Columns**:
- All available columns from fact + dimension tables
- Semantic types: `Product`, `Customer`, `Date`, `Metric`
- Flags: `is_dimension`, `is_metric`

**Filters**:
- Pre-defined filters users can apply
- Stored in `tables_used` JSON field

#### Dataset Flow

```
1. User creates Dataset
   ↓
2. Frontend → POST /api/v1/datasets
   {
     "name": "Sales Analysis",
     "database_name": "SalesDB",
     "schema_name": "dbo",
     "table_name": "FactSales",
     "dimensions": [
       {
         "table_name": "DimProduct",
         "join_condition": "fact.ProductKey = DimProduct.ProductKey"
       }
     ]
   }
   ↓
3. API stores in metadata database → datasets table
   ↓
4. API queries data source to discover columns
   ↓
5. Stores column metadata → dataset_columns table
   ↓
6. Dataset ready to use for Charts
```

#### Dataset Storage

```sql
-- datasets table
CREATE TABLE datasets (
  id INT PRIMARY KEY IDENTITY,
  name NVARCHAR(255),
  database_name NVARCHAR(255),  -- Which data source
  schema_name NVARCHAR(255),    -- e.g., "dbo"
  table_name NVARCHAR(255),     -- Fact table name
  date_column NVARCHAR(255),    -- For time filtering
  tables_used NVARCHAR(MAX),    -- JSON: includes filters
  ...
);

-- dataset_dimensions table
CREATE TABLE dataset_dimensions (
  id INT PRIMARY KEY IDENTITY,
  dataset_id INT,
  dimension_table_name NVARCHAR(255),
  join_condition NVARCHAR(MAX),
  display_name NVARCHAR(255),
  ...
);

-- dataset_columns table
CREATE TABLE dataset_columns (
  id INT PRIMARY KEY IDENTITY,
  dataset_id INT,
  table_name NVARCHAR(255),
  column_name NVARCHAR(255),
  data_type NVARCHAR(100),
  semantic_type NVARCHAR(100),  -- Product, Customer, Date, etc.
  is_dimension BIT,
  is_metric BIT,
  ...
);
```

---

### 2. Charts: Visualization Engine

**Purpose**: Charts transform dataset configurations into executable SQL queries and visualize results with ECharts.

#### Chart Components

**Query Configuration** (`query_config` JSON):
```json
{
  "metric": "SalesAmount",           // What to measure
  "groupby": ["ProductCategory"],    // How to group
  "filters": [                       // What to filter
    {
      "column": "Region",
      "operator": "=",
      "value": "North"
    }
  ],
  "time_range": {                    // Time filtering
    "column": "OrderDate",
    "start": "2024-01-01",
    "end": "2024-12-31"
  }
}
```

**Visualization Configuration** (`viz_config` JSON):
```json
{
  "chart_type": "bar",
  "colors": ["#0078D4", "#50E6FF"],
  "show_legend": true,
  "show_data_labels": true,
  "title": "Sales by Category",
  "x_axis_label": "Category",
  "y_axis_label": "Sales ($)",
  ...
}
```

#### Chart Rendering Architecture

Charts use a **shared hydration pattern** across all rendering contexts (chart builder page and dashboard). This eliminates duplicate state-management code and ensures identical query/render behaviour wherever a chart appears.

```
ChartBuilderProvider               ← React Context: holds all chart state
├── ChartHydrator (chart={...})    ← Populates context from a chart config object
│   └── renders null               ← Pure side-effect component
└── [Consumer]
    ├── CreateChartLayout          ← Full builder UI  (chart detail page)
    └── ChartPreview               ← Visualisation only (dashboard context)
```

**`ChartBuilderProvider`** (`components/charts/ChartBuilderContext.tsx`)
— Provides the shared state: metric, groupby, filters, time range, viz config, run state, etc.

**`ChartHydrator`** (`components/charts/ChartHydrator.tsx`)
— Accepts a raw `chart` object (from the API) and an optional `externalFilters` array.
Populates `ChartBuilderContext` via `useEffect`. Renders `null` — it is a side-effect component only.
Merges chart-level filters with any `externalFilters` passed by the parent (e.g., dashboard-level filters).

**`CreateChartLayout`** (`components/charts/CreateChartLayout.tsx`)
— Full drag-and-drop builder UI. Consumed only on the chart detail page (`/charts/[id]`).

**`ChartPreview`** (`components/charts/ChartPreview.tsx`)
— Renders the ECharts visualisation. Used inside dashboards, where no builder UI is needed.

#### Chart Flow: From Config to Visualization

```
1. User creates/edits Chart
   ↓
2. Frontend: Chart Builder UI (CreateChartLayout)
   - Select metric, group-by, filters, time range
   - Configure visualization options
   ↓
3. Frontend → POST/PUT /api/v1/charts
   {
     "dataset_id": 123,
     "query_config": { metric, groupby, filters },
     "viz_config": { chart_type, colors, ... }
   }
   ↓
4. API stores in charts table
   ↓
5. User navigates to chart page → GET /api/v1/charts/:id
   ↓
6. Frontend: ChartBuilderProvider wraps the page
   ChartHydrator receives the chart object and populates context
   CreateChartLayout renders the builder
   ↓
7. On "Run" — Frontend → POST /api/v1/sql/generate
   Query Generator Service builds SQL from query_config:

   SELECT
     ProductCategory,
     SUM(SalesAmount) as SalesAmount
   FROM SalesDB.dbo.FactSales fact
   JOIN SalesDB.dbo.DimProduct dim ON fact.ProductKey = dim.ProductKey
   WHERE Region = 'North'
     AND OrderDate >= '2024-01-01'
     AND OrderDate <= '2024-12-31'
   GROUP BY ProductCategory
   ORDER BY SalesAmount DESC
   ↓
8. Frontend → POST /api/v1/sql/execute → Python Proxy → Fabric SQL
   ↓
9. Frontend: ECharts renders visualisation
   - Applies viz_config (colors, chart type, axes, legend)
   - Shows interactive chart
```

#### Chart Storage

```sql
CREATE TABLE charts (
  id INT PRIMARY KEY IDENTITY,
  name NVARCHAR(255),
  dataset_id INT,                    -- Links to dataset
  query_config NVARCHAR(MAX),        -- JSON: metric, groupby, filters
  viz_config NVARCHAR(MAX),          -- JSON: ECharts options
  created_by NVARCHAR(255),
  created_at DATETIME2,
  ...
);
```

---

### 3. Dashboards: Multi-Chart Composition

**Purpose**: Dashboards compose multiple charts with shared filtering and interactive layout.

#### Dashboard Components

**Charts Array**:
- List of chart IDs to display
- Each chart executes independently

**Layout** (Grid positions):
```json
{
  "chart_123": { "x": 0, "y": 0, "w": 6, "h": 4 },
  "chart_456": { "x": 6, "y": 0, "w": 6, "h": 4 },
  "chart_789": { "x": 0, "y": 4, "w": 12, "h": 4 }
}
```

**Dashboard Filters** (Applied to all charts):
```json
[
  {
    "column": "Region",
    "operator": "IN",
    "value": ["North", "South"]
  }
]
```

#### Dashboard Rendering Architecture

A dashboard is a **layout of independent charts**. Each chart slot uses the same shared `ChartHydrator` + `ChartPreview` stack used on the chart detail page — the only difference is that `CreateChartLayout` is omitted (no builder UI needed).

```
DashboardCanvas
└── For each chart slot in layout:
    └── DashboardChartComponent
        └── DashboardChartLoader (fetches chart definition, preload cache)
            └── ChartBuilderProvider (isolated state per chart)
                ├── ChartHydrator (chart={...}, externalFilters={dashboardFilters})
                └── ChartPreview   (ECharts visualisation)
```

**Preload Cache**: Chart definitions are cached in memory per session so navigating between dashboard view/edit does not re-fetch unchanged charts.

**Filter Propagation**: Dashboard-level filters are passed as `externalFilters` to each `ChartHydrator`. They are merged with the chart's own filters before SQL generation — charts always re-execute when dashboard filters change.

#### Dashboard Flow

```
1. User creates Dashboard
   ↓
2. Frontend → POST /api/v1/dashboards
   {
     "name": "Sales Overview",
     "layout": { rows, columns, chart references }
   }
   ↓
3. API stores in dashboards table
   ↓
4. User views Dashboard → GET /api/v1/dashboards/:id
   ↓
5. API returns dashboard config (includes is_favorite for the requesting user)
   ↓
6. Frontend renders grid layout:
   - DashboardCanvas renders rows/columns from layout config
   - Each chart slot mounts an isolated ChartBuilderProvider
   - DashboardChartLoader fetches chart definition (preload cache first)
   - ChartHydrator populates context; ChartPreview runs the query and renders
   - All chart slots load independently and in parallel
   ↓
7. User applies dashboard filter:
   - externalFilters prop on each ChartHydrator updates
   - Each chart re-runs its query with the merged filter set
   - No page reload; all charts re-execute concurrently
```

#### Dashboard Storage

```sql
CREATE TABLE dashboards (
  id INT PRIMARY KEY IDENTITY,
  name NVARCHAR(255),
  description NVARCHAR(MAX),
  charts NVARCHAR(MAX),          -- JSON: array of chart IDs
  layout NVARCHAR(MAX),          -- JSON: grid positions
  filters NVARCHAR(MAX),         -- JSON: dashboard-level filters
  created_by NVARCHAR(255),
  created_at DATETIME2,
  ...
);
```

---

### 4. SQL Lab: Direct Query Interface

**Purpose**: SQL Lab provides a direct SQL editor for ad-hoc queries, exploration, and debugging.

#### SQL Lab Features

**1. Monaco SQL Editor**:
- Syntax highlighting
- IntelliSense (table/column suggestions)
- Multi-line queries

**2. Data Source Selection**:
- Dropdown to select database
- Lists all active data sources from `data_sources` table

**3. Query Execution**:
- Execute any SQL query
- Display results in data grid
- Export to CSV

**4. Query History**:
- Auto-saves executed queries
- Stored in `query_history` table
- View/re-run previous queries

**5. Saved Queries**:
- Save queries with name/description
- Stored in `saved_queries` table
- Share with team

#### SQL Lab Flow

```
1. User opens SQL Lab → /lab
   ↓
2. Frontend loads:
   - GET /api/v1/data-sources/active → List data sources
   - GET /api/v1/lab/tables?database=SalesDB → List tables
   ↓
3. User writes SQL:
   SELECT * FROM Products WHERE Category = 'Electronics'
   ↓
4. User clicks Execute
   ↓
5. Frontend → POST /api/v1/sql/execute
   {
     "sql": "SELECT * FROM Products...",
     "database": "SalesDB"
   }
   ↓
6. API validates query (basic syntax check)
   ↓
7. API checks query cache (SQL hash)
   ├─ HIT → Return cached results
   └─ MISS ↓
   ↓
8. API → POST http://localhost:5001/api/v1/execute
   {
     "sql": "SELECT * FROM Products...",
     "database": "SalesDB"
   }
   ↓
9. Python Proxy:
   a. Queries data_sources table:
      SELECT connection_string FROM data_sources
      WHERE database_name = 'SalesDB'
   b. Gets/creates connection pool for endpoint
   c. Executes SQL via ODBC
   d. Returns results as JSON
   ↓
10. API caches results (5 min TTL)
   ↓
11. API saves to query_history table
   ↓
12. Results returned to Frontend
   ↓
13. Frontend renders in data grid
```

#### SQL Lab Storage

```sql
-- Query history (auto-saved)
CREATE TABLE query_history (
  id INT PRIMARY KEY IDENTITY,
  user_email NVARCHAR(255),
  database_name NVARCHAR(255),
  sql_query NVARCHAR(MAX),
  execution_time_ms INT,
  row_count INT,
  status NVARCHAR(50),      -- 'success' or 'error'
  error_message NVARCHAR(MAX),
  created_at DATETIME2,
  ...
);

-- Saved queries (user-saved)
CREATE TABLE saved_queries (
  id INT PRIMARY KEY IDENTITY,
  name NVARCHAR(255),
  description NVARCHAR(MAX),
  database_name NVARCHAR(255),
  sql_query NVARCHAR(MAX),
  created_by NVARCHAR(255),
  created_at DATETIME2,
  ...
);
```

---

### How Everything Connects

#### The Big Picture

```
┌─────────────────────────────────────────────────────────────┐
│                    User Workflows                           │
└─────────────────────────────────────────────────────────────┘

1. Data Source Configuration
   UI: /data-sources → Add warehouse endpoint
   Storage: data_sources table
   Result: Python proxy can connect to warehouse

2. Dataset Creation
   UI: /datasets/new → Define semantic layer
   Storage: datasets, dataset_dimensions, dataset_columns
   Result: Business-friendly data model

3. Chart Creation
   UI: /charts/new → Select dataset, configure query & viz
   Flow: Dataset → Generate SQL → Python Proxy → Execute
   Storage: charts table
   Result: Visualized insights

4. Dashboard Creation
   UI: /dashboards/new → Compose charts, add layout
   Flow: Load multiple charts, apply shared filters
   Storage: dashboards table
   Result: Multi-chart analytics view

5. Ad-Hoc Exploration
   UI: /lab → Write SQL, explore tables
   Flow: Direct SQL → Python Proxy → Execute
   Storage: query_history, saved_queries
   Result: Flexible data exploration
```

#### Data Flow Summary

```
User Action
    ↓
Frontend (React UI)
    ↓
API (Express.js) - Routes → Services → Adapters
    ↓
Python Proxy (Query data_sources for endpoint)
    ↓
Connection Pool (Get/create pool for database)
    ↓
ODBC Connection (Execute SQL)
    ↓
Fabric SQL (Metadata DB or Data Warehouse)
    ↓
Results (JSON)
    ↓
Cache (Query cache, response cache)
    ↓
Frontend (Render: ECharts, data grid, etc.)
```

---

## Caching Strategy

### Three-Level Cache

#### 1. Response Cache (HTTP)
**Location**: `apps/loomx-api/src/middleware/cache.ts`

- **What**: Full HTTP responses for GET requests
- **TTL**: 60 seconds
- **Key**: `${method}:${path}:${queryString}`
- **Invalidation**: Automatic (TTL), manual on writes

**Example**:
```typescript
// Cache middleware
if (req.method === 'GET') {
  const cacheKey = `${req.method}:${req.path}:${JSON.stringify(req.query)}`;
  const cached = cache.get(cacheKey);
  if (cached) {
    return res.json(cached);
  }
}
```

#### 2. Query Cache (SQL Results)
**Location**: `apps/loomx-api/src/services/*.service.ts`

- **What**: SQL query results
- **TTL**: 300 seconds (5 minutes)
- **Key**: Hash of SQL + parameters
- **Invalidation**: Automatic (TTL)

**Example**:
```typescript
const cacheKey = hashQuery(sql, params);
const cached = queryCache.get(cacheKey);
if (cached) return cached;
const results = await db.query(sql, params);
queryCache.set(cacheKey, results);
```

#### 3. Metadata Cache (Schema)
**Location**: `apps/loomx-api/src/services/metadata.service.ts`

- **What**: Table schemas, column lists
- **TTL**: 600 seconds (10 minutes)
- **Key**: `metadata:${tableName}`
- **Invalidation**: Automatic (TTL)

### Cache Invalidation Strategy

**On Write Operations**:
- POST/PUT/DELETE requests invalidate related caches
- Invalidation by pattern matching (e.g., `/api/v1/charts/*`)
- Cascade invalidation (e.g., deleting dataset invalidates its charts)

**Example**:
```typescript
// After creating a chart
await chartsService.create(data);
cache.flushAll(); // Invalidate all caches
// OR selective invalidation:
cache.keys().forEach(key => {
  if (key.includes('charts')) cache.del(key);
});
```

---

## Component Architecture

### Frontend Component Hierarchy

```
RootLayout (app/layout.tsx)
├── Providers (app/providers.tsx)
│   ├── AuthProvider (auth/useAuth.tsx)
│   └── ThemeProvider (contexts/ThemeContext.tsx)
├── ClientLayout (components/ClientLayout.tsx)
│   ├── AuthScreen (not authenticated)
│   └── Layout (authenticated)
│       ├── Header
│       │   ├── Logo
│       │   ├── Navigation
│       │   └── User Menu
│       └── Main Content
│           ├── Page Routes (app/*/page.tsx)
│           ├── Modals
│           └── Toasts
```

### Key Components

#### LOOMXLogo (`components/LOOMXLogo.tsx`)
- SVG logo with theme-aware gradients
- Animation support (revolve, pulse)
- Click handler for navigation

#### Layout (`components/Layout.tsx`)
- Main app shell
- Header navigation
- User avatar with initials
- Theme-aware styling

#### Chart Builder Context (`components/charts/ChartBuilderContext.tsx`)
- React Context for all chart state
- Query configuration: metric, groupby, filters, time range, filter logic
- Visualization options: chart type, colors, fonts, legend, axes
- Run state: isRunning, results, error
- `runContext` prop (`'chart-builder'` | `'dashboard'`) drives query source tagging

#### ChartHydrator (`components/charts/ChartHydrator.tsx`)
- **Shared hydration component** — the single source of truth for populating `ChartBuilderContext` from a raw API chart object
- Used on both the chart detail page and inside each dashboard chart slot
- Accepts `chart` (API response) and optional `externalFilters` (from dashboard)
- Renders `null` — pure side-effect, no DOM output
- Eliminates all duplicate hydration code that previously existed in separate paths

#### Dashboard Builder Context (`components/dashboards/DashboardContext.tsx`)
- React Context for dashboard state
- Grid layout management (react-grid-layout)
- Dashboard-level filters (propagated to each ChartHydrator as `externalFilters`)
- Chart preloading and preload cache

---

## API Design

### RESTful Conventions

**Endpoints follow REST principles**:
- `GET /api/v1/resource` - List resources
- `GET /api/v1/resource/:id` - Get single resource
- `POST /api/v1/resource` - Create resource
- `PUT /api/v1/resource/:id` - Update resource
- `DELETE /api/v1/resource/:id` - Delete resource

### Response Format

**Success Response**:
```json
{
  "id": 123,
  "name": "My Chart",
  "data": { ... }
}
```

**Error Response**:
```json
{
  "error": "Resource not found",
  "code": "NOT_FOUND",
  "details": { ... }
}
```

### API Versioning

Current version: **v1** (`/api/v1/*`)

Future versions will be added as `/api/v2/*` without breaking v1.

---

## Frontend Architecture

### Next.js App Router Structure

```
apps/loomx-web/app/
├── layout.tsx                  # Root layout (metadata, fonts)
├── page.tsx                    # Home page
├── providers.tsx               # Context providers
├── icon.tsx                    # Dynamic favicon
├── apple-icon.tsx              # Apple touch icon
├── charts/
│   ├── page.tsx                # Charts list
│   ├── new/page.tsx            # Create chart (step 1)
│   ├── new/build/page.tsx      # Create chart (step 2)
│   └── [id]/
│       ├── page.tsx            # Edit chart
│       └── view/page.tsx       # View-only redirect
├── dashboards/
│   ├── page.tsx                # Dashboards list
│   ├── new/page.tsx            # Create dashboard
│   └── [id]/
│       ├── edit/page.tsx       # Edit dashboard
│       └── view/page.tsx       # View dashboard
├── datasets/
│   ├── page.tsx                # Datasets list
│   ├── new/page.tsx            # Create dataset
│   └── [id]/page.tsx           # View dataset
└── lab/
    ├── page.tsx                # SQL Lab
    └── queries/page.tsx        # Query history
```

### Dynamic Routes

All `[id]` routes are marked as **force-dynamic** to prevent static generation:

```typescript
export const dynamic = 'force-dynamic';
export const dynamicParams = true;
```

This ensures routes are always rendered on-demand with fresh data.

### State Management

**React Context API** for global state:
- **AuthContext** - User authentication state
- **ThemeContext** - User theme preferences

**Local State** (useState) for component-specific state.

**No Redux** - Kept simple with Context + Hooks.

---

## Deployment

### Development

**Quick Start** (Recommended):
```powershell
# Automated setup script (Windows)
.\start.ps1
```

The `start.ps1` script handles everything:
- ✅ Checks prerequisites (Node.js, Python, pnpm)
- ✅ Verifies root `.env` configuration
- ✅ Installs Python packages in virtual environment
- ✅ Installs Node.js dependencies
- ✅ Kills processes on ports 3000, 5001, 8080
- ✅ Starts all three services in separate windows
- ✅ Performs health checks

**Manual Start**:
```bash
# 1. Copy environment template
cp .env.example .env

# 2. Edit .env with your configuration
# (Only metadata database and Azure AD - no warehouse endpoints!)

# 3. Install dependencies
pnpm install

# 4. Start all services
pnpm dev
```

**Ports**:
- Web: http://localhost:3000
- API: http://localhost:8080
- Python Proxy: http://localhost:5001

**First Time Setup**:
1. Create root `.env` file from `.env.example`
2. Configure only metadata database connection
3. Run `start.ps1` or `pnpm dev`
4. Add data warehouses via UI at `/data-sources`

### Production Build

```bash
# Build all apps
pnpm build

# Start production servers
pnpm start
```

### Environment Configuration

**Single Root `.env` File Architecture**:
- ✅ All services read from **one** `.env` file at repository root
- ✅ No duplicate configuration across services
- ✅ Eliminates configuration drift
- ✅ Simpler setup and maintenance

**Location**: `D:\Repos\IDEASFabric\sources\dev\Tools\LOOMX\.env`

**Required Variables**:
```env
# Metadata Database (ONLY THIS - warehouses come from data_sources table!)
FABRIC_METADATA_ENDPOINT=your-metadata-endpoint.fabric.microsoft.com
FABRIC_METADATA_DATABASE=YourMetadataDatabase

# Azure AD Authentication
AZURE_TENANT_ID=your-tenant-id
AZURE_CLIENT_ID=your-client-id

# Service URLs
PYTHON_PROXY_URL=http://localhost:5001
API_URL=http://localhost:8080
WEB_URL=http://localhost:3000

# Node Environment
NODE_ENV=development
PORT=8080
```

**How Services Read Configuration**:

**API** (`apps/loomx-api/src/server.ts`):
```typescript
import { config } from 'dotenv';
import { resolve } from 'path';

// Load from root .env (two levels up from src/)
config({ path: resolve(__dirname, '../../../.env') });
```

**Web** (`apps/loomx-web/next.config.ts`):
```typescript
import { config } from 'dotenv';
import { resolve } from 'path';

// Load from root .env
config({ path: resolve(__dirname, '../../.env') });

// Map to NEXT_PUBLIC_ variables for browser access
const nextConfig: NextConfig = {
  env: {
    NEXT_PUBLIC_API_BASE_URL: process.env.API_URL || 'http://localhost:8080',
    NEXT_PUBLIC_AAD_CLIENT_ID: process.env.AZURE_CLIENT_ID || '',
    // ...
  },
};
```

**Python Proxy** (`apps/loomx-python-proxy/proxy.py`):
```python
from dotenv import load_dotenv
import os

# Load from root .env (two levels up)
load_dotenv(dotenv_path=os.path.join(os.path.dirname(__file__), '../../.env'))

FABRIC_METADATA_ENDPOINT = os.environ['FABRIC_METADATA_ENDPOINT']
FABRIC_METADATA_DATABASE = os.environ['FABRIC_METADATA_DATABASE']
```

**Important Notes**:
- ⚠️ **DO NOT** add data warehouse endpoints to `.env`
- ✅ Configure data warehouses via UI at `/data-sources`
- ✅ Warehouse connection info stored in `data_sources` table
- ✅ Python proxy queries metadata DB for warehouse endpoints dynamically

### Docker Deployment (Optional)

Dockerfiles can be created for containerized deployment:
- `loomx-api` → Express server
- `loomx-web` → Next.js SSR server
- `loomx-python-proxy` → Flask server

---

## Performance Optimizations

### Frontend
- **Next.js SSR** - Fast initial page loads
- **Code Splitting** - Lazy load components
- **Image Optimization** - Next.js Image component
- **Font Optimization** - Next.js Font system

### Backend
- **Connection Pooling** - Reuse database connections
- **Multi-level Caching** - Reduce database load
- **Parallel Queries** - Execute independent queries concurrently
- **Compression** - Gzip responses

### Database
- **Indexed Queries** - All primary/foreign keys indexed
- **Query Optimization** - Parameterized queries, avoid N+1
- **Connection Limits** - Max 10 connections per data source

---

## Security Considerations

> For the complete threat model, known limitations, and contributor security checklist, see **[`SECURITY.md`](./SECURITY.md)** at the repository root.

### Authentication
- Azure AD token-based authentication (MSAL on frontend, `extractUser` middleware on backend)
- JWT payload decoded and validated (exp, iss) on every API request via `authMiddleware.ts`
- Short-lived access tokens (1 hour); MSAL handles silent refresh automatically
- User identity sourced from JWT (`preferred_username`) — not from client-controlled headers

### Authorization
- User identity resolved via `getCurrentUserId(req)` in `userContext.ts` (JWT-first)
- User-scoped data for favorites and history
- `'anonymous'` fallback — never `'system'` — prevents privilege misattribution

### Input Validation
- SQL identifier injection: all schema/table/column names pass through `quoteIdentifier()` in `queryGenerator.service.ts`
- Filter operator allowlist (`SAFE_OPERATORS`) in `buildOptimizedFilterClause` — unknown operators default to `=`
- SQL Lab query size cap: 64 KB (`MAX_SQL_BYTES`) enforced before execution
- All write endpoints validated with `ValidationError` before service calls

### Data Protection
- HTTPS in production
- Sensitive data in environment variables (never committed — `.env` is in `.gitignore`)
- XSS prevention via React auto-escaping
- CSRF mitigation: all mutating API calls use `msalFetch` (Bearer token required)

### Error Handling
- `errorHandler` middleware strips internal error messages in `NODE_ENV=production`
- Manual catch blocks in `lab.ts` route handlers apply the same sanitisation pattern
- No stack traces or internal paths returned to clients in production

### Secrets Management
- **Local Development**: Single root `.env` file (never committed to git)
- **Production**: Azure Key Vault (recommended) or secure environment variables
- **Data Sources**: Connection info in metadata database (encrypted at rest by Fabric)
- **Template**: `.env.example` provided with placeholder values

---

## Monitoring & Logging

### Health Checks

**Endpoint**: `GET /api/health`

**Response**:
```json
{
  "status": "healthy",
  "timestamp": "2026-02-04T10:00:00Z",
  "services": {
    "database": "connected",
    "pythonProxy": "connected"
  }
}
```

### Logging

**Backend**: Console logging (structured JSON in production)
**Frontend**: Browser console (errors sent to backend)

---

## Testing Strategy

### Unit Tests
- Service layer functions
- Utility functions
- Pure components

### Integration Tests
- API endpoint testing
- Database operations
- Authentication flows

### E2E Tests (Future)
- User workflows
- Critical paths

---

## Future Enhancements

### Planned Architecture Changes
- **Redis Cache** - Replace in-memory cache with Redis for multi-instance support
- **Message Queue** - Add RabbitMQ/Kafka for async operations
- **WebSockets** - Real-time dashboard updates
- **GraphQL** - Alternative to REST for complex queries

### Scalability
- **Horizontal Scaling** - Multiple API instances behind load balancer
- **Database Replicas** - Read replicas for query-heavy workloads
- **CDN** - Static asset caching

---

## Appendix

### Key Files Reference

**Configuration**:
- `.env` - **Single root configuration file** (all services)
- `.env.example` - Template with placeholder values
- `start.ps1` - Automated startup script (Windows)

**Backend**:
- `apps/loomx-api/src/server.ts` - Express server entry point; applies `extractUser` globally
- `apps/loomx-api/src/middleware/authMiddleware.ts` - JWT decode; `extractUser` + `requireAuth`
- `apps/loomx-api/src/middleware/userContext.ts` - `getCurrentUserId(req)` — canonical identity helper
- `apps/loomx-api/src/middleware/errorHandler.ts` - Centralised error shaping + production sanitisation
- `apps/loomx-api/src/routes/*.ts` - API route handlers
- `apps/loomx-api/src/services/*.service.ts` - Business logic
- `apps/loomx-api/src/services/queryGenerator.service.ts` - SQL builder; exports `quoteIdentifier`, `SAFE_OPERATORS`
- `apps/loomx-api/src/db/connection.ts` - Database connection
- `apps/loomx-api/schema.sql` - Database schema (includes data_sources table)

**Frontend**:
- `apps/loomx-web/next.config.ts` - Next.js config (loads root .env)
- `apps/loomx-web/app/layout.tsx` - Root layout
- `apps/loomx-web/components/Layout.tsx` - App shell
- `apps/loomx-web/components/charts/ChartBuilderContext.tsx` - Shared chart state (React Context)
- `apps/loomx-web/components/charts/ChartHydrator.tsx` - **Shared hydrator** — populates context from API chart object; used on chart page and in every dashboard chart slot
- `apps/loomx-web/components/charts/ChartPreview.tsx` - ECharts visualisation consumer
- `apps/loomx-web/components/charts/CreateChartLayout.tsx` - Full builder UI (chart detail page only)
- `apps/loomx-web/auth/useAuth.tsx` - Authentication hook
- `apps/loomx-web/utils/msalFetch.ts` - Authenticated fetch wrapper (attaches Bearer token; CSRF mitigation)
- `apps/loomx-web/contexts/ThemeContext.tsx` - Theme provider

**Python Proxy**:
- `apps/loomx-python-proxy/proxy.py` - ODBC proxy server (loads root .env, queries data_sources table)

**Security**:
- `SECURITY.md` - Threat model, authentication architecture, known limitations, contributor checklist

---

---

**Document Version**: 2.0
**Last Updated**: 2026-02-20
**Maintained By**: LOOMX Development Team
