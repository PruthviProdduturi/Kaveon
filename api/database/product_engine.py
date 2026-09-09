"""Private HTTP client for the Engine product transaction endpoint.

The Engine owns transaction execution. This module only serializes bound
parameters and forwards one complete transaction; it never persists state or
logs credentials or statement parameters.
"""

import json
import os
import uuid
from datetime import date, datetime
from typing import Any, Iterable

import httpx
from fastapi import HTTPException
from services.engine_bridge import _endpoint, _verify_context


def _json_value(value: Any) -> Any:
    if value is None or isinstance(value, (bool, int, float, str)):
        return value
    if isinstance(value, (datetime, date)):
        return value.isoformat()
    raise ValueError("product transaction parameters must be JSON scalars or result references")


def _parameter(value: Any, statement_index: int) -> Any:
    if isinstance(value, dict):
        reference = value.get("$result")
        if set(value) != {"$result"} or not isinstance(reference, dict):
            raise ValueError("invalid product transaction result reference")
        if set(reference) != {"statement", "column"}:
            raise ValueError("result references require statement and column")
        if not isinstance(reference["statement"], int) or reference["statement"] < 0 or reference["statement"] >= statement_index:
            raise ValueError("result references may only target prior statements")
        if not isinstance(reference["column"], str) or not reference["column"]:
            raise ValueError("result reference column is required")
        return {"$result": {"statement": reference["statement"], "column": reference["column"]}}
    return _json_value(value)


def _read_only(sql: str) -> bool:
    """Conservative classifier: CTEs and unknown syntax always take write path."""
    stripped = sql.lstrip()
    while stripped.startswith("--"):
        _, _, stripped = stripped.partition("\n")
        stripped = stripped.lstrip()
    keyword = stripped.split(None, 1)[0].upper() if stripped else ""
    return keyword in {"SELECT", "EXPLAIN", "PRAGMA"}


def transaction(statements: Iterable[dict], *, read_only: bool | None = None, operation_id: str | None = None) -> list[dict]:
    token = os.getenv("KAVEON_ENGINE_CATALOG_TOKEN")
    if not token:
        raise HTTPException(503, "Engine service credential is not configured")

    encoded = []
    for index, statement in enumerate(statements):
        sql = statement.get("sql") if isinstance(statement, dict) else None
        params = statement.get("params", []) if isinstance(statement, dict) else None
        if not isinstance(sql, str) or not sql.strip() or not isinstance(params, list):
            raise ValueError("product transaction statements require sql and a parameter array")
        encoded.append({"sql": sql, "params": [_parameter(value, index) for value in params]})
    if not encoded:
        return []

    payload = {
        "statements": encoded,
        "read_only": all(_read_only(item["sql"]) for item in encoded) if read_only is None else bool(read_only),
        "operation_id": operation_id or str(uuid.uuid4()),
    }
    try:
        response = httpx.post(
            _endpoint() + "/v1/product/transaction",
            headers={"Authorization": f"Bearer {token}"},
            json=payload,
            timeout=60,
            follow_redirects=False,
            verify=_verify_context(),
        )
    except httpx.HTTPError:
        raise HTTPException(502, "Engine is unavailable") from None
    if response.status_code in {409, 412}:
        raise HTTPException(409, "Engine product transaction conflict; retry") from None
    if not response.is_success:
        raise HTTPException(502, "Engine rejected the product transaction") from None
    body = response.json()
    results = body.get("results") if isinstance(body, dict) else None
    if not isinstance(results, list) or len(results) != len(encoded):
        raise HTTPException(502, "Engine returned an invalid product transaction response")
    normalized = []
    for result in results:
        if not isinstance(result, dict) or not isinstance(result.get("rows", []), list):
            raise HTTPException(502, "Engine returned an invalid product transaction result")
        normalized.append({"rows": result.get("rows", []), "row_count": result.get("row_count", len(result.get("rows", [])))})
    return normalized
