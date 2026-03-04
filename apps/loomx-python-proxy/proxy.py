"""
LOOMX Python Proxy Service
Uses pyodbc + ODBC Driver 18 to connect to Microsoft Fabric SQL
Exposes a simple REST API for the Node.js backend to call
"""

import os
import sys
import struct
import pyodbc
import hashlib
import json
from flask import Flask, request, make_response
from flask_cors import CORS
from azure.identity import DefaultAzureCredential
from typing import List, Dict, Any, Optional
from dotenv import load_dotenv
from queue import Queue, Empty
from threading import Lock, Thread
from functools import lru_cache
from collections import OrderedDict
import time
from datetime import datetime, date

# Load environment variables from root .env file (two levels up)
load_dotenv(dotenv_path=os.path.join(os.path.dirname(__file__), '../../.env'))

app = Flask(__name__)
CORS(app)

# ── Shared Azure credential ────────────────────────────────────────────────
# One instance is shared across ALL pool connections.
# DefaultAzureCredential caches tokens internally; sharing it means only ONE
# real Azure AD token request is made regardless of how many connections are
# being established simultaneously — eliminates the multi-auth race condition
# seen when a connection storm hits a cold pool.
_azure_credential = DefaultAzureCredential()

# Custom JSON encoder to serialize datetime objects as ISO strings
# This ensures dates display exactly as stored in SQL Server (no timezone conversion)
def custom_jsonify(*args, **kwargs):
    """Custom jsonify that serializes datetime objects as ISO strings"""
    def datetime_handler(obj):
        if isinstance(obj, (datetime, date)):
            # Return ISO format without timezone: 2026-01-13T00:00:00
            # This matches the literal datetime value from SQL Server
            return obj.isoformat()
        raise TypeError(f"Object of type {type(obj)} is not JSON serializable")

    if args and kwargs:
        raise TypeError('custom_jsonify() behavior undefined when passed both args and kwargs')
    elif len(args) == 1:
        data = args[0]
    else:
        data = args or kwargs

    response = make_response(json.dumps(data, default=datetime_handler, ensure_ascii=False))
    response.headers['Content-Type'] = 'application/json'
    return response

# Replace Flask's jsonify with our custom version
jsonify = custom_jsonify

# Configuration from environment
FABRIC_DATAWAREHOUSE_ENDPOINT = os.getenv('FABRIC_DATAWAREHOUSE_ENDPOINT')
FABRIC_DATAWAREHOUSE_DATABASE = os.getenv('FABRIC_DATAWAREHOUSE_DATABASE', 'IDEASServingStoreLH')
FABRIC_METADATA_ENDPOINT = os.getenv('FABRIC_METADATA_ENDPOINT')
FABRIC_METADATA_DATABASE = os.getenv('FABRIC_METADATA_DATABASE')

# Backward compatibility
FABRIC_SQL_ENDPOINT = os.getenv('FABRIC_SQL_ENDPOINT', FABRIC_DATAWAREHOUSE_ENDPOINT)
DEFAULT_DATABASE = os.getenv('FABRIC_SQL_DATABASE', FABRIC_DATAWAREHOUSE_DATABASE)

# ODBC Driver 18 constants (from FabricExplorer)
SQL_COPT_SS_ACCESS_TOKEN = 1256
TOKEN_URL = "https://database.windows.net/.default"

# Cache configuration - DISABLED FOR LIVE DATA
# ONLY authentication tokens are cached (handled by MSAL/Azure Identity)
# All data queries are LIVE - no caching

# Connection pool configuration
MAX_POOL_SIZE_DATAWAREHOUSE = int(os.getenv('MAX_POOL_SIZE_DATAWAREHOUSE', '10'))  # Data warehouse pool size
MAX_POOL_SIZE_METADATA = int(os.getenv('MAX_POOL_SIZE_METADATA', '20'))  # Metadata pool size (increased for concurrent queries)
CONNECTION_TIMEOUT_DATAWAREHOUSE = int(os.getenv('CONNECTION_TIMEOUT_DATAWAREHOUSE', '60'))  # Datawarehouse timeout (seconds)
CONNECTION_TIMEOUT_METADATA = int(os.getenv('CONNECTION_TIMEOUT_METADATA', '30'))  # Metadata timeout (seconds)


