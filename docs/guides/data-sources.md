# Connecting Data Sources

A data source is a database connection that Kaveon can query. You need at least one data source before you can create datasets, build charts, or use the NL→SQL chat.

Navigate to **Data Sources** in the sidebar to manage your connections.

---

## Supported Databases

| Database | Type label (stored in DB) | Driver | Protocol |
|----------|--------------------------|--------|---------|
| Microsoft Fabric SQL (Analytics Endpoint) | `Fabric SQL AEP` | pyodbc, ODBC Driver 18 for SQL Server | TDS (SQL Server) |
| Microsoft Fabric SQL (Data Warehouse) | `Fabric SQL DW` | pyodbc, ODBC Driver 18 for SQL Server | TDS (SQL Server) |
| Azure SQL | `Azure SQL` | pyodbc, ODBC Driver 18 for SQL Server | TDS (SQL Server) |
| PostgreSQL | `PostgreSQL` | psycopg2 | PostgreSQL native |
| StarRocks | `StarRocks` | pymysql | MySQL-compatible (port 9030) |
| Trino | `Trino` | UI registration only — no backend driver yet | — |

> **MySQL / MariaDB**: supported in the backend connection pool (`pymysql`) and as a metadata DB type, but **not exposed in the data source UI**. There is no type selector option for MySQL in the frontend. To connect a MySQL source today, use the API directly with `type: "MySQL"` — the pool maps it to `pymysql` automatically.

---

## Adding a Data Source

**Requires Admin role.**

1. Go to **Data Sources → + Add Data Source**.
2. Fill in:
   - **Name** — a display name for this connection (must be unique)
   - **Type** — select from the supported types
   - **Database name** — the logical database within the server
   - **Connection string** — the endpoint or URL (see format per type below)
   - **Region** — `WW` (worldwide) or `EU` (Europe) — used for data residency tracking
   - **Description** — optional context or notes
