# LOOMX

**Live Operational Outcomes & Metrics eXperience**

> A modern, fast, and beautiful data exploration platform for Microsoft Fabric with enterprise-grade performance and security.

## ✨ Key Features

### 🎨 Modern User Experience
- **Clean Brand Identity** - Modern LOOMX logo with theme-aware gradients
- **Customizable Themes** - Per-user color themes with real-time updates
- **Smooth Animations** - GPU-accelerated animations and transitions
- **Responsive Design** - Works seamlessly across all devices

### ⚡ High Performance
- **Multi-level Caching** - Query cache, metadata cache, response cache
- **Connection Pooling** - Optimized database connections
- **Parallel Execution** - Execute multiple queries simultaneously
- **Smart Cache Management** - LRU cache with TTL support
- **10-100x faster** on cached operations

### 🔒 Enterprise Security
- **Azure AD Authentication** - JWT Bearer token validation on every API request via `authMiddleware`
- **JWT-first Identity** - User identity sourced from token claims, never from client-controlled headers
- **SQL Injection Protection** - Identifier quoting (`quoteIdentifier`) + operator allowlist on all generated SQL
- **CSRF Mitigation** - All mutating API calls authenticated with Bearer token via `msalFetch`
- **Role-Based Access** - User-scoped data and permissions
- See [`SECURITY.md`](./SECURITY.md) for the full threat model, known limitations, and contributor checklist

### 📊 Rich Visualization
- **ECharts Integration** - 20+ chart types with advanced customization
- **Interactive Dashboards** - Drag-and-drop dashboard builder
- **Real-time Updates** - Live data refresh and filtering
- **Advanced Options** - Full control over chart appearance and behavior

### 🏗️ Clean Architecture
- **Pure TypeScript** - End-to-end type safety
- **Layered Design** - Clear separation: Routes → Services → Adapters → Database
- **Shared Chart Hydrator** - Single `ChartHydrator` component drives both the chart builder page and every dashboard chart slot — zero duplicated rendering logic
- **JWT-first Identity** - `authMiddleware` + `userContext` provide a canonical identity chain across all API routes
- **Production Ready** - Health checks, error handling, centralised error sanitisation, structured logging

---

## 🚀 Quick Start

### Prerequisites

- **Node.js** 20+
- **pnpm** 9+
- **Python** 3.8+ (for ODBC proxy)
- Access to **Microsoft Fabric SQL** endpoint

### Installation

```bash
# Clone the repository
git clone <repo-url>
cd LOOMX

# Install dependencies
pnpm install

# Set up environment file (single root .env for all services)
cp .env.example .env

# Edit .env with your credentials:
# - Azure AD tenant and client IDs
# - Fabric metadata database endpoint (stores LOOMX data)
# Note: Data warehouse endpoints are configured via UI at /data-sources

# Create database schema
# Open apps/loomx-api/schema.sql in Azure Data Studio
# Connect to your metadata database and execute the script

# Start all services
pnpm dev
```

### Access the Application

- **Web UI**: http://localhost:3000
- **API**: http://localhost:8080
- **Python Proxy**: http://localhost:5001

---

## 📁 Project Structure

```
LOOMX/
├── apps/
│   ├── loomx-api/                    # Express.js API Backend
│   │   ├── src/
│   │   │   ├── adapters/             # FabricExplorer compatibility layer
│   │   │   ├── db/                   # Database connection (tedious)
│   │   │   ├── middleware/           # Express middleware (auth, cache, errors)
│   │   │   ├── routes/               # API route handlers
│   │   │   ├── services/             # Business logic layer
│   │   │   └── server.ts             # Express server entry point
│   │   ├── schema.sql                # Production database schema
│   │   └── package.json
│   │
│   ├── loomx-web/                    # Next.js 15 Frontend
│   │   ├── app/                      # App Router pages & layouts
│   │   │   ├── charts/               # Chart management pages
│   │   │   ├── dashboards/           # Dashboard pages
│   │   │   ├── datasets/             # Dataset configuration
│   │   │   ├── lab/                  # SQL Lab
│   │   │   ├── icon.tsx              # Dynamic favicon
│   │   │   └── layout.tsx            # Root layout
│   │   ├── auth/                     # MSAL authentication
│   │   ├── components/               # React components
│   │   │   ├── charts/               # Chart builder components
│   │   │   ├── dashboards/           # Dashboard components
│   │   │   └── Layout.tsx            # Main layout
│   │   ├── contexts/                 # React contexts (Theme, etc.)
│   │   ├── styles/                   # Global CSS
│   │   └── utils/                    # Utility functions
│   │
│   └── loomx-python-proxy/           # Python ODBC Proxy (Flask)
│       ├── proxy.py                  # Connection pooling & query execution
│       └── requirements.txt
│
└── packages/
    └── types/                        # Shared TypeScript types
        └── src/index.ts
```