def mask_endpoint(endpoint: str) -> str:
    """Mask endpoint for security - only show suffix"""
    if not endpoint:
        return '(none)'
    # Show only the suffix after the first dash, e.g., "***-database.fabric.microsoft.com"
    parts = endpoint.split('-', 1)
    if len(parts) == 2:
        return f"***-{parts[1]}"
    # If no dash, just show the domain
    if '.' in endpoint:
        return f"***.{endpoint.split('.', 1)[1]}"
    return "***"


# CACHING DISABLED - All data is LIVE
# Only authentication credentials are cached (handled by Azure Identity)
# This ensures real-time data for all queries


class FabricSQLConnection:
    """Fabric SQL connection using Azure AD token auth (pyodbc + ODBC Driver 18)."""

    def __init__(self, server: str, database: str):
        self.server = server
        self.database = database
        self.credential = _azure_credential  # shared singleton — no per-connection auth races
        self.connection = None
        self.in_use = False  # Track if connection is currently in use
        self.last_used = time.time()

    def _try_token_authentication(self) -> bool:
        """Connect using Azure AD token (SQL_COPT_SS_ACCESS_TOKEN injection via pyodbc)."""
        try:
            print(f"[Proxy] Trying token authentication for {self.database}...")
            token_response = self.credential.get_token(TOKEN_URL)
            token = token_response.token

            # Encode token exactly like FabricExplorer (UTF-16-LE + struct pack)
            token_bytes = token.encode('UTF-16-LE')
            token_struct = struct.pack(f'<I{len(token_bytes)}s', len(token_bytes), token_bytes)

            connection_string = (
                "DRIVER={ODBC Driver 18 for SQL Server};"
                f"SERVER={self.server};"
                f"DATABASE={self.database};"
                "Encrypt=yes;"
                "TrustServerCertificate=yes;"
                "Connection Timeout=60;"
            )

            self.connection = pyodbc.connect(
                connection_string,
                attrs_before={SQL_COPT_SS_ACCESS_TOKEN: token_struct}
            )

            # Test connection
            cursor = self.connection.cursor()
            cursor.execute("SELECT 1")
            cursor.fetchone()
            cursor.close()

            print(f"[Proxy] Token authentication successful for {self.database}")
            return True

        except Exception as e:
            print(f"[Proxy] Token authentication failed: {str(e)}")
            return False

    def connect(self):
        """Connect to Fabric SQL using Azure AD token authentication."""
        if self.connection:
            return

        print(f"[Proxy] Connecting to {mask_endpoint(self.server)}/{self.database}...")

        if self._try_token_authentication():
            print(f"[Proxy] Connected successfully to {self.database}")
            return

        raise Exception(f"Failed to connect to {self.server}/{self.database}: Azure AD token authentication failed")

    def execute_query(self, sql: str, params: list = None, use_cache: bool = False) -> Dict[str, Any]:
        """Execute SQL query and return results (LIVE DATA - no caching)"""
        # LIVE DATA MODE - No caching for real-time results
        self.connect()

        start_time = time.time()
        cursor = self.connection.cursor()
        if params:
            cursor.execute(sql, params)
        else:
            cursor.execute(sql)

        # Detect if this is a modification query (INSERT/UPDATE/DELETE)
        # Even if it returns results (e.g., INSERT ... OUTPUT INSERTED.id)
        sql_upper = sql.strip().upper()
        is_modification = sql_upper.startswith(('INSERT', 'UPDATE', 'DELETE', 'MERGE'))

        # Check if this is a SELECT query (has result set) or UPDATE/INSERT/DELETE (no result set)
        if cursor.description:
            # SELECT query - fetch rows
            columns = [column[0] for column in cursor.description]

            # Get rows as arrays (for Lab page) or objects (for metadata queries)
            # Lab page expects arrays, so we return both formats
            rows_as_arrays = []
            rows_as_objects = []

            # JavaScript's Number.MAX_SAFE_INTEGER = 2^53 - 1 = 9007199254740991
            # Integers outside this range lose precision when parsed as JSON numbers
            # Solution: Send as strings, SQL Server accepts string literals for numeric columns
            MAX_SAFE_INTEGER = 9007199254740991

            def preserve_large_integer(value):
                """Convert large integers to strings to preserve precision in JavaScript"""
                if isinstance(value, int) and abs(value) > MAX_SAFE_INTEGER:
                    return str(value)
                return value

            for row in cursor.fetchall():
                # Convert row to list with large integer preservation
                row_array = [preserve_large_integer(value) for value in row]
                rows_as_arrays.append(row_array)

                # Also create object format for metadata queries
                row_dict = {}
                for idx, value in enumerate(row):
                    row_dict[columns[idx]] = preserve_large_integer(value)
                rows_as_objects.append(row_dict)

            cursor.close()

            # CRITICAL: Commit if this was a modification query (e.g., INSERT ... OUTPUT INSERTED.id)
            if is_modification:
                self.connection.commit()
                print(f"[Proxy] Committed {sql_upper.split()[0]} transaction (returned results)")

            result = {
                'columns': columns,
                'rows': rows_as_arrays,  # Lab page expects arrays
                'rows_objects': rows_as_objects,  # Metadata queries use objects
                'row_count': len(rows_as_arrays)
            }

            # LIVE DATA - No caching
            return result
        else:
            # UPDATE/INSERT/DELETE query - return affected row count
            row_count = cursor.rowcount
            cursor.close()

            # CRITICAL: Commit the transaction so changes are visible to subsequent queries
            self.connection.commit()

            result = {
                'columns': [],
                'rows': [],
                'rows_objects': [],
                'row_count': row_count
            }

            return result

    def get_tables(self) -> List[Dict[str, str]]:
        """Get list of tables in database (LIVE DATA - no caching)"""
        # LIVE DATA MODE - No caching
        sql = """
            SELECT
                TABLE_SCHEMA as [schema],
                TABLE_NAME as [name]
            FROM INFORMATION_SCHEMA.TABLES
            WHERE TABLE_TYPE = 'BASE TABLE'
            ORDER BY TABLE_SCHEMA, TABLE_NAME
        """

        result = self.execute_query(sql, use_cache=False)  # Don't double cache

        # Convert array rows to dictionaries
        tables = []
        for row in result['rows']:
            if isinstance(row, list):
                # Row is an array - convert to dict using column names
                table_dict = {}
                for i, col_name in enumerate(result['columns']):
                    table_dict[col_name] = row[i]
                tables.append(table_dict)
            else:
                # Row is already a dict
                tables.append(row)

        # LIVE DATA - No caching
        return tables

    def get_table_columns(self, schema: str, table_name: str) -> List[Dict[str, Any]]:
        """Get columns for a specific table (LIVE DATA - no caching)"""
        # LIVE DATA MODE - No caching
        sql = f"""
            SELECT
                COLUMN_NAME as [name],
                DATA_TYPE as [dataType],
                IS_NULLABLE as [isNullable],
                CHARACTER_MAXIMUM_LENGTH as [maxLength]
            FROM INFORMATION_SCHEMA.COLUMNS
            WHERE TABLE_SCHEMA = '{schema.replace("'", "''")}'
                AND TABLE_NAME = '{table_name.replace("'", "''")}'
            ORDER BY ORDINAL_POSITION
        """

        result = self.execute_query(sql, use_cache=False)  # Don't double cache

        # Convert array rows to dictionaries
        columns = []
        for row in result['rows']:
            if isinstance(row, list):
                # Row is an array - convert to dict using column names
                col_dict = {}
                for i, col_name in enumerate(result['columns']):
                    col_dict[col_name] = row[i]
                columns.append(col_dict)
            else:
                # Row is already a dict
                columns.append(row)

        # LIVE DATA - No caching
        return columns

    def close(self):
        """Close connection"""
        if self.connection:
            self.connection.close()
            self.connection = None
            print(f"[Proxy] Connection closed for {self.database}")


