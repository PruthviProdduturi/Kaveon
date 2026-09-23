"""Typed read repository for KaveonDB system metadata.

This is deliberately separate from ``product_store``: product documents use
the compact document API, while control-plane tables are typed Engine rows.
All API repositories can depend on this boundary without knowing transport,
identity headers, pagination, or Engine response details.
"""

from __future__ import annotations

from typing import Any, Mapping, Optional
import json
from urllib.parse import quote

from fastapi import HTTPException

from services import engine_bridge


_TABLE = r"^[A-Za-z][A-Za-z0-9_]{0,62}$"
_ID_MAX = 255
_JSON_MAX_BYTES = 512 * 1024


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


def create_row(
    table: str,
    row_id: str,
    columns: dict[str, dict[str, Any]],
    actor: str,
    role: str,
    *,
    owner_principal: Optional[str] = None,
) -> dict:
    """Create one typed system row through the Engine transaction boundary.

    ``columns`` contains Engine ``TypedValue`` JSON objects (for example
    ``{"type": "string", "value": "..."}``).  The API never writes a
    system row with a direct filesystem or database connection.
    """
    table, row_id = _valid(table, "table"), _valid(row_id, "row ID")
    if not isinstance(columns, dict) or not columns:
        raise HTTPException(422, "System row columns are required")
    if any(not isinstance(key, str) or not key or len(key) > 128 for key in columns):
        raise HTTPException(422, "Invalid system row column")
    for value in columns.values():
        if not isinstance(value, dict) or value.get("type") not in {"null", "boolean", "integer", "string", "json"}:
            raise HTTPException(422, "System row columns must use Engine typed values")
        if value.get("type") == "json":
            try:
                encoded = json.dumps(value.get("value"), separators=(",", ":"), ensure_ascii=False)
            except (TypeError, ValueError) as error:
                raise HTTPException(422, "System row JSON value is invalid") from error
            if len(encoded.encode("utf-8")) > _JSON_MAX_BYTES:
                raise HTTPException(422, "System row JSON value exceeds the 512 KiB limit")
    owner = owner_principal or actor
    _valid(owner, "owner principal")
    document = {
        "table": table,
        "primary_key": row_id,
        "revision": 1,
        "columns": columns,
        "owner_principal": owner,
    }
    sql = (
        "INSERT INTO kaveon.product.typed_rows (id, document_json) VALUES ("
        + _sql_literal(row_id) + ", " + _sql_literal(json.dumps(document, sort_keys=True, separators=(",", ":"))) + ")"
    )
    begun = engine_bridge._request(
        "POST", "/v1/transaction/sql", "KAVEON_ENGINE_BRIDGE_TOKEN", actor,
        payload={"sql": "BEGIN"}, role=_role(role),
    )
    transaction_id = begun.get("transaction_id") if isinstance(begun, dict) else None
    if not transaction_id:
        raise HTTPException(502, "KaveonDB returned an invalid transaction session")
    try:
        staged = engine_bridge._request(
            "POST", "/v1/transaction/sql", "KAVEON_ENGINE_BRIDGE_TOKEN", actor,
            payload={"sql": sql, "transaction_id": transaction_id}, role="admin",
        )
        committed = engine_bridge._request(
            "POST", "/v1/transaction/sql", "KAVEON_ENGINE_BRIDGE_TOKEN", actor,
            payload={"sql": "COMMIT", "transaction_id": transaction_id}, role="admin",
        )
    except Exception:
        try:
            engine_bridge._request(
                "POST", "/v1/transaction/sql", "KAVEON_ENGINE_BRIDGE_TOKEN", actor,
                payload={"sql": "ROLLBACK", "transaction_id": transaction_id}, role="admin",
            )
        except Exception:
            pass
        raise
    return committed if isinstance(committed, dict) else (staged if isinstance(staged, dict) else {})