---

## 🔧 Configuration

### API Configuration

**File**: `apps/loomx-api/.env`

```env
# Server
NODE_ENV=development
PORT=8080

# Metadata Database (stores LOOMX app data)
FABRIC_METADATA_ENDPOINT=your-metadata-endpoint.database.fabric.microsoft.com
FABRIC_METADATA_DATABASE=YourMetadataDatabase

# Python Proxy (for ODBC connections)
PYTHON_PROXY_URL=http://localhost:5001
PYTHON_PROXY_TIMEOUT_MS=120000   # Max ms to wait for a proxy query (default: 120 s)

# Azure AD Authentication
AZURE_TENANT_ID=your-tenant-id
AZURE_CLIENT_ID=your-client-id

# CORS
WEB_URL=http://localhost:3000
```

### Web Configuration

**File**: `apps/loomx-web/.env.local`

```env
# API Endpoint
NEXT_PUBLIC_API_URL=http://localhost:8080

# Azure AD Authentication
NEXT_PUBLIC_AZURE_CLIENT_ID=your-client-id
NEXT_PUBLIC_AZURE_TENANT_ID=your-tenant-id
NEXT_PUBLIC_AZURE_REDIRECT_URI=http://localhost:3000
```

### Python Proxy Configuration

The Python proxy uses environment variables or falls back to the metadata database configuration. Connection pooling is automatic with a maximum of 10 connections per data source.

---

## 📊 Database Schema

The application uses Microsoft Fabric SQL for metadata storage:

### Core Tables

- **`datasets`** - Semantic layer definitions with dimensions, metrics, and filters
- **`dataset_dimensions`** - Dimension table relationships
- **`dataset_columns`** - Column metadata and semantic types
- **`charts`** - Chart configurations (query_config, viz_config)
- **`dashboards`** - Dashboard layouts and filters
- **`saved_queries`** - User-saved SQL queries
- **`query_history`** - Query execution history
- **`favorites`** - User favorites (supports multiple object types)
- **`activity`** - Audit log for user actions
- **`data_sources`** - Dynamic data source connections
- **`user_themes`** - Per-user theme color preferences

See `apps/loomx-api/schema.sql` for the complete schema with all columns and constraints.

---

## 🛠️ Tech Stack

### Backend (loomx-api)
- **Express.js** - Web framework
- **TypeScript** - Type safety
- **tedious** - Native SQL Server driver
- **@azure/identity** - Azure AD authentication
- **node-cache** - In-memory caching

### Frontend (loomx-web)
- **Next.js 15** - React framework with App Router
- **React 19** - UI library
- **TypeScript** - Type safety
- **@azure/msal-browser** - Azure AD authentication
- **ECharts** - Data visualization
- **Monaco Editor** - SQL editor
- **Font Awesome** - Icons

### Python Proxy (loomx-python-proxy)
- **Flask** - Web framework
- **pyodbc** - ODBC driver for SQL Server
- **Connection pooling** - Reusable database connections

---

## 📚 API Endpoints

### Authentication & Health
- `GET /api/health` - Health check
- `POST /api/connect` - Establish session
- `POST /api/disconnect` - Close session

### Datasets
- `GET /api/v1/datasets/summary` - List datasets with favorites
- `GET /api/v1/datasets/:id` - Get dataset with full schema
- `POST /api/v1/datasets` - Create dataset
- `PUT /api/v1/datasets/:id` - Update dataset
- `DELETE /api/v1/datasets/:id` - Delete dataset

### Charts
- `GET /api/v1/charts/summary` - List charts with favorites
- `GET /api/v1/charts/:id` - Get chart configuration
- `POST /api/v1/charts` - Create chart
- `PUT /api/v1/charts/:id` - Update chart
- `PUT /api/v1/charts/:id/favorite` - Toggle favorite
- `DELETE /api/v1/charts/:id` - Delete chart

### Dashboards
- `GET /api/v1/dashboards/summary` - List dashboards
- `GET /api/v1/dashboards/:id` - Get dashboard
- `POST /api/v1/dashboards` - Create dashboard
- `PUT /api/v1/dashboards/:id` - Update dashboard
- `POST /api/v1/dashboards/:id/favorite` - Toggle favorite
- `DELETE /api/v1/dashboards/:id` - Delete dashboard

