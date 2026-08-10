"""
Context profiler — builds a *global context representation* of a structured
dataset **without a large language model**, by reading the database's own
statistics catalogs rather than scanning the data.

Three signals make up the context representation (patent claim element 1):

  1. Column value distributions  -> Postgres ``pg_stats``
       (n_distinct, null_frac, most_common_vals/_freqs, histogram_bounds).
       These are maintained by ANALYZE / autovacuum and read in O(1) — no
       table scan, so profiling a 100M-row table costs the same as a 100-row one.

  2. Inferred relationships       -> ``pg_constraint`` foreign keys, plus a
       name/type heuristic that links ``<t>_id`` columns to a ``<t>`` primary key
       even when no FK is declared.

  3. Query pattern frequency      -> the platform's own ``query_history`` table
       (how often each physical table has actually been asked about).

The profile, together with the *change counters* captured at profiling time
(``n_live_tup``, ``n_mod_since_analyze``, ``last_analyze``), is what the
validity engine later scores for staleness. Capturing those counters here is
what makes cheap change-detection possible downstream — we never re-query the
data to know it moved.

Storage: snapshots live in the platform metadata DB (``database.metadata``).
Statistics are read from the warehouse pool (``database.pool``).
"""

from __future__ import annotations

import json
import uuid
from datetime import datetime, timezone
from typing import Any, Dict, List, Optional

import database.metadata as meta
import database.pool as pool

# element-key convention shared with context_validity / context_router
#   table  element:  "schema.table"
#   column element:  "schema.table.column"


def _now() -> datetime:
    return datetime.now(timezone.utc).replace(tzinfo=None)


def table_key(schema: str, table: str) -> str:
    return f"{schema}.{table}"


def column_key(schema: str, table: str, column: str) -> str:
    return f"{schema}.{table}.{column}"


# --------------------------------------------------------------------------- #
# metadata storage (self-migrating)                                           #
# --------------------------------------------------------------------------- #

# Postgres-native DDL. The metadata store is Postgres in every deployment that
# supports the context engine; on any other metadata dialect the engine no-ops
# (see is_supported()). JSON is stored as TEXT for portability; ids are app-
# generated UUID strings so we avoid SERIAL/IDENTITY dialect differences.
_DDL = """
CREATE TABLE IF NOT EXISTS context_snapshots (
    id              TEXT PRIMARY KEY,
    database_name   TEXT NOT NULL,
    schema_name     TEXT NOT NULL,
    table_name      TEXT NOT NULL,
    column_name     TEXT,                    -- NULL => table-level element
    element_key     TEXT NOT NULL,
    element_type    TEXT NOT NULL,           -- 'table' | 'column'
    profile         TEXT,                    -- JSON: distribution / relationship stats
    row_count       BIGINT,                  -- n_live_tup at capture
    mods_at_capture BIGINT,                  -- n_mod_since_analyze at capture
    last_analyze    TEXT,                    -- source last_analyze at capture (ISO)
    usage_count     BIGINT DEFAULT 0,        -- query_history hits for this table
    captured_at     TEXT NOT NULL,
    refreshed_at    TEXT NOT NULL
);
CREATE UNIQUE INDEX IF NOT EXISTS ux_context_snapshots_elem
    ON context_snapshots (database_name, element_key);

CREATE TABLE IF NOT EXISTS context_answer_cache (
    id              TEXT PRIMARY KEY,
    database_name   TEXT NOT NULL,
    question_sig    TEXT NOT NULL,           -- normalized question signature
    sql_hash        TEXT,
    result          TEXT,                    -- JSON: cached result set
    element_deps    TEXT,                    -- JSON: [element_key, ...] this answer relied on
    created_at      TEXT NOT NULL,
    last_served_at  TEXT NOT NULL,
    serve_count     BIGINT DEFAULT 0,
    validity_at_serve DOUBLE PRECISION
);
CREATE UNIQUE INDEX IF NOT EXISTS ux_context_answer_sig
    ON context_answer_cache (database_name, question_sig);
"""

_tables_ready = False


