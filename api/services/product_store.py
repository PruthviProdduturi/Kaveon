"""Typed client for KaveonDB's durable product-record transaction API.

PostgreSQL remains authoritative.  This module is the narrow application-side
boundary used by backfill, shadow reads, and a later repository cutover; it
does not enable dual writes by itself.
"""

import json
import os
import re
from dataclasses import dataclass
from typing import Iterable, Literal, Mapping, Optional
from urllib.parse import quote

from fastapi import HTTPException

from services import engine_bridge


ProductKind = Literal["dataset", "chart", "dashboard", "saved_query", "user_theme", "dlm_definition", "dlm_run", "favorite", "source", "user_recent", "query_history", "activity", "chat_session", "chat_message"]
_KINDS = {"dataset", "chart", "dashboard", "saved_query", "user_theme", "dlm_definition", "dlm_run", "favorite", "source", "user_recent", "query_history", "activity", "chat_session", "chat_message"}
_MIGRATION_PRINCIPAL = re.compile(r"^[A-Za-z0-9@._+\-]{1,255}$")

# The owner the platform stamps on content it seeds — the demo dashboards,
# their charts and their dataset. It is a sentinel, not a person: nobody holds
# that identity and nobody can sign in as it.
SEEDED_OWNER = "system"
# Roles the API already lets edit content belonging to someone else; see
# middleware.permissions.can_write, which is the authority for that decision.
_STEWARD_ROLES = frozenset({"Editor", "Admin"})


def writer(owner: Optional[str], actor: str, role: str) -> str:
    """The principal a product write is made as.

    KaveonDB requires a record's owner to be the principal writing it, and —
    unlike its read path — grants no exception to an Admin. The API's own rule
    is the opposite: `can_write` lets an Editor or an Admin edit anyone's
    content. Nothing reconciled the two, so content seeded under the `system`
    sentinel could be authorized by the API and then refused by the Engine,
    which is why saving one of the seeded dashboards failed with "product
    record owner does not match the authenticated principal". Seeded content
    is owned by nobody, so that made it permanently uneditable.

    Authorization has already happened by the time a caller reaches here, so
    this decides attribution only: a write an Editor or Admin makes to seeded
    content is made as the sentinel, which both satisfies the Engine and
    leaves the content seeded rather than quietly transferring it to whoever
    edited it first.

    Deliberately narrow. Content belonging to a real person is still written
    as the caller, so the Engine goes on enforcing that one user cannot write
    another's records and this cannot become a general impersonation path.
    """
    if owner == SEEDED_OWNER and role in _STEWARD_ROLES:
        return SEEDED_OWNER
    return actor


def _migration_actor(default_actor: str) -> str:
    """Return an explicitly allowlisted actor for offline backfill calls only."""
    override = os.getenv("KAVEON_MIGRATION_OWNER_PRINCIPAL", "").strip()
    if not override:
        return default_actor
    allowed = {
        value.strip()
        for value in os.getenv("KAVEON_MIGRATION_OWNER_ALLOWLIST", "").split(",")
        if value.strip()
    }
    if not _MIGRATION_PRINCIPAL.fullmatch(override) or override not in allowed:
        raise RuntimeError("migration owner principal is invalid or not allowlisted")
    return override


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

_PRODUCT_TABLES = {
    "dataset": "datasets", "chart": "charts", "dashboard": "dashboards",
    "saved_query": "saved_queries", "user_theme": "user_themes",
    "dlm_definition": "dlm_definitions", "dlm_run": "dlm_runs",
    "favorite": "favorites", "source": "sources", "user_recent": "user_recents",
    "query_history": "query_history", "activity": "activity",
    "chat_session": "chat_sessions", "chat_message": "chat_messages",
}


def _statement(mutation: ProductMutation) -> str:
    if mutation.kind not in _KINDS:
        raise HTTPException(422, "Unsupported product record kind")
    record_id = _identifier(mutation.record_id, "record ID")
    table = _PRODUCT_TABLES[mutation.kind]
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


def migration_transact(mutations: Iterable[ProductMutation], actor: str, role: str) -> dict:
    """Backfill-only CAS repair; creates retain their canonical record owner."""
    staged = tuple(mutations)
    selected_actor = actor if staged and all(item.operation == "create" for item in staged) \
        else _migration_actor(actor)
    return transact(staged, selected_actor, role)


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


def migration_read(kind: ProductKind, record_id: str, actor: str, role: str) -> Optional[dict]:
    """Backfill-only read using the same bounded owner override as writes."""
    return read(kind, record_id, _migration_actor(actor), role)