### SQL Lab
- `POST /api/v1/sql/execute` - Execute SQL query
- `GET /api/v1/sql/history` - Query history
- `GET /api/v1/sql/saved` - Saved queries
- `POST /api/v1/sql/save` - Save query

### Metadata
- `GET /api/v1/metadata/tables` - List available tables
- `GET /api/v1/metadata/tables/:tableName/preview` - Preview table data

### Theme
- `GET /api/v1/theme` - Get user theme
- `PUT /api/v1/theme` - Update user theme

### Favorites
- `GET /api/v1/favorites` - Get all user favorites
- `POST /api/v1/favorites` - Add favorite
- `DELETE /api/v1/favorites/:id` - Remove favorite

---

## 🔍 Architecture Highlights

### Layered Architecture
1. **Routes** - HTTP request handling, validation
2. **Services** - Business logic, orchestration
3. **Adapters** - FabricExplorer compatibility, data transformation
4. **Database** - Connection pooling, query execution

### Caching Strategy
- **Query Cache** - SQL query results (5 min TTL)
- **Metadata Cache** - Table schemas, column metadata (10 min TTL)
- **Response Cache** - Full HTTP responses for reads (1 min TTL)

### Authentication & Identity Flow
1. User signs in with Azure AD via MSAL
2. Frontend obtains JWT access token
3. Token passed in `Authorization: Bearer` header
4. `extractUser` middleware decodes JWT payload, validates `exp` + `iss`, sets `req.user.email`
5. All route handlers call `getCurrentUserId(req)` — JWT-first, header fallback, `'anonymous'` default

### Chart Rendering Pattern
Charts share a single hydration path regardless of context:
- **`ChartBuilderProvider`** — React Context holding all chart state
- **`ChartHydrator`** — populates context from a raw API chart object; renders `null`
- **`CreateChartLayout`** — full builder UI (chart detail page only)
- **`ChartPreview`** — ECharts visualisation (used inside dashboard chart slots)

Both the chart detail page and every slot in a dashboard render the same `ChartHydrator → ChartPreview` stack. Dashboard-level filters are passed as `externalFilters` to `ChartHydrator`.

### Data Source Management
- Dynamic data sources configured via UI
- Supports multiple warehouses and lakehouses
- Connection details stored in `data_sources` table
- No hardcoded connection strings

---

## 🚀 Development

### Run Individual Services

```bash
# API only
cd apps/loomx-api
pnpm dev

# Web only
cd apps/loomx-web
pnpm dev

# Python proxy only
cd apps/loomx-python-proxy
python proxy.py
```

### Run All Services

```bash
# From root directory
pnpm dev
```

### Type Checking

```bash
pnpm check-types
```

### Build for Production

```bash
pnpm build
```

### Clean Build Artifacts

```bash
pnpm clean
```

---

## 🐛 Troubleshooting

### Connection Issues
- Verify `.env` files have correct Fabric endpoints
- Check Azure AD credentials are valid
- Ensure firewall allows connections to Fabric SQL

### Authentication Errors
- Clear browser cache and localStorage
- Verify Azure AD app registration settings
- Check redirect URI matches exactly

### Build Errors
- Delete `node_modules` and `.next` folders
- Run `pnpm install` again
- Check for TypeScript errors with `pnpm check-types`

### Database Errors
- Verify schema has been applied (`schema.sql`)
- Check database permissions
- Review Python proxy logs for ODBC errors

### Slow Queries / Timeout Errors
- Fabric Delta Lake scans can be slow on first access (cold start)
- Increase the proxy timeout in root `.env`: `PYTHON_PROXY_TIMEOUT_MS=180000`
- Default is 120 s; set higher for very large tables or cold warehouse instances

---

## 📖 Documentation

- **[ARCHITECTURE.md](./ARCHITECTURE.md)** - Detailed architecture documentation (v2.0)
- **[SECURITY.md](./SECURITY.md)** - Threat model, auth architecture, known limitations, contributor checklist
- **apps/loomx-api/schema.sql** - Complete database schema
- **IMPLEMENTATION_SUMMARY.md** - Implementation notes
- **QUICK_START_V2.md** - Quick start guide

---

## 🎯 What's Next

### Planned Features
- Enhanced dashboard filters and interactivity
- More chart types and visualization options
- Query optimization suggestions
- Collaboration features (sharing, comments)
- Advanced access controls
- Export to PowerPoint/PDF

---

## 📄 License

Proprietary - Microsoft Internal Use Only

---

**Built with ❤️ using modern web technologies and best practices**
