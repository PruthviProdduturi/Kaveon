"""
Weft database connection pool.

Direct port of the Flask proxy's FabricSQLConnection + ConnectionPool classes.
All DB calls now live in-process — no HTTP hop to a sidecar.

Key design decisions preserved from the proxy:
  - Single shared DefaultAzureCredential (token cache is shared, no auth storms)
  - Queue-based thread-safe pool (FastAPI sync handlers run in threadpool)
  - Retry-once on 08S01 / 10054 (stale pooled socket, not a real failure)
  - Large integer -> string conversion (JavaScript MAX_SAFE_INTEGER boundary)
  - datetime -> ISO string (no timezone conversion, matches SQL Server storage)
"""

import os
import struct
import json
import time
import pyodbc
from datetime import datetime, date
from queue import Queue, Empty
from threading import Lock
from typing import Any, Dict, List, Optional

from azure.identity import DefaultAzureCredential
from fastapi.responses import JSONResponse

from config import settings

# ── Shared Azure credential singleton ─────────────────────────────────────────
# One instance shared across ALL pool connections.
# DefaultAzureCredential caches tokens internally — sharing it prevents
# multiple concurrent auth requests on a cold-pool connection storm.
_azure_credential: DefaultAzureCredential = DefaultAzureCredential()

class TokenAuthError(Exception):
    """Raised when Azure AD token acquisition fails — signals a 401 to callers."""


SQL_COPT_SS_ACCESS_TOKEN = 1256
TOKEN_URL = "https://database.windows.net/.default"
OSSRDBMS_TOKEN_URL = "https://ossrdbms-aad.database.windows.net/.default"

# JavaScript MAX_SAFE_INTEGER = 2^53 − 1
_MAX_SAFE_INTEGER = 9_007_199_254_740_991


def _safe(value: Any) -> Any:
    """Coerce large ints and datetimes to JSON-safe types."""
    if isinstance(value, int) and abs(value) > _MAX_SAFE_INTEGER:
        return str(value)
    if isinstance(value, (datetime, date)):
        return value.isoformat()
    return value


def _get_ossrdbms_token() -> tuple:
    """
    Obtain an Azure AD access token for Azure Database for PostgreSQL / MySQL.
    Returns (token_str, username) where username is the AAD identity's OID —
    no credentials are stored; authentication is entirely via Managed Identity.
    """
    import base64 as _b64
    import json as _json

    token_response = _azure_credential.get_token(OSSRDBMS_TOKEN_URL)
    token = token_response.token

    # Decode JWT payload (no signature verification needed — we just issued it)
    parts = token.split(".")
    if len(parts) >= 2:
        padding = 4 - len(parts[1]) % 4
        try:
            payload = _json.loads(_b64.urlsafe_b64decode(parts[1] + "=" * padding))
            # UPN for user identities; OID for managed / service-principal identities
            username = (
                payload.get("upn")
                or payload.get("preferred_username")
                or payload.get("unique_name")
                or payload.get("oid")
                or ""
            )
        except Exception:
            username = ""
    else:
        username = ""

    return token, username


# ── JSON helpers ──────────────────────────────────────────────────────────────

def _serialize(obj: Any) -> Any:
    """Custom JSON serializer: datetime -> ISO string, large int -> string."""
    if isinstance(obj, (datetime, date)):
        return obj.isoformat()
    if isinstance(obj, int) and abs(obj) > _MAX_SAFE_INTEGER:
        return str(obj)
    raise TypeError(f"Object of type {type(obj)} is not JSON serializable")


class LargeIntResponse(JSONResponse):
    """
    FastAPI response class that:
      - Serialises datetime objects as ISO strings (no TZ conversion)
      - Converts integers outside JS Number.MAX_SAFE_INTEGER to strings
    Set as the app's default_response_class in main.py.
    """

    def render(self, content: Any) -> bytes:
        return json.dumps(content, default=_serialize, ensure_ascii=False).encode("utf-8")


# ── Helpers ───────────────────────────────────────────────────────────────────

def mask_endpoint(endpoint: str) -> str:
    if not endpoint:
        return "(none)"
    parts = endpoint.split("-", 1)
    if len(parts) == 2:
        return f"***-{parts[1]}"
    if "." in endpoint:
        return f"***.{endpoint.split('.', 1)[1]}"
    return "***"


def is_connection_error(e: Exception) -> bool:
    """True for transient TCP/ODBC errors on a stale pooled socket."""
    msg = str(e).lower()
    return "08s01" in msg or "10054" in msg or "communication link failure" in msg


