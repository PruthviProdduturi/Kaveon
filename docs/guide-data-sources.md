# Connecting Data Sources

A data source is a database connection that Kaveon can query. You need at least one data source before you can create datasets, build charts, or use the NL→SQL chat.

Navigate to **Data Sources** in the sidebar to manage your connections.

---

## Supported Databases

| Database | Type label | Driver | Protocol |
|----------|-----------|--------|---------|
| Microsoft Fabric SQL | `fabric_sql` | pyodbc, ODBC Driver 18 for SQL Server | TDS (SQL Server) |
| Azure SQL | `azure_sql` | pyodbc, ODBC Driver 18 for SQL Server | TDS (SQL Server) |
| PostgreSQL | `postgresql` | psycopg2 | PostgreSQL native |
| MySQL | `mysql` | pymysql | MySQL |
| StarRocks | `StarRocks` | pymysql | MySQL-compatible |
| Trino | coming soon | — | — |

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
   - **Description** — optional context for your team
3. Click **Test Connection** to verify connectivity.
4. Click **Save**.

### Connection string formats

**Microsoft Fabric SQL:**
```
<workspace>.database.fabric.microsoft.com
```
The Fabric SQL Analytics Endpoint hostname. No credentials in the string — authentication is via Azure AD Managed Identity (`DefaultAzureCredential`).

**Azure SQL:**
```
<server>.database.windows.net
```
The Azure SQL server hostname. Authentication is via Azure AD Managed Identity.

**PostgreSQL (including Neon, Supabase):**
```
postgresql://user:password@host:5432/database?sslmode=require
```
Standard PostgreSQL connection URL. SSL is required by default and enforced by psycopg2. Neon connection strings from the Neon console work as-is.

**MySQL:**
```
mysql://user:password@host:3306/database
```
Standard MySQL connection URL. SSL is always enabled.

**StarRocks:**
```
mysql://user:password@host:9030/database
```
StarRocks uses the MySQL wire protocol on port 9030 by default.

---

## Testing Connections

After entering connection details, click **Test Connection**. The backend:

1. Opens a fresh connection (not from the pool) to avoid polluting cached connections.
2. Runs `SELECT 1 AS connection_test` to verify basic connectivity.
3. Creates and drops a temporary table to verify write access:
   - Fabric/Azure SQL: `CREATE TABLE #_kaveon_probe (id INT)` then `DROP TABLE #_kaveon_probe`
   - PostgreSQL: `CREATE TEMP TABLE _kaveon_probe (id INT) ON COMMIT DROP`
   - MySQL: `CREATE TEMPORARY TABLE _kaveon_probe (id INT)` then `DROP TEMPORARY TABLE _kaveon_probe`

The test returns one of these structured outcomes:

| `error_type` | Meaning |
|-------------|---------|
| (none) | Connection and write access both succeeded |
| `access_denied` | Connected but Kaveon's identity lacks permission (login failed, CREATE TABLE denied) |
| `db_not_found` | Server is reachable but the database does not exist |
| `timeout` | Server did not respond within 60 seconds |
| `connection_failed` | Could not establish TCP connection — check hostname and firewall rules |

---

## Editing a Data Source

Click the three-dot menu on any data source card → **Edit**. All fields except the connection string are editable by Admins. To rotate credentials, edit the connection string field.

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
POST   /api/v1/data-sources/:id/favorite
DELETE /api/v1/data-sources/:id/favorite
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
| Test connection | Viewer |
| Set / remove favorite | Viewer (own favorite only) |

Roles are assigned in `AUTH_ADMIN_EMAILS` at deploy time. Emails not in that list receive the `Viewer` role.

---

## Security

The `connection_string` field (which may contain credentials for PostgreSQL/MySQL sources) is **never returned to the browser**. The list and detail endpoints omit it from all responses — only `id`, `name`, `type`, `database_name`, `region`, `description`, `created_by`, and timestamps are returned.

For Fabric SQL and Azure SQL, no credentials are stored at all. Authentication uses `DefaultAzureCredential` from the Azure Identity SDK, which resolves to Managed Identity in production (Render) and developer credentials locally.
