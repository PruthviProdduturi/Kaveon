"""
Lens database connection pool.

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
import re
import struct
import json
import time
import pyodbc
from datetime import datetime, date
from queue import Queue, Empty
from threading import Lock
from typing import Any, Dict, List, Optional
from urllib.parse import urlparse, unquote, parse_qs

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
    """True for transient errors on a stale pooled socket (ODBC + Postgres/MySQL).

    Neon/serverless Postgres closes idle connections, so we must recognise
    psycopg2/pymysql 'connection already closed' style errors and retry with a
    fresh connection.
    """
    msg = str(e).lower()
    patterns = (
        "08s01", "10054", "communication link failure",       # MSSQL / ODBC
        "connection already closed", "server closed the connection",
        "ssl connection has been closed", "connection not open",
        "terminating connection", "consuming input failed",   # psycopg2
        "lost connection", "mysql server has gone away", "broken pipe",  # pymysql
    )
    return any(p in msg for p in patterns)


# ── Dialect adaptation ─────────────────────────────────────────────────────────
# Chart/dataset SQL is generated in T-SQL (SQL Server). When a data source is
# PostgreSQL or MySQL we translate the common idioms so those queries run there.
# Idempotent — safe to apply more than once (a second pass finds nothing to change).

def adapt_sql(sql: str, db_type: str) -> str:
    """Translate T-SQL idioms to the target dialect (postgresql / mysql)."""
    if db_type in ("fabric_sql", "azure_sql", ""):
        return sql

    sql = re.sub(r"\bdbo\.", "", sql, flags=re.IGNORECASE)

    # SELECT [DISTINCT] TOP N / TOP (@x) → … LIMIT N  (keep DISTINCT if present)
    top_match = re.search(r"\bSELECT\s+(DISTINCT\s+)?TOP\s+\(?([^\s)]+)\)?\b", sql, re.IGNORECASE)
    if top_match:
        distinct = top_match.group(1) or ""
        n = top_match.group(2)
        sql = re.sub(r"\bSELECT\s+(?:DISTINCT\s+)?TOP\s+\(?[^\s)]+\)?\s*",
                     f"SELECT {distinct}", sql, count=1, flags=re.IGNORECASE)
        sql = sql.rstrip().rstrip(";") + f" LIMIT {n}"

    sql = re.sub(r"\bGET(?:UTC)?DATE\(\)", "NOW()", sql, flags=re.IGNORECASE)
    sql = re.sub(r"\bISNULL\(", "COALESCE(", sql, flags=re.IGNORECASE)
    # T-SQL string types → portable types (NVARCHAR(MAX)/VARCHAR(MAX) → TEXT; NVARCHAR(n) → VARCHAR(n))
    sql = re.sub(r"\b(?:N)?VARCHAR\s*\(\s*MAX\s*\)", "TEXT", sql, flags=re.IGNORECASE)
    sql = re.sub(r"\bNVARCHAR\b", "VARCHAR", sql, flags=re.IGNORECASE)

    # [ident] → "ident" (PostgreSQL) or `ident` (MySQL)
    if db_type == "postgresql":
        sql = re.sub(r"\[([^\]]+)\]", r'"\1"', sql)
    elif db_type == "mysql":
        sql = re.sub(r"\[([^\]]+)\]", r"`\1`", sql)

    return sql


# ── Data source connection info ────────────────────────────────────────────────

def _map_ds_type(raw: Optional[str]) -> str:
    """Map a data_sources.type label to an internal db_type."""
    t = (raw or "").strip().lower()
    if "postgre" in t:
        return "postgresql"
    if "mysql" in t or "mariadb" in t:
        return "mysql"
    if "star" in t:            # StarRocks speaks the MySQL protocol
        return "mysql"
    return "fabric_sql"        # Fabric SQL / Azure SQL (default)


def parse_db_url(conn: str) -> Dict[str, Any]:
    """Parse a PostgreSQL/MySQL connection URL into parts.

    Accepts e.g. postgresql://user:pass@host:5432/db?sslmode=require
    (the format Neon/Supabase give you). Returns host/port/user/password/sslmode/database.
    """
    conn = (conn or "").strip()
    out: Dict[str, Any] = {"host": "", "port": 0, "user": "", "password": "", "sslmode": "", "database": ""}
    if not conn:
        return out
    # Normalise scheme so urlparse populates netloc
    normalized = re.sub(r"^(postgres(ql)?|mysql)\+?\w*://", "scheme://", conn, flags=re.IGNORECASE)
    if "://" not in normalized:
        normalized = "scheme://" + normalized
    u = urlparse(normalized)
    out["host"] = u.hostname or ""
    out["port"] = u.port or 0
    out["user"] = unquote(u.username) if u.username else ""
    out["password"] = unquote(u.password) if u.password else ""
    out["database"] = (u.path or "").lstrip("/")
    q = parse_qs(u.query or "")
    if "sslmode" in q:
        out["sslmode"] = q["sslmode"][0]
    return out


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
    """Single psycopg2 connection to PostgreSQL.

    Auth precedence: explicit user/password (data sources, e.g. a registered Neon
    source) → METADATA_USER/PASSWORD env (metadata DB) → Azure AD Managed Identity.
    """

    def __init__(self, host: str, port: int, database: str,
                 user: str = "", password: str = "", sslmode: str = ""):
        self.host = host
        self.port = port or 5432
        self.database = database
        self.user = user
        self.password = password
        self.sslmode = sslmode
        self.connection = None
        self.in_use: bool = False
        self.last_used: float = time.time()

    def connect(self):
        if self.connection:
            return
        import os as _os
        import psycopg2
        user = self.user or _os.environ.get("METADATA_USER") or settings.METADATA_USER
        password = self.password or _os.environ.get("METADATA_PASSWORD") or settings.METADATA_PASSWORD
        if user and password:
            # Standard username/password auth — Neon, Supabase, self-hosted PG.
            sslmode = self.sslmode or _os.environ.get("METADATA_SSLMODE") or settings.METADATA_SSLMODE or "require"
            self.connection = psycopg2.connect(
                host=self.host, port=self.port, dbname=self.database,
                user=user, password=password,
                sslmode=sslmode, connect_timeout=30,
            )
        else:
            # Azure AD Managed Identity (Azure Database for PostgreSQL).
            token, username = _get_ossrdbms_token()
            self.connection = psycopg2.connect(
                host=self.host, port=self.port, dbname=self.database,
                user=username, password=token,
                sslmode="require", connect_timeout=30,
            )
        # autocommit avoids a failed statement poisoning the pooled connection
        # ("current transaction is aborted") — important for serverless Postgres.
        self.connection.autocommit = True
        print(f"[Pool] Connected (PostgreSQL) -> {self.database}")

    def execute_query(self, sql: str, params: Optional[list] = None) -> Dict[str, Any]:
        self.connect()
        cursor = self.connection.cursor()
        # Only pass params when present — an empty sequence still makes the driver
        # perform %-interpolation, which breaks any literal '%' (e.g. a "% Affected"
        # column alias) with "not enough arguments for format string".
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
    """Single PyMySQL connection to MySQL.

    Auth precedence: explicit user/password (data sources) → METADATA_USER/PASSWORD
    env (metadata DB) → Azure AD Managed Identity.
    """

    def __init__(self, host: str, port: int, database: str,
                 user: str = "", password: str = "", sslmode: str = ""):
        self.host = host
        self.port = port or 3306
        self.database = database
        self.user = user
        self.password = password
        self.connection = None
        self.in_use: bool = False
        self.last_used: float = time.time()

    def connect(self):
        if self.connection:
            return
        import os as _os
        import pymysql
        user = self.user or _os.environ.get("METADATA_USER") or settings.METADATA_USER
        password = self.password or _os.environ.get("METADATA_PASSWORD") or settings.METADATA_PASSWORD
        if user and password:
            # Standard username/password auth — PlanetScale, self-hosted MySQL.
            self.connection = pymysql.connect(
                host=self.host, port=self.port, database=self.database,
                user=user, password=password,
                charset="utf8mb4", connect_timeout=30,
                ssl={"ssl": {}}, autocommit=True,
                cursorclass=pymysql.cursors.Cursor,
            )
        else:
            # Azure AD Managed Identity (Azure Database for MySQL).
            token, username = _get_ossrdbms_token()
            self.connection = pymysql.connect(
                host=self.host, port=self.port, database=self.database,
                user=username, password=token,
                charset="utf8mb4", connect_timeout=30,
                ssl={"ssl": {}}, autocommit=True,
                cursorclass=pymysql.cursors.Cursor,
            )
        print(f"[Pool] Connected (MySQL) -> {self.database}")

    def execute_query(self, sql: str, params: Optional[list] = None) -> Dict[str, Any]:
        self.connect()
        cursor = self.connection.cursor()
        # Only pass params when present — an empty sequence still makes the driver
        # perform %-interpolation, which breaks any literal '%' (e.g. a "% Affected"
        # column alias) with "not enough arguments for format string".
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
        user: str = "", password: str = "", sslmode: str = "",
    ):
        self.endpoint = endpoint
        self.database = database
        self.pool_size = pool_size
        self.db_type = db_type
        self.host = host
        self.port = port
        self.user = user
        self.password = password
        self.sslmode = sslmode
        self.available: Queue = Queue(maxsize=pool_size)
        self.all_connections: List[Any] = []
        self.lock = Lock()
        print(f"[Pool] Created pool for {database} ({db_type}, max={pool_size})")

    def _create_connection(self) -> Any:
        if self.db_type in ("fabric_sql", "azure_sql"):
            return FabricSQLConnection(self.endpoint, self.database)
        if self.db_type == "postgresql":
            return PostgreSQLConnection(self.host, self.port, self.database,
                                        self.user, self.password, self.sslmode)
        if self.db_type == "mysql":
            return MySQLConnection(self.host, self.port, self.database,
                                   self.user, self.password, self.sslmode)
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
            # Data source — resolve its type + connection info from data_sources.
            ds = _resolve_data_source(database)
            ds_type = _map_ds_type(ds.get("type") if ds else None)
            pool_size = settings.MAX_POOL_SIZE_DATAWAREHOUSE
            if ds_type in ("postgresql", "mysql"):
                # e.g. a registered Neon / Supabase / PlanetScale source. The
                # connection URL (with credentials) lives in connection_string.
                info = parse_db_url(ds.get("connection_string", "") if ds else "")
                if not info["host"]:
                    raise ValueError(f"Invalid {ds_type} connection string for data source: {database}")
                pool = ConnectionPool(
                    "", database, pool_size=pool_size, db_type=ds_type,
                    host=info["host"],
                    port=info["port"] or (5432 if ds_type == "postgresql" else 3306),
                    user=info["user"], password=info["password"], sslmode=info["sslmode"],
                )
            else:
                # Fabric SQL / Azure SQL — connection_string is the ODBC endpoint.
                endpoint = (ds.get("connection_string", "") if ds else "") or settings.DATAWAREHOUSE_ENDPOINT or ""
                if not endpoint:
                    raise ValueError(f"No endpoint configured for database: {database}")
                pool = ConnectionPool(endpoint, database, pool_size=pool_size)

        _pools[database] = pool
        return pool


def _resolve_data_source(database: str) -> Optional[Dict[str, Any]]:
    """Return {type, connection_string} for a registered data source, or None.

    Dialect-aware: uses %s / TRUE against a Postgres/MySQL metadata DB and ? / 1
    against Fabric/Azure SQL.
    """
    meta_db = _live_meta_db()
    if meta_db not in _pools:
        return None
    meta_pool = _pools[meta_db]
    is_pg_my = meta_pool.db_type in ("postgresql", "mysql")
    ph = "%s" if is_pg_my else "?"
    active = "TRUE" if is_pg_my else "1"
    conn = None
    try:
        conn = meta_pool.get_connection()
        result = conn.execute_query(
            f"SELECT type, connection_string FROM data_sources "
            f"WHERE database_name = {ph} AND is_active = {active} ORDER BY id DESC",
            [database],
        )
        rows = result["rows_objects"]
        return rows[0] if rows else None
    except Exception as e:
        print(f"[Pool] Could not resolve data source {database}: {e}")
        return None
    finally:
        if conn:
            meta_pool.return_connection(conn)


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
    # Translate T-SQL idioms when the target is Postgres/MySQL (idempotent).
    sql = adapt_sql(sql, pool.db_type)
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
    sql = adapt_sql(sql, db_pool.db_type)
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
            "CREATE TABLE #_lens_probe (id INT)",
            "DROP TABLE #_lens_probe",
        ]
    if db_type == "postgresql":
        return [
            "CREATE TEMP TABLE _lens_probe (id INT) ON COMMIT DROP",
        ]
    # mysql — session-scoped temporary table, auto-dropped on disconnect
    return [
        "CREATE TEMPORARY TABLE _lens_probe (id INT)",
        "DROP TEMPORARY TABLE _lens_probe",
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
