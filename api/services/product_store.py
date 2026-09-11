"""Typed client for KaveonDB's durable product-record transaction API.

PostgreSQL remains authoritative.  This module is the narrow application-side
boundary used by backfill, shadow reads, and a later repository cutover; it
does not enable dual writes by itself.
"""

import json
from dataclasses import dataclass
from typing import Iterable, Literal, Mapping, Optional
from urllib.parse import quote

from fastapi import HTTPException

from services import engine_bridge


ProductKind = Literal["dataset", "chart", "dashboard", "saved_query", "user_theme", "dlm_definition", "dlm_run", "favorite"]
_KINDS = {"dataset", "chart", "dashboard", "saved_query", "user_theme", "dlm_definition", "dlm_run", "favorite"}


@dataclass(frozen=True)
class ProductMutation:
    operation: Literal["create", "update", "delete"]
    kind: ProductKind
    record_id: str
    document: Optional[Mapping] = None
    expected_revision: Optional[int] = None


def _role(role: str) -> str:
    roles = {"Viewer": "reader", "Analyst": "analyst", "Editor": "analyst", "Admin": "admin"}
    try:
        return roles[role]
    except KeyError:
        raise HTTPException(403, "A recognized Kaveon role is required for product storage") from None


def _identifier(value: str, label: str) -> str:
    if not value or len(value) > 255 or any(ord(character) < 32 for character in value):
        raise HTTPException(422, f"Invalid product {label}")
    if any(character in value for character in ("/", "\\", "'")):
        raise HTTPException(422, f"Invalid product {label}")
    return value


def _sql_literal(value: str) -> str:
    return "'" + value.replace("'", "''") + "'"


def _statement(mutation: ProductMutation) -> str:
    if mutation.kind not in _KINDS:
        raise HTTPException(422, "Unsupported product record kind")
    record_id = _identifier(mutation.record_id, "record ID")
    table = mutation.kind + "s"
    if mutation.operation == "create":
        if mutation.document is None or mutation.expected_revision is not None:
            raise HTTPException(422, "Create requires a document and no expected revision")
        document = json.dumps(mutation.document, sort_keys=True, separators=(",", ":"), ensure_ascii=False)
        return (
            f"INSERT INTO kaveon.product.{table} (id, document_json) VALUES "
            f"({_sql_literal(record_id)}, {_sql_literal(document)})"
        )
    if mutation.operation == "update":
        if mutation.document is None or not mutation.expected_revision or mutation.expected_revision < 1:
            raise HTTPException(422, "Update requires a document and positive expected revision")
        document = json.dumps(mutation.document, sort_keys=True, separators=(",", ":"), ensure_ascii=False)
        return (
            f"UPDATE kaveon.product.{table} SET document_json = {_sql_literal(document)} "
            f"WHERE id = {_sql_literal(record_id)} AND revision = {mutation.expected_revision}"
        )
    if mutation.operation == "delete":
        if mutation.document is not None or not mutation.expected_revision or mutation.expected_revision < 1:
            raise HTTPException(422, "Delete requires a positive expected revision and no document")
        return (
            f"DELETE FROM kaveon.product.{table} WHERE id = {_sql_literal(record_id)} "
            f"AND revision = {mutation.expected_revision}"
        )
    raise HTTPException(422, "Unsupported product mutation")


def _request(sql: str, actor: str, role: str, transaction_id: Optional[str] = None):
    payload = {"sql": sql}
    if transaction_id:
        payload["transaction_id"] = transaction_id
    return engine_bridge._request(
        "POST",
        "/v1/transaction/sql",
        "KAVEON_ENGINE_BRIDGE_TOKEN",
        actor,
        payload=payload,
        role=_role(role),
    )


def transact(mutations: Iterable[ProductMutation], actor: str, role: str) -> dict:
    """Atomically apply a bounded group of typed product mutations."""
    staged = tuple(mutations)
    if not staged or len(staged) > 100:
        raise HTTPException(422, "A product transaction requires between 1 and 100 mutations")
    statements = tuple(_statement(mutation) for mutation in staged)
    begun = _request("BEGIN", actor, role)
    transaction_id = begun.get("transaction_id") if isinstance(begun, dict) else None
    if not transaction_id:
        raise HTTPException(502, "KaveonDB returned an invalid transaction session")
    try:
        for statement in statements:
            _request(statement, actor, role, transaction_id)
        return _request("COMMIT", actor, role, transaction_id)
    except Exception:
        try:
            _request("ROLLBACK", actor, role, transaction_id)
        except Exception:
            pass
        raise


def read(kind: ProductKind, record_id: str, actor: str, role: str) -> Optional[dict]:
    """Read one committed record from a pinned durable product snapshot."""
    if kind not in _KINDS:
        raise HTTPException(422, "Unsupported product record kind")
    record_id = _identifier(record_id, "record ID")
    return engine_bridge._request(
        "GET",
        f"/v1/product/{kind}/{quote(record_id, safe='')}",
        "KAVEON_ENGINE_BRIDGE_TOKEN",
        actor,
        role=_role(role),
    )