def list_records(kind: ProductKind, actor: str, role: str, *, max_records: int = 1000) -> list[dict]:
    """Read a bounded, snapshot-pinned product family without source fallback."""
    if kind not in _KINDS or max_records < 1 or max_records > 1000:
        raise HTTPException(422, "Invalid product list request")
    records, cursor, snapshot_id = [], None, None
    while True:
        query = "?limit=100" + (("&cursor=" + quote(cursor, safe="")) if cursor else "")
        page = engine_bridge._request("GET", f"/v1/products/{kind}{query}",
            "KAVEON_ENGINE_BRIDGE_TOKEN", actor, role=_role(role))
        if not isinstance(page, dict) or not isinstance(page.get("records"), list):
            raise HTTPException(502, "KaveonDB returned an invalid product list")
        current_snapshot = page.get("snapshot_id")
        if not isinstance(current_snapshot, str) or not current_snapshot:
            raise HTTPException(502, "KaveonDB product list omitted its snapshot identity")
        if snapshot_id is not None and current_snapshot != snapshot_id:
            raise HTTPException(409, "KaveonDB product list changed during pagination")
        snapshot_id = current_snapshot
        for record in page["records"]:
            if not isinstance(record, dict) or not isinstance(record.get("document"), dict):
                raise HTTPException(502, "KaveonDB returned an invalid product list record")
            records.append(record)
            if len(records) > max_records:
                raise HTTPException(503, "KaveonDB product list exceeds its configured bound")
        cursor = page.get("next_cursor")
        if cursor is None: return records
        if not isinstance(cursor, str) or not cursor:
            raise HTTPException(502, "KaveonDB returned an invalid product list cursor")


def list_records_snapshot(kind: ProductKind, actor: str, role: str, *, max_records: int = 1000) -> tuple[list[dict], str]:
    """Return records plus the exact snapshot identity used by every page."""
    if kind not in _KINDS or max_records < 1 or max_records > 1000:
        raise HTTPException(422, "Invalid product list request")
    records, cursor, snapshot_id = [], None, None
    while True:
        query = "?limit=100" + (("&cursor=" + quote(cursor, safe="")) if cursor else "")
        page = engine_bridge._request("GET", f"/v1/products/{kind}{query}",
            "KAVEON_ENGINE_BRIDGE_TOKEN", actor, role=_role(role))
        if not isinstance(page, dict) or not isinstance(page.get("records"), list):
            raise HTTPException(502, "KaveonDB returned an invalid product list")
        current = page.get("snapshot_id")
        if not isinstance(current, str) or not current or (snapshot_id is not None and current != snapshot_id):
            raise HTTPException(409, "KaveonDB product list changed during pagination")
        snapshot_id = current
        for record in page["records"]:
            if not isinstance(record, dict) or not isinstance(record.get("document"), dict):
                raise HTTPException(502, "KaveonDB returned an invalid product list record")
            records.append(record)
            if len(records) > max_records:
                raise HTTPException(503, "KaveonDB product list exceeds its configured bound")
        cursor = page.get("next_cursor")
        if cursor is None: return records, snapshot_id
        if not isinstance(cursor, str) or not cursor:
            raise HTTPException(502, "KaveonDB returned an invalid product list cursor")


# The Engine answers a product family as documents, so a cardinality is paid
# for by reading them. The bound keeps an administrative reading of the
# control plane from turning into an unbounded transfer; a family past it is
# reported as counted-to-the-bound rather than guessed at or refused.
COUNT_BOUND = 1000


def count_records(kind: ProductKind, actor: str, role: str, *, bound: int = COUNT_BOUND) -> dict:
    """How many records one product family holds, read from one pinned snapshot.

    Returns ``{"records": int, "truncated": bool, "snapshotId": str}``.
    ``truncated`` is true when the family holds more than ``bound`` records, in
    which case ``records`` is exactly ``bound`` and the reading is a floor.
    """
    if kind not in _KINDS:
        raise HTTPException(422, "Unsupported product record kind")
    if not isinstance(bound, int) or bound < 1 or bound > COUNT_BOUND:
        raise HTTPException(422, "Invalid product count bound")
    counted, cursor, snapshot_id = 0, None, None
    while True:
        query = "?limit=100" + (("&cursor=" + quote(cursor, safe="")) if cursor else "")
        page = engine_bridge._request("GET", f"/v1/products/{kind}{query}",
            "KAVEON_ENGINE_BRIDGE_TOKEN", actor, role=_role(role))
        if not isinstance(page, dict) or not isinstance(page.get("records"), list):
            raise HTTPException(502, "KaveonDB returned an invalid product list")
        current = page.get("snapshot_id")
        if not isinstance(current, str) or not current:
            raise HTTPException(502, "KaveonDB product list omitted its snapshot identity")
        if snapshot_id is not None and current != snapshot_id:
            raise HTTPException(409, "KaveonDB product list changed during pagination")
        snapshot_id = current
        counted += len(page["records"])
        if counted >= bound:
            return {"records": bound, "truncated": counted > bound or page.get("next_cursor") is not None,
                    "snapshotId": snapshot_id}
        cursor = page.get("next_cursor")
        if cursor is None:
            return {"records": counted, "truncated": False, "snapshotId": snapshot_id}
        if not isinstance(cursor, str):
            raise HTTPException(502, "KaveonDB returned an invalid product list cursor")