# ── FabricSQLConnection ───────────────────────────────────────────────────────

class FabricSQLConnection:
    """Single pyodbc connection to a Fabric SQL / Azure SQL endpoint."""

    def __init__(self, server: str, database: str):
        self.server = server
        self.database = database
        self.connection: Optional[pyodbc.Connection] = None
        self.in_use: bool = False
        self.last_used: float = time.time()

    def _try_token_auth(self) -> bool:
        try:
            token_response = _azure_credential.get_token(TOKEN_URL)
            token_bytes = token_response.token.encode("UTF-16-LE")
            token_struct = struct.pack(f"<I{len(token_bytes)}s", len(token_bytes), token_bytes)

            trust_cert = "yes" if settings.SQL_TRUST_SERVER_CERT else "no"
            conn_str = (
                "DRIVER={ODBC Driver 18 for SQL Server};"
                f"SERVER={self.server};"
                f"DATABASE={self.database};"
                "Encrypt=yes;"
                f"TrustServerCertificate={trust_cert};"
                "Connection Timeout=60;"
            )
            self.connection = pyodbc.connect(
                conn_str, attrs_before={SQL_COPT_SS_ACCESS_TOKEN: token_struct}
            )
            cursor = self.connection.cursor()
            cursor.execute("SELECT 1")
            cursor.fetchone()
            cursor.close()
            print(f"[Pool] Connected -> {self.database}")
            return True
        except Exception as e:
            print(f"[Pool] Token auth failed: {e}")
            return False

    def connect(self):
        if self.connection:
            return
        if not self._try_token_auth():
            raise TokenAuthError(
                f"Failed to connect to {self.server}/{self.database}: "
                "Azure AD token authentication failed"
            )

    def execute_query(self, sql: str, params: Optional[list] = None) -> Dict[str, Any]:
        """Execute SQL and return {columns, rows, rows_objects, row_count}."""
        self.connect()
        cursor = self.connection.cursor()
        if params:
            cursor.execute(sql, params)
        else:
            cursor.execute(sql)

        sql_upper = sql.strip().upper()
        is_modification = sql_upper.startswith(("INSERT", "UPDATE", "DELETE", "MERGE"))

        if cursor.description:
            columns = [col[0] for col in cursor.description]
            rows_arrays: List[list] = []
            rows_objects: List[dict] = []

            for row in cursor.fetchall():
                safe_row = [_safe(v) for v in row]
                rows_arrays.append(safe_row)
                rows_objects.append({columns[i]: safe_row[i] for i in range(len(columns))})

            cursor.close()
            if is_modification:
                self.connection.commit()

            return {
                "columns": columns,
                "rows": rows_arrays,
                "rows_objects": rows_objects,
                "row_count": len(rows_arrays),
            }
        else:
            row_count = cursor.rowcount
            cursor.close()
            self.connection.commit()
            return {"columns": [], "rows": [], "rows_objects": [], "row_count": row_count}

    def execute_query_cancellable(
        self, sql: str, params: Optional[list] = None, cancel_event=None
    ) -> Dict[str, Any]:
        """Like execute_query but watches *cancel_event* (threading.Event).
        When the event is set the pyodbc cursor is cancelled mid-flight."""
        import threading as _threading

        self.connect()
        cursor = self.connection.cursor()

        if cancel_event is not None:
            def _watcher():
                cancel_event.wait()
                try:
                    cursor.cancel()
                except Exception:
                    pass
            _threading.Thread(target=_watcher, daemon=True).start()

        try:
            if params:
                cursor.execute(sql, params)
            else:
                cursor.execute(sql)

            sql_upper = sql.strip().upper()
            is_modification = sql_upper.startswith(("INSERT", "UPDATE", "DELETE", "MERGE"))

            if cursor.description:
                columns = [col[0] for col in cursor.description]
                rows_arrays: List[list] = []
                rows_objects: List[dict] = []
                for row in cursor.fetchall():
                    safe_row = [_safe(v) for v in row]
                    rows_arrays.append(safe_row)
                    rows_objects.append({columns[i]: safe_row[i] for i in range(len(columns))})
                cursor.close()
                if is_modification:
                    self.connection.commit()
                return {"columns": columns, "rows": rows_arrays, "rows_objects": rows_objects, "row_count": len(rows_arrays)}
            else:
                row_count = cursor.rowcount
                cursor.close()
                self.connection.commit()
                return {"columns": [], "rows": [], "rows_objects": [], "row_count": row_count}
        finally:
            if cancel_event is not None:
                cancel_event.set()

    def get_tables(self) -> List[Dict[str, str]]:
        sql = """
            SELECT TABLE_SCHEMA as [schema], TABLE_NAME as [name]
            FROM INFORMATION_SCHEMA.TABLES
            WHERE TABLE_TYPE = 'BASE TABLE'
            ORDER BY TABLE_SCHEMA, TABLE_NAME
        """
        result = self.execute_query(sql)
        return result["rows_objects"]

    def get_table_columns(self, schema: str, table_name: str) -> List[Dict[str, Any]]:
        sql = """
            SELECT
                COLUMN_NAME as [name],
                DATA_TYPE as [dataType],
                IS_NULLABLE as [isNullable],
                CHARACTER_MAXIMUM_LENGTH as [maxLength]
            FROM INFORMATION_SCHEMA.COLUMNS
            WHERE TABLE_SCHEMA = ?
                AND TABLE_NAME = ?
            ORDER BY ORDINAL_POSITION
        """
        result = self.execute_query(sql, [schema, table_name])
        return result["rows_objects"]

    def close(self):
        if self.connection:
            try:
                self.connection.close()
            except Exception:
                pass
            self.connection = None


