"""
Metadata database query layer.
Direct port of metadataProxy.service.ts — same @param0 replacement logic.
"""

from typing import Any, List, Optional, TypeVar
from database.pool import execute_query
from config import settings

T = TypeVar("T")


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


def query(sql: str, params: Optional[List[Any]] = None) -> dict:
    """
    Execute *sql* (with @paramN placeholders) against the metadata DB.
    Returns {"rows": list[dict], "row_count": int}.
    """
    db = settings.FABRIC_METADATA_DATABASE
    if not db:
        raise RuntimeError("FABRIC_METADATA_DATABASE is not configured")

    processed = _replace_params(sql, params)
    result = execute_query(processed, db)

    # execute_query returns rows_objects already
    rows = result.get("rows_objects", [])
    return {"rows": rows, "row_count": result.get("row_count", len(rows))}


def query_one(sql: str, params: Optional[List[Any]] = None) -> Optional[dict]:
    result = query(sql, params)
    return result["rows"][0] if result["rows"] else None


def execute(sql: str, params: Optional[List[Any]] = None) -> int:
    """Execute a DML statement; returns affected row count."""
    result = query(sql, params)
    return result["row_count"]
