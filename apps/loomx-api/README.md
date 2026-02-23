# LOOMX API

**Live Operational Outcomes & Metrics eXperience**

Express.js backend API for LOOMX, providing REST endpoints for data visualization and exploration.

## Architecture

- **Framework:** Express.js + TypeScript
- **Database:** Microsoft Fabric SQL (via Python Proxy)
- **Authentication:** Azure AD (MSAL)
- **Caching:** Multi-tier (response cache, query cache, metadata cache)

## Prerequisites

- **Node.js 20+**
- **pnpm** (package manager)
- **Python Proxy** running on port 5001
- **Azure AD credentials** configured

## Setup

### 1. Install Dependencies

From the monorepo root:
```bash
pnpm install
```

Or from this directory:
```bash
cd apps/loomx-api
pnpm install
```

### 2. Configure Environment

**LOOMX uses a single root `.env` file for all services.**

From the repository root:
```bash
cp .env.example .env
```

Edit the root `.env` file with your values. All services (API, Web, Python Proxy) read from this single file.

**Important:** Data warehouse connections are managed in the metadata database's `data_sources` table, not in `.env`. Users configure these through the UI at `/data-sources`.

### 3. Start the Server

**Development mode** (with auto-reload):
```bash
pnpm dev
```

**Production mode:**
```bash
pnpm build
pnpm start
```

The API will start on **http://localhost:8080**

## Project Structure

```
apps/loomx-api/
├── src/
│   ├── adapters/        # Data format adapters
│   ├── db/              # Database connection
│   ├── middleware/      # Express middleware
│   ├── routes/          # API route handlers
│   ├── services/        # Business logic
│   └── server.ts        # Entry point
├── .env.example         # Environment template
└── package.json
```

## API Endpoints

### Health & Metadata
- `GET /api/v1/health` - Health check
- `GET /api/v1/metadata` - Application metadata

### Data Sources
- `GET /api/v1/data-sources/active` - List active data sources
- `POST /api/v1/data-sources` - Create data source
- `PUT /api/v1/data-sources/:id` - Update data source

### Datasets
- `GET /api/v1/datasets` - List all datasets
- `GET /api/v1/datasets/:id` - Get dataset details
- `POST /api/v1/datasets` - Create dataset
- `PUT /api/v1/datasets/:id` - Update dataset
- `DELETE /api/v1/datasets/:id` - Delete dataset

### Charts
- `GET /api/v1/charts` - List all charts
- `GET /api/v1/charts/:id` - Get chart details
- `POST /api/v1/charts` - Create chart
- `PUT /api/v1/charts/:id` - Update chart
- `DELETE /api/v1/charts/:id` - Delete chart

### Dashboards
- `GET /api/v1/dashboards` - List all dashboards
- `GET /api/v1/dashboards/:id` - Get dashboard details
- `POST /api/v1/dashboards` - Create dashboard
- `PUT /api/v1/dashboards/:id` - Update dashboard
- `DELETE /api/v1/dashboards/:id` - Delete dashboard

### SQL Lab
- `GET /api/v1/lab/tables` - List database tables
- `GET /api/v1/lab/schema/:schema/:table` - Get table schema
- `POST /api/v1/lab/query` - Execute SQL query
- `GET /api/v1/lab/saved-queries` - List saved queries
- `POST /api/v1/lab/saved-queries` - Save query

### SQL Execution
- `POST /api/v1/sql/execute` - Execute SQL query
- `GET /api/v1/sql/distinct-filter-values` - Get distinct values for filters

### Favorites
- `GET /api/v1/favorites` - List user favorites
- `POST /api/v1/favorites` - Add favorite
- `DELETE /api/v1/favorites/:id` - Remove favorite

### Theme
- `GET /api/v1/theme` - Get user theme
- `POST /api/v1/theme` - Save user theme

### Cache Management
- `POST /api/v1/cache/clear` - Clear all caches

## Development

### Build
```bash
pnpm build
```

### Type Check
```bash
pnpm type-check
```

### Clean Build Artifacts
```bash
pnpm clean
```

## Dependencies

### Core
- `express` - Web framework
- `typescript` - Type safety
- `axios` - HTTP client for Python proxy
- `tedious` - SQL Server driver (fallback)

### Azure
- `@azure/identity` - Azure AD authentication
- `@azure/msal-node` - MSAL for Node.js

### Utilities
- `cors` - CORS middleware
- `helmet` - Security headers
- `dotenv` - Environment variables
- `node-cache` - In-memory caching
- `uuid` - UUID generation

## Environment Variables

| Variable | Description | Required |
|----------|-------------|----------|
| `NODE_ENV` | Environment (development/production) | Yes |
| `PORT` | API server port | Yes |
| `FABRIC_METADATA_ENDPOINT` | Metadata database endpoint | Yes |
| `FABRIC_METADATA_DATABASE` | Metadata database name | Yes |
| `PYTHON_PROXY_URL` | Python proxy URL | Yes |
| `AZURE_TENANT_ID` | Azure AD tenant ID | Yes |
| `AZURE_CLIENT_ID` | Azure AD client/app ID | Yes |
| `WEB_URL` | Frontend URL for CORS | Yes |

## Notes

- **Data Sources:** Configure multiple warehouses/lakehouses via the UI, not environment variables
- **Python Proxy:** Required for Fabric SQL connectivity - must be running on port 5001
- **Caching:** Multi-tier caching improves performance (response, query, metadata)
- **Authentication:** Uses Azure AD with MSAL for secure token-based auth