class ConnectionPool:
    """Thread-safe connection pool for high performance"""

    def __init__(self, endpoint: str, database: str, pool_size: int = 10):
        self.endpoint = endpoint
        self.database = database
        self.pool_size = pool_size
        self.available = Queue(maxsize=pool_size)
        self.all_connections = []
        self.lock = Lock()

        print(f"[Pool] Created connection pool for {database} (max size: {pool_size})")

    def get_connection(self) -> FabricSQLConnection:
        """Get a connection from the pool (thread-safe)"""
        try:
            # Try to get an available connection (non-blocking)
            conn = self.available.get(block=False)
            conn.in_use = True
            conn.last_used = time.time()
            return conn
        except Empty:
            # No available connections, create a new one if under pool size
            with self.lock:
                if len(self.all_connections) < self.pool_size:
                    print(f"[Pool] Creating new connection for {self.database} ({len(self.all_connections) + 1}/{self.pool_size})")
                    conn = FabricSQLConnection(self.endpoint, self.database)
                    self.all_connections.append(conn)
                    conn.in_use = True
                    conn.last_used = time.time()
                    return conn

            # Pool is full, wait for an available connection (with timeout)
            print(f"[Pool] Waiting for available connection to {self.database}...")
            try:
                conn = self.available.get(block=True, timeout=30)
                conn.in_use = True
                conn.last_used = time.time()
                return conn
            except Empty:
                raise Exception(f"Timeout waiting for connection to {self.database}")

    def return_connection(self, conn: FabricSQLConnection):
        """Return a connection to the pool"""
        if conn and conn.connection:
            conn.in_use = False
            conn.last_used = time.time()
            try:
                self.available.put(conn, block=False)
            except:
                # Queue is full, this shouldn't happen but handle gracefully
                print(f"[Pool] Warning: Could not return connection to pool (pool full)")

    def close_all(self):
        """Close all connections in the pool"""
        with self.lock:
            for conn in self.all_connections:
                conn.close()
            self.all_connections.clear()
            # Clear the queue
            while not self.available.empty():
                try:
                    self.available.get(block=False)
                except Empty:
                    break


