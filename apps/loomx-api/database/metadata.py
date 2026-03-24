"""
Metadata database query layer.
Direct port of metadataProxy.service.ts — same @paramN replacement logic.
Includes lightweight T-SQL → ANSI dialect translation for non-MSSQL backends.

Security: parameters are passed to the driver via native placeholders (? / %s)
rather than string interpolation. The driver handles escaping.
"""

import re
from typing import Any, List, Optional, TypeVar
from database.pool import execute_query
from config import settings

T = TypeVar("T")


# ── Dialect translation ────────────────────────────────────────────────────────

def _adapt_sql(sql: str, db_type: str) -> str:
    """Translate T-SQL idioms to the target dialect when the metadata DB is not MSSQL."""
    if db_type in ("fabric_sql", "azure_sql"):
        return sql  # native T-SQL, no changes needed

    # Strip dbo. schema prefix (SQL Server specific)
    sql = re.sub(r"\bdbo\.", "", sql, flags=re.IGNORECASE)

    # SELECT TOP N / TOP (@paramN) → SELECT … LIMIT N / LIMIT @paramN
    top_match = re.search(r"\bSELECT\s+TOP\s+\(?([^\s)]+)\)?\b", sql, re.IGNORECASE)
    if top_match:
        n = top_match.group(1)
        sql = re.sub(r"\bSELECT\s+TOP\s+\(?[^\s)]+\)?\s*", "SELECT ", sql, count=1, flags=re.IGNORECASE)
        sql = sql.rstrip().rstrip(";") + f" LIMIT {n}"

    # GETDATE() → NOW()
    sql = re.sub(r"\bGETDATE\(\)", "NOW()", sql, flags=re.IGNORECASE)

    # ISNULL(x, y) → COALESCE(x, y)
    sql = re.sub(r"\bISNULL\(", "COALESCE(", sql, flags=re.IGNORECASE)

    # [bracket_identifier] → "double_quote" (PostgreSQL) or `backtick` (MySQL)
    if db_type == "postgresql":
        sql = re.sub(r"\[([^\]]+)\]", r'"\1"', sql)
    elif db_type == "mysql":
        sql = re.sub(r"\[([^\]]+)\]", r"`\1`", sql)

    return sql


# ── Native parameter conversion ────────────────────────────────────────────────

def _to_native_params(sql: str, params: Optional[List[Any]], db_type: str) -> tuple:
    """
    Replace @paramN placeholders with driver-native ? (MSSQL) or %s (PostgreSQL/MySQL).

    Scans left-to-right so the returned params list matches placeholder order.
    Values are passed as Python objects — the driver handles type-safe escaping.

    Returns (converted_sql, ordered_params_list).
    """
    if not params:
        return sql, []

    placeholder = "?" if db_type in ("fabric_sql", "azure_sql") else "%s"
    ordered: List[Any] = []

    def _sub(m: re.Match) -> str:
        idx = int(m.group(1))
        ordered.append(params[idx] if idx < len(params) else None)
        return placeholder

    converted = re.sub(r"@param(\d+)", _sub, sql, flags=re.IGNORECASE)
    return converted, ordered


# ── Public API ─────────────────────────────────────────────────────────────────

def query(sql: str, params: Optional[List[Any]] = None) -> dict:
    """
    Execute *sql* (with @paramN placeholders) against the metadata DB.
    Parameters are passed to the DB driver directly — no string interpolation.
    Returns {"rows": list[dict], "row_count": int}.
    """
    import os as _os
    # Read from os.environ directly — settings is frozen at import time and will
    # hold stale values if the .env was changed after startup.
    db = _os.environ.get("METADATA_DATABASE") or settings.METADATA_DATABASE
    if not db:
        from fastapi import HTTPException
        raise HTTPException(
            status_code=503,
            detail={"code": "setup_required", "message": "Metadata database is not configured. Please complete setup."},
        )

    db_type = _os.environ.get("METADATA_DB_TYPE") or settings.METADATA_DB_TYPE or "fabric_sql"
    adapted = _adapt_sql(sql, db_type)
    native_sql, native_params = _to_native_params(adapted, params, db_type)

    result = execute_query(native_sql, db, native_params or None)

    rows = result.get("rows_objects", [])
    return {"rows": rows, "row_count": result.get("row_count", len(rows))}


def query_one(sql: str, params: Optional[List[Any]] = None) -> Optional[dict]:
    result = query(sql, params)
    return result["rows"][0] if result["rows"] else None


def execute(sql: str, params: Optional[List[Any]] = None) -> int:
    """Execute a DML statement; returns affected row count."""
    result = query(sql, params)
    return result["row_count"]
