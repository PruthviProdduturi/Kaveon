"""
Metadata database query layer.
Direct port of metadataProxy.service.ts — same @param0 replacement logic.
Includes lightweight T-SQL → ANSI dialect translation for non-MSSQL backends.
"""

import os
import re
from typing import Any, List, Optional, TypeVar
from database.pool import execute_query
from config import settings

T = TypeVar("T")


# ── Dialect translation ────────────────────────────────────────────────────────

def _adapt_sql(sql: str) -> str:
    """Translate T-SQL idioms to the target dialect when the metadata DB is not MSSQL."""
    db_type = os.environ.get("FABRIC_METADATA_DB_TYPE") or settings.FABRIC_METADATA_DB_TYPE or "fabric_sql"
    if db_type in ("fabric_sql", "azure_sql"):
        return sql  # native T-SQL, no changes needed

    # Strip dbo. schema prefix (SQL Server specific)
    sql = re.sub(r"\bdbo\.", "", sql, flags=re.IGNORECASE)

    # SELECT TOP N → SELECT … LIMIT N
    top_match = re.search(r"\bSELECT\s+TOP\s+(\d+)\b", sql, re.IGNORECASE)
    if top_match:
        n = top_match.group(1)
        sql = re.sub(r"\bSELECT\s+TOP\s+\d+\b\s*", "SELECT ", sql, count=1, flags=re.IGNORECASE)
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


# ── Parameter interpolation ────────────────────────────────────────────────────

def _replace_params(sql: str, params: Optional[List[Any]]) -> str:
    """
    Replace @param0, @param1 … in *sql* with properly escaped literals.
    Replacement is done in REVERSE order to avoid @param1 matching inside @param10.
    """
    if not params:
        return sql

    processed = sql
    for i in range(len(params) - 1, -1, -1):
        param = params[i]
        placeholder = f"@param{i}"

        if param is None:
            value = "NULL"
        elif isinstance(param, str):
            value = f"'{param.replace(chr(39), chr(39) * 2)}'"
        elif isinstance(param, bool):
            value = "1" if param else "0"
        elif isinstance(param, (int, float)):
            value = str(param)
        else:
            import json as _json
            from datetime import datetime, date
            if isinstance(param, (datetime, date)):
                value = f"'{param.isoformat()}'"
            else:
                dumped = _json.dumps(param, ensure_ascii=False)
                value = f"'{dumped.replace(chr(39), chr(39) * 2)}'"

        processed = processed.replace(placeholder, value)

    return processed


# ── Public API ─────────────────────────────────────────────────────────────────

def query(sql: str, params: Optional[List[Any]] = None) -> dict:
    """
    Execute *sql* (with @paramN placeholders) against the metadata DB.
    Returns {"rows": list[dict], "row_count": int}.
    """
    db = settings.FABRIC_METADATA_DATABASE
    if not db:
        raise RuntimeError("FABRIC_METADATA_DATABASE is not configured")

    processed = _adapt_sql(_replace_params(sql, params))
    result = execute_query(processed, db)

    rows = result.get("rows_objects", [])
    return {"rows": rows, "row_count": result.get("row_count", len(rows))}


def query_one(sql: str, params: Optional[List[Any]] = None) -> Optional[dict]:
    result = query(sql, params)
    return result["rows"][0] if result["rows"] else None


def execute(sql: str, params: Optional[List[Any]] = None) -> int:
    """Execute a DML statement; returns affected row count."""
    result = query(sql, params)
    return result["row_count"]