# Data sources - LIVE DATA (no caching)
def get_data_source_info(database: str) -> Optional[Dict[str, str]]:
    """
    Query data_sources table from metadata DB to get endpoint for a database.
    Returns dict with 'endpoint' and 'database' or None if not found.
    LIVE DATA - No caching.
    """
    # LIVE DATA MODE - No caching, always query database
    conn = None
    try:
        # Connect to metadata database
        metadata_pool = get_connection_pool(FABRIC_METADATA_DATABASE)
        conn = metadata_pool.get_connection()

        # Query data_sources table
        sql = """
            SELECT
                id,
                name,
                type,
                connection_string as endpoint,
                database_name,
                region,
                is_active
            FROM data_sources
            WHERE database_name = ?
            ORDER BY is_active DESC, id DESC
        """

        result = conn.execute_query(sql, params=[database])

        if result['rows_objects']:
            row = result['rows_objects'][0]
            data_source_info = {
                'id': row.get('id'),
                'name': row.get('name'),
                'type': row.get('type'),
                'endpoint': row.get('endpoint'),
                'database': row.get('database_name'),
                'region': row.get('region'),
                'is_active': row.get('is_active')
            }

            # LIVE DATA - No caching
            return data_source_info

        return None

    except Exception as e:
        print(f"[DataSource] Error querying data_sources table: {e}")
        return None
    finally:
        if conn:
            metadata_pool.return_connection(conn)


# Connection pools per database (keyed by endpoint:database for uniqueness)
connection_pools: Dict[str, ConnectionPool] = {}
pool_lock = Lock()



def get_connection_pool(database: str = None) -> ConnectionPool:
    """Get or create a connection pool for the specified database"""
    db = database or FABRIC_METADATA_DATABASE or 'default'

    # Create unique pool key: endpoint:database
    # Get or create pool (thread-safe)
    with pool_lock:
        # Check if pool already exists for this database
        if db in connection_pools:
            return connection_pools[db]

        # Determine endpoint based on database type
        if db == FABRIC_METADATA_DATABASE:
            # Metadata database - use configured endpoint
            endpoint = FABRIC_METADATA_ENDPOINT
            pool_size = MAX_POOL_SIZE_METADATA
            print(f"[Proxy] Creating pool for METADATA: {db} at {mask_endpoint(endpoint)} (pool size: {pool_size})")
        else:
            # User data source (warehouse, lakehouse, etc.) - query data_sources table
            # Note: Don't call get_data_source_info here to avoid recursion
            # We'll handle it separately
            data_source = None

            # Try to query data sources (but don't use get_connection_pool to avoid recursion)
            try:
                # Create a temporary connection to metadata DB if pool exists
                if FABRIC_METADATA_DATABASE in connection_pools:
                    metadata_pool = connection_pools[FABRIC_METADATA_DATABASE]
                    conn = None
                    try:
                        conn = metadata_pool.get_connection()
                        sql = """
                            SELECT
                                id,
                                name,
                                type,
                                connection_string as endpoint,
                                database_name,
                                region,
                                is_active
                            FROM data_sources
                            WHERE database_name = ?
                            ORDER BY is_active DESC, id DESC
                        """
                        result = conn.execute_query(sql, params=[db])
                        if result['rows_objects']:
                            data_source = result['rows_objects'][0]
                    finally:
                        if conn:
                            metadata_pool.return_connection(conn)
            except Exception as e:
                print(f"[Proxy] Warning: Could not query data_sources for {db}: {e}")

            if data_source:
                endpoint = data_source['endpoint']
                ds_type = data_source.get('type', 'unknown')
                ds_name = data_source.get('name', db)
                pool_size = MAX_POOL_SIZE_DATAWAREHOUSE
                print(f"[Proxy] Creating pool for {ds_type.upper()}: '{ds_name}' ({db}) at {mask_endpoint(endpoint)} (pool size: {pool_size})")
            else:
                # Fallback to environment variable (backward compatibility)
                endpoint = FABRIC_DATAWAREHOUSE_ENDPOINT or FABRIC_SQL_ENDPOINT
                pool_size = MAX_POOL_SIZE_DATAWAREHOUSE
                if endpoint:
                    print(f"[Proxy] Creating pool for SQL ENDPOINT (fallback from .env): {db} at {mask_endpoint(endpoint)} (pool size: {pool_size})")
                else:
                    print(f"[Proxy] ERROR: No endpoint found for database {db}")
                    raise ValueError(f"No endpoint configured for database: {db}")

        # Create connection pool
        connection_pools[db] = ConnectionPool(endpoint, db, pool_size=pool_size)

    return connection_pools[db]