def is_supported() -> bool:
    """Context engine requires a Postgres metadata store (for the catalogs it
    mirrors) — it degrades to a no-op elsewhere rather than erroring."""
    import os
    dbt = None
    try:
        from config import settings
        dbt = getattr(settings, "METADATA_DB_TYPE", None)
    except Exception:
        dbt = None
    dbt = os.environ.get("METADATA_DB_TYPE") or dbt or "fabric_sql"
    return dbt == "postgresql"


def ensure_tables() -> None:
    global _tables_ready
    if _tables_ready:
        return
    for stmt in filter(str.strip, _DDL.split(";")):
        meta.execute(stmt)
    _tables_ready = True


# --------------------------------------------------------------------------- #
# statistics reads (warehouse pool, Postgres catalogs)                        #
# --------------------------------------------------------------------------- #


def _warehouse_is_postgres(database: str) -> bool:
    try:
        return pool.get_connection_pool(database).db_type == "postgresql"
    except Exception:
        return False


def _table_change_stats(database: str, schema: str) -> Dict[str, Dict[str, Any]]:
    """One cheap catalog read: live rows + modifications since analyze +
    last analyze time, for every table in *schema*. This is factors (a) and (b)
    of the validity score, captured without touching the data."""
    sql = """
        SELECT relname AS table_name,
               n_live_tup,
               n_mod_since_analyze,
               GREATEST(COALESCE(last_analyze, 'epoch'),
                        COALESCE(last_autoanalyze, 'epoch')) AS last_analyze
        FROM pg_stat_user_tables
        WHERE schemaname = %s
    """
    res = pool.execute_query(sql.replace("%s", f"'{_esc(schema)}'"), database)
    out: Dict[str, Dict[str, Any]] = {}
    for r in res.get("rows_objects", []):
        out[r["table_name"]] = {
            "row_count": _int(r.get("n_live_tup")),
            "mods_since_analyze": _int(r.get("n_mod_since_analyze")),
            "last_analyze": _iso(r.get("last_analyze")),
        }
    return out


def _column_distributions(database: str, schema: str) -> Dict[str, List[Dict[str, Any]]]:
    """Per-column value distribution straight from pg_stats — no data scan."""
    sql = f"""
        SELECT tablename, attname,
               n_distinct, null_frac, avg_width,
               most_common_vals::text  AS most_common_vals,
               most_common_freqs::text AS most_common_freqs,
               histogram_bounds::text  AS histogram_bounds
        FROM pg_stats
        WHERE schemaname = '{_esc(schema)}'
    """
    res = pool.execute_query(sql, database)
    by_table: Dict[str, List[Dict[str, Any]]] = {}
    for r in res.get("rows_objects", []):
        by_table.setdefault(r["tablename"], []).append({
            "column": r["attname"],
            "n_distinct": _num(r.get("n_distinct")),
            "null_frac": _num(r.get("null_frac")),
            "avg_width": _int(r.get("avg_width")),
            "most_common_vals": r.get("most_common_vals"),
            "most_common_freqs": r.get("most_common_freqs"),
            "histogram_bounds": r.get("histogram_bounds"),
        })
    return by_table


def _inferred_relationships(database: str, schema: str) -> List[Dict[str, str]]:
    """Declared foreign keys from pg_constraint. (Heuristic <t>_id inference is
    layered on top by the router when a question spans tables with no FK.)"""
    sql = f"""
        SELECT
            tc.table_name        AS from_table,
            kcu.column_name      AS from_column,
            ccu.table_name       AS to_table,
            ccu.column_name      AS to_column
        FROM information_schema.table_constraints tc
        JOIN information_schema.key_column_usage kcu
             ON tc.constraint_name = kcu.constraint_name
            AND tc.table_schema = kcu.table_schema
        JOIN information_schema.constraint_column_usage ccu
             ON ccu.constraint_name = tc.constraint_name
            AND ccu.table_schema = tc.table_schema
        WHERE tc.constraint_type = 'FOREIGN KEY'
          AND tc.table_schema = '{_esc(schema)}'
    """
    try:
        res = pool.execute_query(sql, database)
    except Exception:
        return []
    return [
        {
            "from": f"{schema}.{r['from_table']}.{r['from_column']}",
            "to": f"{schema}.{r['to_table']}.{r['to_column']}",
            "kind": "declared_fk",
        }
        for r in res.get("rows_objects", [])
    ]