# ── PostgreSQLConnection ──────────────────────────────────────────────────────

class PostgreSQLConnection:
    """Single psycopg2 connection to Azure Database for PostgreSQL via Managed Identity."""

    def __init__(self, host: str, port: int, database: str):
        self.host = host
        self.port = port or 5432
        self.database = database
        self.connection = None
        self.in_use: bool = False
        self.last_used: float = time.time()

    def connect(self):
        if self.connection:
            return
        import psycopg2
        token, username = _get_ossrdbms_token()
        self.connection = psycopg2.connect(
            host=self.host, port=self.port, dbname=self.database,
            user=username, password=token,
            sslmode="require", connect_timeout=30,
        )
        self.connection.autocommit = False
        print(f"[Pool] Connected (PostgreSQL) -> {self.database}")

    def execute_query(self, sql: str, params: Optional[list] = None) -> Dict[str, Any]:
        self.connect()
        cursor = self.connection.cursor()
        cursor.execute(sql, params or [])
        sql_upper = sql.strip().upper()
        is_modification = sql_upper.startswith(("INSERT", "UPDATE", "DELETE", "MERGE"))

        if cursor.description:
            columns = [col[0] for col in cursor.description]
            rows_arrays: List[list] = []
            rows_objects: List[dict] = []
            for row in cursor.fetchall():
                safe_row = [_safe(v) for v in row]
                rows_arrays.append(safe_row)
                rows_objects.append({columns[i]: safe_row[i] for i in range(len(columns))})
            cursor.close()
            if is_modification:
                self.connection.commit()
            return {"columns": columns, "rows": rows_arrays, "rows_objects": rows_objects, "row_count": len(rows_arrays)}
        else:
            row_count = cursor.rowcount
            cursor.close()
            self.connection.commit()
            return {"columns": [], "rows": [], "rows_objects": [], "row_count": row_count}

    def get_tables(self) -> List[Dict[str, str]]:
        result = self.execute_query("""
            SELECT table_schema AS "schema", table_name AS "name"
            FROM information_schema.tables
            WHERE table_type = 'BASE TABLE'
              AND table_schema NOT IN ('pg_catalog', 'information_schema')
            ORDER BY table_schema, table_name
        """)
        return result["rows_objects"]

    def get_table_columns(self, schema: str, table_name: str) -> List[Dict[str, Any]]:
        result = self.execute_query("""
            SELECT column_name AS "name", data_type AS "dataType",
                   is_nullable AS "isNullable", character_maximum_length AS "maxLength"
            FROM information_schema.columns
            WHERE table_schema = %s AND table_name = %s
            ORDER BY ordinal_position
        """, [schema, table_name])
        return result["rows_objects"]

    def close(self):
        if self.connection:
            try:
                self.connection.close()
            except Exception:
                pass
            self.connection = None


# ── MySQLConnection ───────────────────────────────────────────────────────────