def get_connection(database: str = None) -> FabricSQLConnection:
    """Get a connection from the pool (fast and thread-safe)"""
    pool = get_connection_pool(database)
    return pool.get_connection()


def return_connection(conn: FabricSQLConnection, database: str = None):
    """Return a connection to the pool"""
    if conn:
        db = database or conn.database or DEFAULT_DATABASE
        if db in connection_pools:
            connection_pools[db].return_connection(conn)
        else:
            # Pool doesn't exist, just close the connection
            conn.close()


# API Routes

# Warmup gate — ensures at most one warmup thread runs at a time.
# Set to True when /api/v1/warmup is first called (user sign-in).
_warmup_triggered = False
_warmup_trigger_lock = Lock()


def _run_warmup_and_heartbeat():
    """
    Warm the metadata DB first, then discover and warm all active data sources
    (warehouses, lakehouses) in parallel. Keeps all pools alive with a 5-minute
    heartbeat thereafter.

    Flow:
      1. Warm metadata (6 conns) — all endpoint details live here.
      2. Query data_sources for active DB names.
      3. Warm each data source pool (1 conn each) in parallel threads.
      4. Heartbeat loop: ping all pools every 5 min.

    Uses retry-with-backoff (0s, 10s, 20s) for each pool independently.
    """
    def _warm_pool(db_name: str, n_conns: int, label: str):
        """Warm n_conns connections for the given database, with retry.
        Endpoint is resolved automatically by get_connection_pool (from data_sources table)."""
        if not db_name:
            return False
        delays = [0, 10, 20]
        for attempt, delay in enumerate(delays, 1):
            if delay:
                time.sleep(delay)
            conns = []
            try:
                print(f"[Proxy] Warming {label} pool ({db_name}) — attempt {attempt}/3…")
                for _ in range(n_conns):
                    conn = get_connection(db_name)
                    conn.execute_query("SELECT 1 AS warmup")
                    conns.append(conn)
                print(f"[Proxy] {label} pool ready ({n_conns} connection(s) warmed).")
                return True
            except Exception as e:
                print(f"[Proxy] {label} warmup attempt {attempt} failed: {e}")
                if attempt < len(delays):
                    print(f"[Proxy] Retrying {label} in {delays[attempt]}s…")
            finally:
                for conn in conns:
                    return_connection(conn, db_name)
        print(f"[Proxy] {label} warmup gave up — first request will connect lazily.")
        return False

    # Step 1: Warm metadata DB first.
    # All SQL endpoint details live in metadata (data_sources table), so metadata
    # must be connected before we can discover and warm any other database.
    #
    # Why 6 connections for metadata:
    #   GET /favorite/current  → 1 query
    #   GET /list              → 1 query
    #   GET /status            → 1 query
    #   GET /summary           → 5 parallel sub-queries (datasets, charts,
    #                            dashboards, favorites, saved_queries)
    #   Total simultaneous     → 8; 6 warm connections covers the summary
    #                            burst and leaves 2 for the other callers.
    meta_ok = _warm_pool(FABRIC_METADATA_DATABASE, 6, "metadata")

    if not meta_ok:
        return  # metadata unavailable — can't look up any other endpoints

    # Step 2: Discover all active data sources from metadata and warm them in parallel.
    # Every warehouse/lakehouse endpoint is stored in data_sources — no hardcoding needed.
    active_dbs: list = []
    try:
        conn = get_connection(FABRIC_METADATA_DATABASE)
        try:
            result = conn.execute_query(
                "SELECT DISTINCT database_name FROM data_sources "
                "WHERE is_active = 1 AND database_name IS NOT NULL"
            )
            active_dbs = [row['database_name'] for row in (result.get('rows_objects') or [])]
            print(f"[Proxy] Discovered {len(active_dbs)} active data source(s) to warm: {active_dbs}")
        finally:
            return_connection(conn, FABRIC_METADATA_DATABASE)
    except Exception as e:
        print(f"[Proxy] Could not query data_sources for warmup: {e}")

    # Fallback: also warm FABRIC_DATAWAREHOUSE_DATABASE if set and not already covered
    if FABRIC_DATAWAREHOUSE_DATABASE and FABRIC_DATAWAREHOUSE_DATABASE not in active_dbs:
        active_dbs.append(FABRIC_DATAWAREHOUSE_DATABASE)

    ds_threads = []
    for db_name in active_dbs:
        t = Thread(
            target=_warm_pool,
            args=(db_name, 1, f"data-source({db_name})"),
            daemon=True,
        )
        t.start()
        ds_threads.append((t, db_name))

    for t, _ in ds_threads:
        t.join()  # wait for all data sources before starting heartbeat

    # Heartbeat: ping all warmed pools every 5 min to keep Fabric serverless alive.
    # Uses the same active_dbs list discovered above, plus the metadata DB.
    heartbeat_dbs = [FABRIC_METADATA_DATABASE] + active_dbs
    while True:
        time.sleep(300)
        for db in heartbeat_dbs:
            if not db:
                continue
            hb_conn = None
            try:
                hb_conn = get_connection(db)
                hb_conn.execute_query("SELECT 1 AS heartbeat")
            except Exception as e:
                print(f"[Proxy] heartbeat failed for {db} (will reconnect on next request): {e}")
            finally:
                if hb_conn:
                    return_connection(hb_conn, db)