def _query_pattern_frequency(database: str) -> Dict[str, int]:
    """How often each physical table has actually been queried, from the
    platform's own query_history.tables_used. Drives the usage-weighted decay
    (factor c): hot tables are profiled/refreshed more aggressively."""
    try:
        res = meta.query(
            "SELECT tables_used FROM query_history "
            "WHERE tables_used IS NOT NULL AND database_name = @param0",
            [database],
        )
    except Exception:
        # database_name column may not exist in every deployment — fall back
        try:
            res = meta.query("SELECT tables_used FROM query_history WHERE tables_used IS NOT NULL")
        except Exception:
            return {}
    freq: Dict[str, int] = {}
    for r in res.get("rows_objects", res.get("rows", [])):
        raw = r.get("tables_used") if isinstance(r, dict) else None
        if not raw:
            continue
        for t in _parse_tables_used(raw):
            freq[t] = freq.get(t, 0) + 1
    return freq


# --------------------------------------------------------------------------- #
# public: build / refresh the context representation                          #
# --------------------------------------------------------------------------- #


def build_context(database: str, schema: str = "public",
                  tables: Optional[List[str]] = None) -> Dict[str, Any]:
    """Profile *schema* in *database* and persist one snapshot row per table
    element and per column element. Returns a summary of what was captured."""
    if not is_supported() or not _warehouse_is_postgres(database):
        return {"supported": False, "reason": "context engine requires Postgres", "elements": 0}

    ensure_tables()
    now = _now()
    now_iso = now.isoformat()

    change = _table_change_stats(database, schema)
    dists = _column_distributions(database, schema)
    rels = _inferred_relationships(database, schema)
    freq = _query_pattern_frequency(database)

    target_tables = tables or sorted(set(list(change.keys()) + list(dists.keys())))
    n_elems = 0

    for tbl in target_tables:
        ch = change.get(tbl, {})
        usage = freq.get(tbl, 0) + freq.get(f"{schema}.{tbl}", 0)

        # table-level element: relationships + change counters + usage
        tprofile = {
            "kind": "table",
            "relationships": [r for r in rels if r["from"].startswith(f"{schema}.{tbl}.")
                              or r["to"].startswith(f"{schema}.{tbl}.")],
            "column_count": len(dists.get(tbl, [])),
        }
        _upsert_snapshot(database, schema, tbl, None, "table", tprofile, ch, usage, now_iso)
        n_elems += 1

        # column-level elements: distribution stats
        for col in dists.get(tbl, []):
            _upsert_snapshot(database, schema, tbl, col["column"], "column",
                             {"kind": "column", **col}, ch, usage, now_iso)
            n_elems += 1

    return {
        "supported": True,
        "database": database,
        "schema": schema,
        "tables_profiled": len(target_tables),
        "elements": n_elems,
        "relationships": len(rels),
        "captured_at": now_iso,
        "method": "pg_stats + pg_stat_user_tables + query_history (no LLM, no data scan)",
    }


def refresh_elements(database: str, element_keys: List[str]) -> int:
    """Re-capture change counters (and, for columns, distributions) for a set of
    elements after a live query proved them stale. Returns count refreshed.
    This closes the loop in claim element 5 (update validity to refreshed)."""
    if not element_keys:
        return 0
    ensure_tables()
    now_iso = _now().isoformat()

    # group requested elements by (schema, table) to minimise catalog reads
    by_schema: Dict[str, set] = {}
    for k in element_keys:
        parts = k.split(".")
        if len(parts) >= 2:
            by_schema.setdefault(parts[0], set()).add(parts[1])

    refreshed = 0
    for schema, tbls in by_schema.items():
        change = _table_change_stats(database, schema)
        dists = _column_distributions(database, schema)
        for tbl in tbls:
            ch = change.get(tbl, {})
            usage = _current_usage(database, schema, tbl)
            _touch_change(database, table_key(schema, tbl), ch, now_iso)
            refreshed += 1
            for col in dists.get(tbl, []):
                _upsert_snapshot(database, schema, tbl, col["column"], "column",
                                 {"kind": "column", **col}, ch, usage, now_iso)
                refreshed += 1
    return refreshed


