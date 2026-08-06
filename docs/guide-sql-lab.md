# SQL Lab

SQL Lab is a full-featured query editor for exploring your data sources directly. Navigate to **Lab** in the sidebar.

---

## Editor

The editor is [Monaco Editor](https://microsoft.github.io/monaco-editor/) — the same editor that powers VS Code.

### Keyboard shortcuts

| Action | Shortcut |
|--------|----------|
| Run query | `Ctrl+Enter` / `Cmd+Enter` |
| Format SQL | `Shift+Alt+F` |
| Comment line | `Ctrl+/` / `Cmd+/` |
| Find | `Ctrl+F` / `Cmd+F` |
| Go to line | `Ctrl+G` / `Cmd+G` |

### Autocomplete

Monaco provides autocomplete for SQL keywords. Column and table names are populated from the **schema browser** on the left — select a database first to load its tables, then expand a table to load its columns.

---

## Multi-Tab Support

Each query runs in its own tab. Open additional tabs with the `+` button in the tab bar. Tabs persist their query text during the session.

Tabs are not persisted across page reloads — use **Save Query** to keep a query across sessions.

---

## Selecting a Database

The database selector in the toolbar shows all active data sources. Select a database before running a query. The schema browser updates to show tables from that database.

If no database is selected, the editor defaults to the metadata database (where demo datasets live). This is intentional — you can always run a query to explore the platform's own metadata.

---

## Running Queries

### Synchronous execution (`POST /api/v1/lab/execute`)

Used for simple, short queries where result delivery is expected within the HTTP timeout. Returns columns, rows, and row count.

### Cancellable async execution (`POST /api/v1/lab/query`)

Used by the primary Run button. The backend wraps the query in an executor thread and polls for client disconnect every 500 ms. If the browser tab is closed or the user navigates away, the backend sends a cancel signal to the database cursor (`cursor.cancel()`) and returns `204 No Content`.

This means long-running queries do not keep accumulating on the database server when the user abandons them.

### Execution response

```json
{
  "success": true,
  "columns": ["order_id", "total", "region"],
  "rows": [[1, 1234.50, "West"], [2, 890.00, "East"]],
  "rowCount": 2,
  "executionTime": 0.412
}
```

`executionTime` is end-to-end duration in seconds as measured by the API (SQL generation + network + database). For Fabric SQL, the database-side time is measured separately.

---

## Result Panel

Results are displayed below the editor in a scrollable table. Columns are sortable by clicking their headers. Numeric columns right-align automatically.

### Export

The result panel provides a **Download CSV** button. The export is generated client-side from the in-memory result rows (no additional server request).

### Truncation

The API returns up to the row count the database sends — there is no server-side row limit beyond what the SQL itself specifies. For safety, use `LIMIT` / `TOP` in your queries. Very large result sets (> 10,000 rows) will slow the browser table render.

---

## Query History

Every query executed through the Lab is recorded in query history. Access it via the **History** panel in the Lab sidebar.

History records include:
- SQL text
- Execution time (ms)
- Row count
- Status (`success` / `error`)
- Error message (on failure)
- Timestamp
- Trigger source (`lab`, `dataset-preview`, `dataset-filter-values`)

History is shared across users by default (admins can see all queries; analysts see their own). To clear your history, click **Clear History** in the History panel.

### API endpoints

```
GET  /api/v1/lab/query-history?limit=50
DELETE /api/v1/lab/query-history
```

---

## Saved Queries

Save any query with **Save Query** (Ctrl+S). Saved queries are personal — each user has their own collection.

Operations:

| Operation | Endpoint |
|-----------|---------|
| List | `GET /api/v1/lab/saved-queries` |
| Get by id | `GET /api/v1/lab/saved-queries/:id` |
| Create | `POST /api/v1/lab/saved-queries` |
| Update | `PUT /api/v1/lab/saved-queries/:id` |
| Delete | `DELETE /api/v1/lab/saved-queries/:id` |

Saved query payload:

```json
{
  "name": "Monthly revenue by region",
  "sql": "SELECT region, SUM(revenue) ...",
  "database": "sales_db",
  "description": "Used for the Q4 review deck"
}
```

---

## Rate Limiting

SQL execution is rate-limited per user: **120 queries per 60-second window**.

If the limit is exceeded, the API returns:

```json
HTTP 429
{
  "detail": "Rate limit exceeded — maximum 120 requests per 60s."
}
```

The rate limiter uses a sliding window. In production with `REDIS_URL` set, the window is enforced across all API workers. Without Redis, it is in-process per worker (suitable for single-worker deployments).

---

## Create Table as Select (CTAS)

From the result panel, click **Save as Table** to materialise the query result as a new table in the database. This uses:

```sql
SELECT * INTO [schema].[table_name]
FROM (
  <your query>
) AS __ctas_source
```

CTAS requires `Analyst` role and is subject to the same rate limit as regular queries. The target schema and table name are set in a modal before execution.

---

## Supported Databases

| Database | Driver | Auth method |
|----------|--------|-------------|
| Microsoft Fabric SQL | pyodbc + ODBC Driver 18 for SQL Server | Azure AD token (Managed Identity / `DefaultAzureCredential`) |
| Azure SQL | pyodbc + ODBC Driver 18 for SQL Server | Azure AD token |
| PostgreSQL (including Neon) | psycopg2 | Username/password or Azure AD Managed Identity |
| MySQL | pymysql | Username/password or Azure AD Managed Identity |
| StarRocks | pymysql (MySQL-compatible protocol) | Username/password |

### Fabric SQL / Azure SQL connection

Authentication uses `DefaultAzureCredential` from the `azure-identity` package. The credential is a singleton shared across all pool connections to avoid token-acquisition storms. Token bytes are packed into the ODBC `SQL_COPT_SS_ACCESS_TOKEN` connection attribute.

Connection string format (stored in `data_sources.connection_string`):
```
<server>.database.fabric.microsoft.com   (Fabric SQL endpoint)
<server>.database.windows.net            (Azure SQL endpoint)
```

### PostgreSQL connection

psycopg2 connects with `sslmode=require` by default. Neon serverless connections are handled specially — `autocommit=True` is set on every psycopg2 connection to avoid the "current transaction is aborted" error that occurs when a failed statement poisons a pooled connection on serverless Postgres.

Connection URL format:
```
postgresql://user:password@host:5432/database?sslmode=require
```

### MySQL connection

pymysql connects with `ssl={"ssl": {}}` and `autocommit=True`. StarRocks uses the same driver since it speaks the MySQL wire protocol.

Connection URL format:
```
mysql://user:password@host:3306/database
```

---

## Schema Browser

The schema browser on the left panel lists all tables for the selected database. Click a table to expand it and see columns with their data types and nullability.

The backend endpoints that power it:

```
GET /api/v1/lab/tables?database=<db>
GET /api/v1/lab/tables/:table_id/columns?database=<db>
GET /api/v1/lab/schema/:schema/:table?database=<db>
```

Distinct column values (used by filter pickers) are fetched via:

```
GET /api/v1/lab/distinct/:schema/:table/:column?database=<db>&limit=100
```
