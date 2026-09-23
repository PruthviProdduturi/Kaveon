"""Typed read repository for KaveonDB system metadata.

This is deliberately separate from ``product_store``: product documents use
the compact document API, while control-plane tables are typed Engine rows.
All API repositories can depend on this boundary without knowing transport,
identity headers, pagination, or Engine response details.
"""

from __future__ import annotations

from typing import Any, Optional
from urllib.parse import quote

from fastapi import HTTPException

from services import engine_bridge


_TABLE = r"^[A-Za-z][A-Za-z0-9_]{0,62}$"
_ID_MAX = 255


def _valid(value: str, label: str) -> str:
    import re

    if not isinstance(value, str) or not value or len(value) > _ID_MAX:
        raise HTTPException(422, f"Invalid system {label}")
    if label == "table":
        if not re.fullmatch(_TABLE, value):
            raise HTTPException(422, "Invalid system table")
    elif any(ord(c) < 32 for c in value) or "/" in value or "\\" in value:
        raise HTTPException(422, f"Invalid system {label}")
    return value


def _role(role: str) -> str:
    # System metadata is an administrative surface.  The Engine enforces this
    # independently of the API so a misconfigured route cannot expose it.
    if role != "Admin":
        raise HTTPException(403, "System metadata requires the Admin role")
    return "admin"


def _row(value: Any) -> dict:
    if not isinstance(value, dict) or not isinstance(value.get("columns"), dict):
        raise HTTPException(502, "KaveonDB returned an invalid system row")
    return {
        "table": value.get("table"),
        "id": value.get("id"),
        "revision": value.get("revision"),
        "generation": value.get("generation"),
        "snapshot_id": value.get("snapshot_id"),
        "columns": value["columns"],
    }


def read_row(table: str, row_id: str, actor: str, role: str) -> Optional[dict]:
    """Read one committed system row, or ``None`` when it does not exist."""
    table, row_id = _valid(table, "table"), _valid(row_id, "row ID")
    result = engine_bridge._request(
        "GET", f"/v1/system/{quote(table, safe='')}/{quote(row_id, safe='')}",
        "KAVEON_ENGINE_BRIDGE_TOKEN", actor, role=_role(role),
    )
    return None if result is None else _row(result)


def list_rows(
    table: str,
    actor: str,
    role: str,
    *,
    limit: int = 1000,
    cursor: Optional[str] = None,
) -> dict:
    """Return one bounded, snapshot-pinned page of typed system rows."""
    table = _valid(table, "table")
    if not isinstance(limit, int) or limit < 1 or limit > 1000:
        raise HTTPException(422, "System row page limit must be between 1 and 1000")
    if cursor is not None:
        cursor = _valid(cursor, "cursor")
    query = f"?limit={limit}" + (f"&cursor={quote(cursor, safe='')}" if cursor else "")
    result = engine_bridge._request(
        "GET", f"/v1/system/{quote(table, safe='')}{query}",
        "KAVEON_ENGINE_BRIDGE_TOKEN", actor, role=_role(role),
    )
    if not isinstance(result, dict) or not isinstance(result.get("rows"), list):
        raise HTTPException(502, "KaveonDB returned an invalid system row page")
    snapshot_id = result.get("snapshot_id")
    if not isinstance(snapshot_id, str) or not snapshot_id:
        raise HTTPException(502, "KaveonDB system row page omitted its snapshot identity")
    rows = [_row(item) for item in result["rows"]]
    next_cursor = result.get("next_cursor")
    if next_cursor is not None and (not isinstance(next_cursor, str) or not next_cursor):
        raise HTTPException(502, "KaveonDB returned an invalid system row cursor")
    return {
        "generation": result.get("generation"),
        "snapshot_id": snapshot_id,
        "rows": rows,
        "next_cursor": next_cursor,
    }