3. Click **Save**. (The Test Connection button exists but is not yet wired to a live probe — see [Testing Connections](#testing-connections).)

### Connection string formats

**Fabric SQL AEP / Fabric SQL DW:**
```
<workspace>.datawarehouse.fabric.microsoft.com
```
The Fabric SQL endpoint hostname. No credentials in the string — authentication is via Azure AD Managed Identity (`DefaultAzureCredential`).

**Azure SQL:**
```
<server>.database.windows.net
```
The Azure SQL server hostname. Authentication is via Azure AD Managed Identity.

**PostgreSQL (including Neon, Supabase):**
```
postgresql://user:password@host:5432/database?sslmode=require
```
Standard PostgreSQL connection URL. SSL is required by default and enforced by psycopg2. Neon connection strings from the Neon console work as-is. The UI label for the connection field is "Host:Port".

**StarRocks:**
```
mysql://user:password@host:9030/database
```
StarRocks uses the MySQL wire protocol on port 9030 by default. The UI label for the connection field is "FE Host:Port".

**Trino:**
```
http://your-coordinator:8080
```
Trino appears in the UI type selector with a "Coordinator URL" field, but has no backend driver yet. Data sources can be registered for future use.

**MySQL (API-only):**
```
mysql://user:password@host:3306/database
```
Standard MySQL connection URL. Not available in the UI type selector — create via the API directly.

---

## Testing Connections

The data source test endpoint (`POST /api/v1/data-sources/{id}/test`) currently returns a stub response — **connection testing is not yet implemented** for data sources. The endpoint looks up the data source's `database_name` and `type` but does not open a live connection.

> **Setup wizard test**: The initial setup wizard (`POST /api/v1/setup/test`) uses a separate `probe_connection` function (`database/pool.py`) that does perform a real connectivity + write-access test:
>
> 1. Opens a fresh connection (not from the pool).
> 2. Runs `SELECT 1 AS connection_test`.
> 3. Creates and drops a temporary table to verify write access:
>    - Fabric/Azure SQL: `CREATE TABLE #_kaveon_probe (id INT)` then `DROP TABLE #_kaveon_probe`
>    - PostgreSQL: `CREATE TEMP TABLE _kaveon_probe (id INT) ON COMMIT DROP`
>    - MySQL: `CREATE TEMPORARY TABLE _kaveon_probe (id INT)` then `DROP TEMPORARY TABLE _kaveon_probe`
>
> On failure, it classifies the error as `access_denied`, `db_not_found`, `timeout`, or `connection_failed`.

---

## Editing a Data Source

Click the three-dot menu on any data source card → **Edit**. All fields are editable by Admins, including the connection string (for credential rotation or endpoint changes). Uses `PATCH /api/v1/data-sources/{id}`.

---

## Enabling and Disabling

Data sources can be enabled (`is_active = 1`) or disabled (`is_active = 0`). Disabled data sources are hidden from the data source selector in the chart builder and dataset creator, but their metadata is preserved. Re-enable them at any time.

---

## Favorites

Each user can mark one data source as their **favorite**. The favorite data source:

- Appears first in the data source selector
- Is pre-selected on the homepage for NL→SQL queries
- Is shown in the sidebar quick-access section

To set a favorite: open the data source card → click the star icon. Only one data source can be favorited per user — setting a new favorite removes the previous one.

API endpoints:

```
POST   /api/v1/data-sources/{id}/favorite
DELETE /api/v1/data-sources/{id}/favorite
GET    /api/v1/data-sources/favorite/current
```

---

## Schema Discovery

Kaveon discovers the table and column schema of a data source on demand — it does not crawl schemas on a schedule.

When you open a dataset or use SQL Lab, the backend queries `INFORMATION_SCHEMA` on the target database:

**Tables (all dialects):**
```sql
-- Fabric SQL / Azure SQL
SELECT TABLE_SCHEMA, TABLE_NAME
FROM INFORMATION_SCHEMA.TABLES
WHERE TABLE_TYPE = 'BASE TABLE'
ORDER BY TABLE_SCHEMA, TABLE_NAME

-- PostgreSQL
SELECT table_schema, table_name
FROM information_schema.tables
WHERE table_type = 'BASE TABLE'
  AND table_schema NOT IN ('pg_catalog', 'information_schema')

-- MySQL / StarRocks
SELECT table_schema, table_name
FROM information_schema.tables
WHERE table_type = 'BASE TABLE' AND table_schema = '<database>'
```

**Columns (all dialects):**
```sql
-- Fabric SQL / Azure SQL
SELECT COLUMN_NAME, DATA_TYPE, IS_NULLABLE, CHARACTER_MAXIMUM_LENGTH
FROM INFORMATION_SCHEMA.COLUMNS
WHERE TABLE_SCHEMA = ? AND TABLE_NAME = ?
ORDER BY ORDINAL_POSITION

-- PostgreSQL / MySQL equivalent (same structure, dialect-appropriate quoting)
```

Schema results are returned as:

```json
{
  "tables": [
    { "id": "dbo.orders", "schema": "dbo", "name": "orders", "fullName": "dbo.orders" }
  ]
}
```

Column results include `name`, `dataType`, `isNullable`, and `maxLength`.

---

## Connection Pooling

Each data source gets a per-process `ConnectionPool` in `database/pool.py`. Pool parameters:

| Setting | Metadata DB | Data source |
|---------|-------------|-------------|
| Pool size | Configurable via `MAX_POOL_SIZE_METADATA` | Configurable via `MAX_POOL_SIZE_DATAWAREHOUSE` |
| Wait timeout | 30 seconds | 30 seconds |
| Retry on stale socket | Yes, once | Yes, once |

Stale connections (detected by error patterns like `08S01`, `10054`, `ssl connection has been closed`, `lost connection`) are discarded and replaced rather than recycled.

Neon serverless Postgres in particular closes idle connections aggressively. The pool handles this with a retry-once strategy and by setting `autocommit=True` on all psycopg2 connections.

---

## Permissions

| Operation | Required role |
|-----------|--------------|
| List data sources | Viewer |
| View data source details | Viewer |
| Add data source | Admin |
| Edit data source | Admin |
| Delete data source | Admin |
| Test connection (stub) | Viewer |
| Set / remove favorite | Viewer (own favorite only) |

Roles are assigned in `AUTH_ADMIN_EMAILS` at deploy time. Emails not in that list receive the `Viewer` role.

---

## DLM Limitations by Database Type

The DLM (Data Language Model) — which compiles per-dataset context for zero-LLM question answering, precomputed dashboard serving, and value-index resolution — **requires PostgreSQL** for full functionality.

| Capability | PostgreSQL | Fabric SQL / Azure SQL / StarRocks / Trino |
|-----------|-----------|-------------------------------------------|
| DLM manifest (columns, joins, metrics, synonyms) | Full | Full |
| Context profiling (pg_stats value inventory) | Full | No-op (profiler `is_supported()` returns `False`) |
| Value index (entity resolution for NL queries) | Full | Skipped — no catalog stats to index |
| HLL sketch cuboids (approximate COUNT DISTINCT) | Full | Skipped — register SQL is PostgreSQL-specific |
| `serve-chart` from precomputed context | Full | Only serves if answers were precomputed at generate time; falls back to live query |
| DLM artifact status | `ready` | `unsupported` |

In practice: datasets on non-PostgreSQL data sources can still build a manifest and precompute metric totals/breakdowns, but lack the pg_stats-driven value inventory and HLL sketches. NL chat questions for those datasets route through the LLM path rather than the deterministic DLM resolver.

---

## Dataset Column Metadata

Each dataset exposes its column metadata via:

```
GET /api/v1/datasets/{id}/columns
```

Returns the dataset's column list (from `dataset_columns`), expanded with dimension metadata. Each column includes `table_name`, `column_name`, `data_type`, `is_dimension`, `is_metric`, and `semantic_type`.

---

## Security

The `connection_string` field (which may contain credentials for PostgreSQL/MySQL/StarRocks sources) is **never returned to the browser**. The list and detail endpoints select only `_PUBLIC_FIELDS` — `id`, `name`, `type`, `database_name`, `region`, `description`, `created_by`, `created_at`, `updated_at`, and `is_active`.

For Fabric SQL and Azure SQL, no credentials are stored at all. Authentication uses `DefaultAzureCredential` from the Azure Identity SDK, which resolves to Managed Identity in production (Azure Container Apps) and developer credentials locally.