@app.route('/api/v1/warmup', methods=['POST'])
def trigger_warmup():
    """
    Fire-and-forget pool warmup — called by the Node.js API immediately after
    a user signs in (POST /connect). Returns 202 instantly; warmup runs in a
    background thread alongside the user's page data fetches.

    Idempotent: safe to call multiple times; only one warmup thread ever runs.
    """
    global _warmup_triggered
    with _warmup_trigger_lock:
        if _warmup_triggered:
            return jsonify({'status': 'already_started'}), 200
        _warmup_triggered = True
    Thread(target=_run_warmup_and_heartbeat, daemon=True).start()
    return jsonify({'status': 'started'}), 202


@app.route('/health', methods=['GET'])
def health():
    """Health check endpoint - LIVE DATA mode (no caching)"""
    return jsonify({
        'status': 'ok',
        'service': 'loomx-python-proxy',
        'features': ['connection-pooling', 'parallel-processing', 'live-data'],
        'data_mode': 'LIVE - No caching (only auth tokens cached)',
        'pools': {
            db: {
                'size': len(pool.all_connections),
                'max_size': pool.pool_size
            }
            for db, pool in connection_pools.items()
        }
    })


@app.route('/api/v1/cache/stats', methods=['GET'])
def cache_stats():
    """Cache statistics - DISABLED (LIVE DATA mode)"""
    return jsonify({
        'status': 'disabled',
        'message': 'Caching disabled - All data is LIVE',
        'data_mode': 'LIVE - No caching (only auth tokens cached)'
    })


@app.route('/api/v1/cache/clear', methods=['POST'])
def clear_cache():
    """Clear cache - NO-OP (LIVE DATA mode)"""
    return jsonify({
        'status': 'ok',
        'message': 'No cache to clear - All data is LIVE',
        'data_mode': 'LIVE - No caching (only auth tokens cached)'
    })


@app.route('/api/v1/tables', methods=['GET'])
def get_tables():
    """Get list of tables"""
    conn = None
    database = None
    start_time = time.time()
    try:
        database = request.args.get('database', DEFAULT_DATABASE)
        conn = get_connection(database)
        tables = conn.get_tables()

        # Format like LOOMX expects
        formatted = [
            {
                'id': f"{t['schema']}.{t['name']}",
                'schema': t['schema'],
                'name': t['name'],
                'fullName': f"{t['schema']}.{t['name']}"
            }
            for t in tables
        ]

        return jsonify(formatted)

    except Exception as e:
        print(f"[Proxy] Error fetching tables: {str(e)}")
        import traceback
        traceback.print_exc()
        return jsonify({
            'error': 'Failed to fetch tables',
            'message': str(e),
            'tables': []
        }), 500
    finally:
        if conn:
            return_connection(conn, database)