# --------------------------------------------------------------------------- #
# snapshot persistence helpers                                                #
# --------------------------------------------------------------------------- #


def _upsert_snapshot(database: str, schema: str, table: str, column: Optional[str],
                     etype: str, profile: Dict[str, Any], change: Dict[str, Any],
                     usage: int, now_iso: str) -> None:
    key = column_key(schema, table, column) if column else table_key(schema, table)
    existing = meta.query_one(
        "SELECT id FROM context_snapshots WHERE database_name = @param0 AND element_key = @param1",
        [database, key],
    )
    row_count = change.get("row_count")
    mods = change.get("mods_since_analyze")
    last_an = change.get("last_analyze")
    profile_json = json.dumps(profile, default=str)

    if existing:
        meta.execute(
            "UPDATE context_snapshots SET profile = @param0, row_count = @param1, "
            "mods_at_capture = @param2, last_analyze = @param3, usage_count = @param4, "
            "refreshed_at = @param5 WHERE id = @param6",
            [profile_json, row_count, mods, last_an, usage, now_iso, existing["id"]],
        )
    else:
        meta.execute(
            "INSERT INTO context_snapshots (id, database_name, schema_name, table_name, "
            "column_name, element_key, element_type, profile, row_count, mods_at_capture, "
            "last_analyze, usage_count, captured_at, refreshed_at) VALUES "
            "(@param0,@param1,@param2,@param3,@param4,@param5,@param6,@param7,@param8,"
            "@param9,@param10,@param11,@param12,@param13)",
            [str(uuid.uuid4()), database, schema, table, column, key, etype,
             profile_json, row_count, mods, last_an, usage, now_iso, now_iso],
        )


def _touch_change(database: str, element_key: str, change: Dict[str, Any], now_iso: str) -> None:
    meta.execute(
        "UPDATE context_snapshots SET row_count = @param0, mods_at_capture = @param1, "
        "last_analyze = @param2, refreshed_at = @param3 "
        "WHERE database_name = @param4 AND element_key = @param5",
        [change.get("row_count"), change.get("mods_since_analyze"),
         change.get("last_analyze"), now_iso, database, element_key],
    )


def _current_usage(database: str, schema: str, table: str) -> int:
    row = meta.query_one(
        "SELECT usage_count FROM context_snapshots "
        "WHERE database_name = @param0 AND element_key = @param1",
        [database, table_key(schema, table)],
    )
    return _int(row.get("usage_count")) if row else 0


def load_snapshots(database: str, element_keys: Optional[List[str]] = None) -> Dict[str, Dict[str, Any]]:
    """Load stored snapshots keyed by element_key. If *element_keys* is given,
    only those are returned."""
    ensure_tables()
    if element_keys:
        placeholders = ",".join(f"@param{i+1}" for i in range(len(element_keys)))
        res = meta.query(
            f"SELECT * FROM context_snapshots WHERE database_name = @param0 "
            f"AND element_key IN ({placeholders})",
            [database, *element_keys],
        )
    else:
        res = meta.query(
            "SELECT * FROM context_snapshots WHERE database_name = @param0", [database]
        )
    out: Dict[str, Dict[str, Any]] = {}
    for r in res.get("rows_objects", res.get("rows", [])):
        if isinstance(r, dict):
            out[r["element_key"]] = r
    return out


# --------------------------------------------------------------------------- #
# small parse/coerce helpers                                                  #
# --------------------------------------------------------------------------- #


def _esc(s: str) -> str:
    return str(s).replace("'", "''")


def _parse_tables_used(raw: Any) -> List[str]:
    if isinstance(raw, list):
        return [str(x).lower() for x in raw]
    s = str(raw).strip()
    if s.startswith("["):
        try:
            return [str(x).lower() for x in json.loads(s)]
        except Exception:
            pass
    return [t.strip().lower() for t in s.replace(";", ",").split(",") if t.strip()]


def _int(v: Any) -> int:
    try:
        return int(v)
    except Exception:
        return 0


def _num(v: Any) -> float:
    try:
        return float(v)
    except Exception:
        return 0.0


def _iso(v: Any) -> Optional[str]:
    if v is None:
        return None
    if isinstance(v, datetime):
        return v.isoformat()
    return str(v)