class MySQLConnection:
    """Single PyMySQL connection to Azure Database for MySQL via Managed Identity."""

    def __init__(self, host: str, port: int, database: str):
        self.host = host
        self.port = port or 3306
        self.database = database
        self.connection = None
        self.in_use: bool = False
        self.last_used: float = time.time()

    def connect(self):
        if self.connection:
            return
        import pymysql
        token, username = _get_ossrdbms_token()
        self.connection = pymysql.connect(
            host=self.host, port=self.port, database=self.database,
            user=username, password=token,
            charset="utf8mb4", connect_timeout=30,
            ssl={"ssl": {}},
            cursorclass=pymysql.cursors.Cursor,
        )
        print(f"[Pool] Connected (MySQL) -> {self.database}")

    def execute_query(self, sql: str, params: Optional[list] = None) -> Dict[str, Any]:
        self.connect()
        cursor = self.connection.cursor()
        cursor.execute(sql, params or [])
        sql_upper = sql.strip().upper()
        is_modification = sql_upper.startswith(("INSERT", "UPDATE", "DELETE", "MERGE"))

        if cursor.description:
            columns = [col[0] for col in cursor.description]
            rows_arrays: List[list] = []
            rows_objects: List[dict] = []
            for row in cursor.fetchall():
                safe_row = [_safe(v) for v in row]
                rows_arrays.append(safe_row)
                rows_objects.append({columns[i]: safe_row[i] for i in range(len(columns))})
            cursor.close()
            if is_modification:
                self.connection.commit()
            return {"columns": columns, "rows": rows_arrays, "rows_objects": rows_objects, "row_count": len(rows_arrays)}
        else:
            row_count = cursor.rowcount
            cursor.close()
            self.connection.commit()
            return {"columns": [], "rows": [], "rows_objects": [], "row_count": row_count}

    def get_tables(self) -> List[Dict[str, str]]:
        result = self.execute_query("""
            SELECT table_schema AS `schema`, table_name AS `name`
            FROM information_schema.tables
            WHERE table_type = 'BASE TABLE' AND table_schema = %s
            ORDER BY table_name
        """, [self.database])
        return result["rows_objects"]

    def get_table_columns(self, schema: str, table_name: str) -> List[Dict[str, Any]]:
        result = self.execute_query("""
            SELECT column_name AS `name`, data_type AS `dataType`,
                   is_nullable AS `isNullable`, character_maximum_length AS `maxLength`
            FROM information_schema.columns
            WHERE table_schema = %s AND table_name = %s
            ORDER BY ordinal_position
        """, [schema, table_name])
        return result["rows_objects"]

    def close(self):
        if self.connection:
            try:
                self.connection.close()
            except Exception:
                pass
            self.connection = None


# ── ConnectionPool ────────────────────────────────────────────────────────────

class ConnectionPool:
    """Thread-safe connection pool backed by a Queue. Supports all DB types."""

    def __init__(
        self, endpoint: str, database: str, pool_size: int = 10,
        db_type: str = "fabric_sql",
        host: str = "", port: int = 0,
    ):
        self.endpoint = endpoint
        self.database = database
        self.pool_size = pool_size
        self.db_type = db_type
        self.host = host
        self.port = port
        self.available: Queue = Queue(maxsize=pool_size)
        self.all_connections: List[Any] = []
        self.lock = Lock()
        print(f"[Pool] Created pool for {database} ({db_type}, max={pool_size})")

    def _create_connection(self) -> Any:
        if self.db_type in ("fabric_sql", "azure_sql"):
            return FabricSQLConnection(self.endpoint, self.database)
        if self.db_type == "postgresql":
            return PostgreSQLConnection(self.host, self.port, self.database)
        if self.db_type == "mysql":
            return MySQLConnection(self.host, self.port, self.database)
        raise ValueError(f"Unsupported db_type: {self.db_type}")

    def get_connection(self) -> Any:
        try:
            conn = self.available.get(block=False)
            conn.in_use = True
            conn.last_used = time.time()
            return conn
        except Empty:
            pass

        with self.lock:
            if len(self.all_connections) < self.pool_size:
                conn = self._create_connection()
                self.all_connections.append(conn)
                conn.in_use = True
                conn.last_used = time.time()
                return conn

        # Pool full — wait up to 30 s
        try:
            conn = self.available.get(block=True, timeout=30)
            conn.in_use = True
            conn.last_used = time.time()
            return conn
        except Empty:
            raise Exception(f"Timeout waiting for connection to {self.database}")

    def return_connection(self, conn: Any):
        if conn and conn.connection:
            conn.in_use = False
            conn.last_used = time.time()
            try:
                self.available.put(conn, block=False)
            except Exception:
                pass

    def discard_connection(self, conn: Any):
        """Remove a dead connection — never recycle a stale socket."""
        with self.lock:
            try:
                self.all_connections.remove(conn)
            except ValueError:
                pass
        try:
            conn.close()
        except Exception:
            pass

    def close_all(self):
        with self.lock:
            for conn in self.all_connections:
                conn.close()
            self.all_connections.clear()
            while not self.available.empty():
                try:
                    self.available.get(block=False)
                except Empty:
                    break