@app.route('/api/v1/tables/<path:table_id>/columns', methods=['GET'])
def get_table_columns(table_id: str):
    """Get columns for a specific table"""
    conn = None
    database = None
    try:
        database = request.args.get('database', DEFAULT_DATABASE)

        # Parse schema.table
        parts = table_id.split('.')
        if len(parts) != 2:
            return jsonify({'error': 'Invalid table ID format'}), 400

        schema, table_name = parts
        conn = get_connection(database)
        columns = conn.get_table_columns(schema, table_name)

        return jsonify(columns)

    except Exception as e:
        print(f"[Proxy] Error fetching columns: {str(e)}")
        return jsonify({
            'error': 'Failed to fetch columns',
            'message': str(e)
        }), 500
    finally:
        if conn:
            return_connection(conn, database)


@app.route('/api/v1/execute', methods=['POST'])
def execute_query():
    """Execute SQL query"""
    conn = None
    database = None
    start_time = time.time()
    try:
        data = request.json
        sql = data.get('sql')
        database = data.get('database', DEFAULT_DATABASE)

        if not sql:
            return jsonify({'error': 'SQL query is required'}), 400

        conn = get_connection(database)
        result = conn.execute_query(sql)

        return jsonify({
            'columns': result['columns'],
            'rows': result['rows'],
            'rowCount': result['row_count']
        })

    except Exception as e:
        print(f"[Execute] ERROR: {str(e)}")
        import traceback
        traceback.print_exc()
        return jsonify({
            'error': 'Query execution failed',
            'message': str(e)
        }), 500
    finally:
        if conn:
            return_connection(conn, database)


@app.route('/api/v1/execute/batch', methods=['POST'])
def execute_batch():
    """Execute multiple SQL queries in parallel"""
    start_time = time.time()
    try:
        data = request.json
        queries = data.get('queries', [])
        database = data.get('database', DEFAULT_DATABASE)

        if not queries:
            return jsonify({'error': 'Queries array is required'}), 400

        results = []
        errors = []
        connections = []

        def execute_single_query(index, sql):
            """Execute a single query (for parallel execution)"""
            conn = None
            try:
                conn = get_connection(database)
                result = conn.execute_query(sql)
                results.append({
                    'index': index,
                    'success': True,
                    'columns': result['columns'],
                    'rows': result['rows'],
                    'rowCount': result['row_count']
                })
                connections.append(conn)
            except Exception as e:
                errors.append({
                    'index': index,
                    'success': False,
                    'error': str(e)
                })
                if conn:
                    connections.append(conn)

        # Execute all queries in parallel using threads
        threads = []
        for idx, query_sql in enumerate(queries):
            thread = Thread(target=execute_single_query, args=(idx, query_sql))
            thread.start()
            threads.append(thread)

        # Wait for all threads to complete
        for thread in threads:
            thread.join()

        # Return all connections to pool
        for conn in connections:
            return_connection(conn, database)

        # Combine results and errors, maintaining order
        combined = []
        for i in range(len(queries)):
            # Find result or error for this index
            result_item = next((r for r in results if r['index'] == i), None)
            error_item = next((e for e in errors if e['index'] == i), None)
            combined.append(result_item or error_item)

        return jsonify({
            'total': len(queries),
            'successful': len(results),
            'failed': len(errors),
            'results': combined
        })

    except Exception as e:
        print(f"[Proxy] Error in batch execution: {str(e)}")
        import traceback
        traceback.print_exc()
        return jsonify({
            'error': 'Batch execution failed',
            'message': str(e)
        }), 500