def update_row(
    table: str,
    row_id: str,
    columns: Mapping[str, Mapping[str, Any]],
    expected_revision: int,
    actor: str,
    role: str,
) -> dict:
    """CAS-update a typed system row through the Engine transaction boundary."""
    table, row_id = _valid(table, "table"), _valid(row_id, "row ID")
    if not isinstance(expected_revision, int) or expected_revision < 1:
        raise HTTPException(422, "System row revision must be positive")
    _validate_columns(columns)
    document = {"table": table, "primary_key": row_id, "revision": expected_revision + 1,
                "columns": columns, "owner_principal": actor}
    return _typed_row_mutation("UPDATE", row_id, document, actor, role, expected_revision)


def delete_row(
    table: str,
    row_id: str,
    expected_revision: int,
    actor: str,
    role: str,
) -> dict:
    """CAS-delete a typed system row through the Engine transaction boundary."""
    table, row_id = _valid(table, "table"), _valid(row_id, "row ID")
    if not isinstance(expected_revision, int) or expected_revision < 1:
        raise HTTPException(422, "System row revision must be positive")
    # The table is part of the predicate encoded in the typed-row document.
    # The Engine validates existence, ownership, and the expected revision.
    return _typed_row_mutation("DELETE", row_id, None, actor, role, expected_revision, table=table)


def _validate_columns(columns: Mapping[str, Mapping[str, Any]]) -> None:
    if not isinstance(columns, Mapping) or not columns:
        raise HTTPException(422, "System row columns are required")
    for key, value in columns.items():
        if not isinstance(key, str) or not key or len(key) > 128:
            raise HTTPException(422, "Invalid system row column")
        if not isinstance(value, Mapping) or value.get("type") not in {"null", "boolean", "integer", "string", "json"}:
            raise HTTPException(422, "System row columns must use Engine typed values")
        if value.get("type") == "json":
            try:
                encoded = json.dumps(value.get("value"), separators=(",", ":"), ensure_ascii=False)
            except (TypeError, ValueError) as error:
                raise HTTPException(422, "System row JSON value is invalid") from error
            if len(encoded.encode("utf-8")) > _JSON_MAX_BYTES:
                raise HTTPException(422, "System row JSON value exceeds the 512 KiB limit")


def _typed_row_mutation(operation: str, row_id: str, document: Optional[dict], actor: str,
                        role: str, expected_revision: int, *, table: Optional[str] = None) -> dict:
    if operation == "DELETE":
        sql = f"DELETE FROM kaveon.product.typed_rows WHERE id = {_sql_literal(row_id)} AND revision = {expected_revision}"
    else:
        payload = json.dumps(document, sort_keys=True, separators=(",", ":"), ensure_ascii=False)
        sql = ("UPDATE kaveon.product.typed_rows SET document_json = " + _sql_literal(payload) +
               f" WHERE id = {_sql_literal(row_id)} AND revision = {expected_revision}")
    begun = engine_bridge._request("POST", "/v1/transaction/sql", "KAVEON_ENGINE_BRIDGE_TOKEN", actor,
                                   payload={"sql": "BEGIN"}, role=_role(role))
    transaction_id = begun.get("transaction_id") if isinstance(begun, dict) else None
    if not transaction_id:
        raise HTTPException(502, "KaveonDB returned an invalid transaction session")
    try:
        staged = engine_bridge._request("POST", "/v1/transaction/sql", "KAVEON_ENGINE_BRIDGE_TOKEN", actor,
                                        payload={"sql": sql, "transaction_id": transaction_id}, role="admin")
        committed = engine_bridge._request("POST", "/v1/transaction/sql", "KAVEON_ENGINE_BRIDGE_TOKEN", actor,
                                           payload={"sql": "COMMIT", "transaction_id": transaction_id}, role="admin")
    except Exception:
        try:
            engine_bridge._request("POST", "/v1/transaction/sql", "KAVEON_ENGINE_BRIDGE_TOKEN", actor,
                                   payload={"sql": "ROLLBACK", "transaction_id": transaction_id}, role="admin")
        except Exception:
            pass
        raise
    return committed if isinstance(committed, dict) else (staged if isinstance(staged, dict) else {})


def _sql_literal(value: str) -> str:
    return "'" + value.replace("'", "''") + "'"