# ── Global pool registry ──────────────────────────────────────────────────────

_pools: Dict[str, ConnectionPool] = {}
_pool_lock = Lock()


def _live_meta_db() -> str:
    """Return the current metadata database name from os.environ (kept live by
    main.py lifespan and config update endpoints) with fallback to the frozen
    settings object for deployments that don't use .env files."""
    import os as _os
    return _os.environ.get("METADATA_DATABASE") or settings.METADATA_DATABASE


def _live_meta_endpoint() -> str:
    import os as _os
    return _os.environ.get("METADATA_ENDPOINT") or settings.METADATA_ENDPOINT


def _live_meta_db_type() -> str:
    import os as _os
    return _os.environ.get("METADATA_DB_TYPE") or settings.METADATA_DB_TYPE or "fabric_sql"


def get_connection_pool(database: str) -> ConnectionPool:
    """Get or create the pool for *database* (keyed by database name).

    Uses os.environ for metadata DB identity so live config updates take effect
    without requiring a full process restart.
    """
    with _pool_lock:
        if database in _pools:
            return _pools[database]

        if database == _live_meta_db():
            db_type = _live_meta_db_type()
            pool_size = settings.MAX_POOL_SIZE_METADATA
            if db_type in ("fabric_sql", "azure_sql"):
                endpoint = _live_meta_endpoint()
                if not endpoint:
                    raise ValueError(f"No endpoint configured for metadata database: {database}")
                pool = ConnectionPool(endpoint, database, pool_size=pool_size, db_type=db_type)
            else:
                import os as _os
                pool = ConnectionPool(
                    "", database, pool_size=pool_size, db_type=db_type,
                    host=_os.environ.get("METADATA_HOST") or settings.METADATA_HOST,
                    port=int(_os.environ.get("METADATA_PORT") or settings.METADATA_PORT or
                             (5432 if db_type == "postgresql" else 3306)),
                )
        else:
            # Data warehouse — always Fabric SQL / Azure SQL via pyodbc
            endpoint = _resolve_endpoint(database)
            if not endpoint:
                raise ValueError(f"No endpoint configured for database: {database}")
            pool = ConnectionPool(endpoint, database, pool_size=settings.MAX_POOL_SIZE_DATAWAREHOUSE)

        _pools[database] = pool
        return pool


def _resolve_endpoint(database: str) -> str:
    """Query data_sources in the metadata DB to find the endpoint for *database*."""
    meta_db = _live_meta_db()
    if meta_db not in _pools:
        # Metadata pool not yet created — fall back to env var
        return settings.DATAWAREHOUSE_ENDPOINT or ""

    meta_pool = _pools[meta_db]
    conn = None
    try:
        conn = meta_pool.get_connection()
        result = conn.execute_query(
            "SELECT connection_string FROM data_sources "
            "WHERE database_name = ? AND is_active = 1 "
            "ORDER BY id DESC",
            [database],
        )
        if result["rows_objects"]:
            return result["rows_objects"][0].get("connection_string", "")
        return settings.DATAWAREHOUSE_ENDPOINT or ""
    except Exception as e:
        print(f"[Pool] Could not resolve endpoint for {database}: {e}")
        return settings.DATAWAREHOUSE_ENDPOINT or ""
    finally:
        if conn:
            meta_pool.return_connection(conn)


def execute_query(sql: str, database: str, params: Optional[list] = None) -> Dict[str, Any]:
    """
    Execute *sql* against *database* using the connection pool.
    Retries once on stale-connection errors (08S01 / 10054).
    """
    pool = get_connection_pool(database)
    for attempt in range(2):
        conn = pool.get_connection()
        try:
            result = conn.execute_query(sql, params)
            pool.return_connection(conn)
            return result
        except Exception as e:
            if attempt == 0 and is_connection_error(e):
                print(f"[Pool] Stale connection on {database} — discarding and retrying")
                pool.discard_connection(conn)
                continue
            pool.return_connection(conn)
            raise
    raise Exception(f"Query failed after retry on {database}")