@app.route('/api/v1/probe', methods=['POST'])
def probe_connection():
    """
    Test an arbitrary Fabric SQL connection.
    Used by the LoomX setup wizard to validate and initialise the metadata database.

    Request body:
      endpoint   - Fabric SQL endpoint hostname (e.g. "xxx.datawarehouse.fabric.microsoft.com")
      database   - Database name
      statements - (optional) list of SQL statements to execute in sequence.
                   Defaults to ['SELECT 1 AS test'].

    Response (always HTTP 200):
      { success: true,  results: [...] }
      { success: false, error_type: str, message: str }

    error_type values:
      connection_failed  - cannot reach the server
      access_denied      - connected but authentication / permission denied
      db_not_found       - server reachable but database does not exist
      timeout            - connection timed out
    """
    data = request.json or {}
    endpoint = data.get('endpoint')
    database = data.get('database')
    statements = data.get('statements', ['SELECT 1 AS test'])

    if not endpoint or not database:
        return jsonify({
            'success': False,
            'error_type': 'invalid_request',
            'message': 'endpoint and database are required',
        })

    conn = None
    try:
        conn = FabricSQLConnection(endpoint, database)
        conn.connect()

        results = []
        for stmt in statements:
            stmt = stmt.strip()
            if not stmt:
                continue
            result = conn.execute_query(stmt)
            results.append({
                'success': True,
                'row_count': result['row_count'],
                'rows': result['rows'][:10],   # cap at 10 rows
                'columns': result['columns'],
            })

        return jsonify({'success': True, 'results': results})

    except Exception as e:
        raw = str(e).lower()
        if any(k in raw for k in ['login failed', 'not authorized', 'access denied', 'permission denied', 'token authentication failed']):
            error_type = 'access_denied'
        elif any(k in raw for k in ['cannot open database', 'invalid database', 'does not exist', 'catalog not found']):
            error_type = 'db_not_found'
        elif any(k in raw for k in ['timeout', 'timed out', 'connection timeout']):
            error_type = 'timeout'
        else:
            error_type = 'connection_failed'

        print(f"[Probe] {error_type}: {endpoint}/{database} — {str(e)}")
        return jsonify({
            'success': False,
            'error_type': error_type,
            'message': str(e),
        })
    finally:
        if conn:
            try:
                conn.close()
            except Exception:
                pass


@app.route('/api/v1/shutdown', methods=['POST'])
def shutdown():
    """Graceful shutdown — called by setup route to force config reload on restart."""
    Thread(target=lambda: (time.sleep(0.5), os._exit(0)), daemon=True).start()
    return jsonify({'success': True, 'message': 'Proxy shutting down for config reload'})


def warm_connection_pool():
    """Pre-warm connection pools with a few connections to reduce first-request latency"""
    print("[Proxy] Warming connection pools (LIVE DATA mode - no caching)...")

    databases_to_warm = []

    # Always warm metadata database if configured
    if FABRIC_METADATA_DATABASE:
        databases_to_warm.append(FABRIC_METADATA_DATABASE)

    # Optionally warm default database
    if DEFAULT_DATABASE and DEFAULT_DATABASE != FABRIC_METADATA_DATABASE:
        databases_to_warm.append(DEFAULT_DATABASE)

    for db in databases_to_warm:
        try:
            print(f"[Proxy] Warming {db} pool...")
            # Get 3 connections from pool to establish initial connections
            conns = [get_connection(db) for _ in range(3)]

            # Execute simple query to fully establish connections in parallel
            for conn in conns:
                conn.execute_query("SELECT 1 AS warmup", use_cache=False)

            # Return all connections to pool
            for conn in conns:
                return_connection(conn, db)

            print(f"[Proxy] Warmed {db} with {len(conns)} connections")

        except Exception as e:
            print(f"[Proxy] Warning: Failed to warm {db}: {str(e)}")


if __name__ == '__main__':
    # Metadata endpoint/database are optional during first-time setup.
    # The LoomX setup wizard will provide credentials and update .env.
    # Only /api/v1/probe (and the health endpoint) are fully functional
    # until the metadata DB is configured.
    if not FABRIC_METADATA_ENDPOINT or not FABRIC_METADATA_DATABASE:
        print("WARNING: FABRIC_METADATA_ENDPOINT / FABRIC_METADATA_DATABASE not set.")
        print("         Proxy starting in setup mode — /api/v1/probe is available.")
        print("         Use the LoomX setup wizard after login to configure your metadata DB.")

    print("=" * 44)
    print("LOOMX Python Proxy")
    print("=" * 44)
    print(f"Server: http://localhost:5001")
    print(f"Health: http://localhost:5001/health")
    print("=" * 44)

    # Start pool warmup immediately at server startup — don't wait for first user sign-in.
    # Fabric SQL cold start takes ~9s; warming now means connections are ready
    # by the time the first user signs in, keeping page load under 5s.
    if FABRIC_METADATA_ENDPOINT and FABRIC_METADATA_DATABASE:
        with _warmup_trigger_lock:
            _warmup_triggered = True
        Thread(target=_run_warmup_and_heartbeat, daemon=True).start()
        print("[Proxy] Connection pool warmup started at server startup.")

    # Run Flask app
    # use_reloader=False prevents double startup banner from Flask's reloader
    app.run(host='0.0.0.0', port=5001, debug=True, use_reloader=False)