def execute_query_cancellable(
    sql: str, database: str, params: Optional[list] = None, cancel_event=None
) -> Dict[str, Any]:
    """execute_query variant that supports mid-flight cancellation via cancel_event."""
    db_pool = get_connection_pool(database)
    conn = db_pool.get_connection()
    try:
        result = conn.execute_query_cancellable(sql, params, cancel_event=cancel_event)
        db_pool.return_connection(conn)
        return result
    except Exception as e:
        if is_connection_error(e):
            db_pool.discard_connection(conn)
        else:
            db_pool.return_connection(conn)
        raise


def get_tables(database: str) -> List[Dict[str, Any]]:
    pool = get_connection_pool(database)
    conn = pool.get_connection()
    try:
        tables = conn.get_tables()
        return [
            {
                "id": f"{t['schema']}.{t['name']}",
                "schema": t["schema"],
                "name": t["name"],
                "fullName": f"{t['schema']}.{t['name']}",
            }
            for t in tables
        ]
    finally:
        pool.return_connection(conn)


def get_table_columns(table_id: str, database: str) -> List[Dict[str, Any]]:
    parts = table_id.split(".")
    if len(parts) != 2:
        raise ValueError(f"Invalid table ID format: {table_id}")
    schema, table_name = parts
    pool = get_connection_pool(database)
    conn = pool.get_connection()
    try:
        return conn.get_table_columns(schema, table_name)
    finally:
        pool.return_connection(conn)


def _write_probe_statements(db_type: str) -> List[str]:
    """
    Returns a pair of statements that verify write access then immediately roll back.
    Uses a temp table so nothing is left behind in the database.
    """
    if db_type in ("fabric_sql", "azure_sql"):
        return [
            "CREATE TABLE #_weft_probe (id INT)",
            "DROP TABLE #_weft_probe",
        ]
    if db_type == "postgresql":
        return [
            "CREATE TEMP TABLE _weft_probe (id INT) ON COMMIT DROP",
        ]
    # mysql — session-scoped temporary table, auto-dropped on disconnect
    return [
        "CREATE TEMPORARY TABLE _weft_probe (id INT)",
        "DROP TEMPORARY TABLE _weft_probe",
    ]


def probe_connection(
    endpoint: str, database: str, statements: Optional[List[str]] = None,
    db_type: str = "fabric_sql",
    host: str = "", port: int = 0,
) -> Dict[str, Any]:
    """
    Test an arbitrary connection (used by setup wizard).
    When called without explicit statements (plain connectivity test), also
    verifies write access via a temporary table that is immediately discarded.
    Returns {success, results} or {success, error_type, message}.
    """
    check_write = statements is None
    if statements is None:
        statements = ["SELECT 1 AS connection_test"] + _write_probe_statements(db_type)

    if db_type in ("fabric_sql", "azure_sql"):
        conn: Any = FabricSQLConnection(endpoint, database)
    elif db_type == "postgresql":
        conn = PostgreSQLConnection(host, port or 5432, database)
    elif db_type == "mysql":
        conn = MySQLConnection(host, port or 3306, database)
    else:
        return {"success": False, "error_type": "connection_failed", "message": f"Unsupported db_type: {db_type}"}
    try:
        conn.connect()
        results = []
        for stmt in statements:
            stmt = stmt.strip()
            if not stmt:
                continue
            result = conn.execute_query(stmt)
            results.append({
                "success": True,
                "row_count": result["row_count"],
                "rows": result["rows"][:10],
                "columns": result["columns"],
            })
        return {"success": True, "results": results}
    except Exception as e:
        raw = str(e).lower()
        if any(k in raw for k in ["login failed", "not authorized", "access denied", "permission denied",
                                   "token authentication failed", "privilege", "create"]):
            error_type = "access_denied"
        elif any(k in raw for k in ["cannot open database", "invalid database", "does not exist", "catalog not found"]):
            error_type = "db_not_found"
        elif any(k in raw for k in ["timeout", "timed out", "connection timeout"]):
            error_type = "timeout"
        else:
            error_type = "connection_failed"
        # Give a clearer message when write check fails
        if check_write and "create" in raw:
            return {"success": False, "error_type": "access_denied",
                    "message": "Connected successfully but write access was denied. Grant CREATE TABLE permission to the identity."}
        return {"success": False, "error_type": error_type, "message": str(e)}
    finally:
        try:
            conn.close()
        except Exception:
            pass
