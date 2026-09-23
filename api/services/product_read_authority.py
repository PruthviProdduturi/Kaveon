"""Explicit, family-scoped KaveonDB read-authority cutover."""

import hashlib
import os
from typing import Optional

from services import product_store


ENVIRONMENT_KEY = "KAVEONDB_READ_AUTHORITY_FAMILIES"
ALL_FAMILIES_TOKEN = "all"
SUPPORTED_FAMILIES = frozenset({
    "datasets", "charts", "dashboards", "saved_queries", "user_themes",
    "user_recents", "favorites", "query_history", "activity", "chat_history",
    "sources", "dlm_definitions", "dlm_runs",
})
_KIND = {
    "datasets": "dataset", "charts": "chart", "dashboards": "dashboard",
    "saved_queries": "saved_query", "user_themes": "user_theme",
    "user_recents": "user_recent", "favorites": "favorite",
    "query_history": "query_history", "activity": "activity",
    "sources": "source", "dlm_definitions": "dlm_definition", "dlm_runs": "dlm_run",
}
_OWNER_FAMILIES = {"saved_queries", "user_themes", "user_recents", "favorites", "query_history"}


class ProductReadAuthorityError(RuntimeError):
    """A cutover family could not be read safely from KaveonDB."""


def enabled(family: str) -> bool:
    """Return whether *family* has explicitly moved; reject unknown config."""
    if family not in SUPPORTED_FAMILIES:
        raise ProductReadAuthorityError(f"unsupported KaveonDB read-authority family: {family}")
    raw = os.getenv(ENVIRONMENT_KEY, "").strip()
    if not raw:
        return False
    configured = {item.strip().lower() for item in raw.split(",") if item.strip()}
    # ``all`` is an explicit deployment switch for the complete product
    # repository set.  It deliberately expands only families implemented by
    # product_store; control-plane families remain fail-closed until their
    # Engine schemas and replay evidence exist.
    if ALL_FAMILIES_TOKEN in configured:
        configured.remove(ALL_FAMILIES_TOKEN)
        configured.update(SUPPORTED_FAMILIES)
    unknown = sorted(configured - SUPPORTED_FAMILIES)
    if unknown:
        raise ProductReadAuthorityError(
            "unknown KaveonDB read-authority families: " + ", ".join(unknown)
        )
    return family in configured


def read_document(
    family: str,
    record_id: str,
    actor: Optional[str],
    role: str,
) -> Optional[dict]:
    """Read one cutover record without any PostgreSQL fallback.

    KaveonDB is queried with the caller identity and an admin bridge role so the
    API can apply product visibility rules consistently with the legacy query.
    """
    if not enabled(family):
        raise ProductReadAuthorityError(f"{family} has not moved to KaveonDB read authority")
    principal = actor or "kaveon-system"
    target = product_store.read(_KIND[family], str(record_id), principal, "Admin")
    if target is None:
        return None
    document = target.get("document") if isinstance(target, dict) else None
    if not isinstance(document, dict):
        raise ProductReadAuthorityError(f"KaveonDB returned an invalid {family} document")
    if family in _OWNER_FAMILIES:
        owner = document.get("user_email", document.get("created_by"))
        if role != "Admin" and owner != actor:
            return None
        return dict(document)
    if family in {"sources", "dlm_definitions", "dlm_runs", "activity"}:
        if family == "activity" and role != "Admin" and document.get("user_email") != actor:
            return None
        return dict(document)
    visibility = document.get("visibility") or "internal"
    owner = document.get("created_by") or document.get("owner")
    permitted = (
        role == "Admin"
        or visibility == "published"
        or visibility == "internal" and role in {"Analyst", "Editor"}
        or visibility == "private" and bool(actor) and owner == actor
    )
    if actor is not None and not permitted:
        return None
    result = dict(document)
    if actor and family in {"datasets", "charts", "dashboards", "saved_queries"}:
        favorite_id = hashlib.sha256(
            f"{actor}\0{_KIND[family]}\0{result.get('id')}".encode()
        ).hexdigest()
        result["favorite"] = product_store.read("favorite", favorite_id, actor, "Admin") is not None
    else:
        result["favorite"] = False
    return result


def list_documents(family: str, actor: str, role: str) -> list[dict]:
    """List one cutover family from KaveonDB with legacy visibility and order."""
    if not enabled(family):
        raise ProductReadAuthorityError(f"{family} has not moved to KaveonDB read authority")
    if family == "chat_history":
        raise ProductReadAuthorityError("chat history requires its typed session or message list")
    records = product_store.list_records(_KIND[family], actor, "Admin")
    favorites = product_store.list_records("favorite", actor, "Admin") if family in {
        "datasets", "charts", "dashboards", "saved_queries", "sources"
    } else []
    favorite_ids = {
        (str(document.get("object_type")), str(document.get("object_id")))
        for record in favorites
        if isinstance(record, dict) and isinstance((document := record.get("document")), dict)
        and document.get("user_email") == actor
    }
    documents = []
    for record in records:
        document = record.get("document") if isinstance(record, dict) else None
        if not isinstance(document, dict):
            raise ProductReadAuthorityError(f"KaveonDB returned an invalid {family} document")
        if family in _OWNER_FAMILIES and role != "Admin" and document.get("user_email", document.get("created_by")) != actor:
            continue
        if family == "activity" and role != "Admin" and document.get("user_email") != actor:
            continue
        if family in {"sources", "dlm_definitions", "dlm_runs", "activity"}:
            item = dict(document)
            identity = str(item.get("source_id") if family == "sources" else item.get("id"))
            item["favorite"] = ("source", identity) in favorite_ids if family == "sources" else False
            documents.append(item); continue
        visibility = document.get("visibility") or ("private" if family in _OWNER_FAMILIES else "internal")
        owner = document.get("created_by") or document.get("owner")
        if not (role == "Admin" or visibility == "published"
            or visibility == "internal" and role in {"Analyst", "Editor"}
            or visibility == "private" and owner == actor): continue
        item = dict(document)
        item["favorite"] = (_KIND[family], str(item.get("id"))) in favorite_ids
        documents.append(item)
    def modified(item: dict) -> str:
        return str(item.get("updated_at") or item.get("modified_at")
                   or item.get("executed_at") or item.get("created_at") or "")
    return sorted(documents, key=lambda item: (modified(item), str(item.get("id") or "")), reverse=True)


def list_typed(kind: str, family: str, actor: str, role: str) -> list[dict]:
    """Return owner-scoped documents for a multi-kind family such as chat."""
    if not enabled(family) or family != "chat_history" or kind not in {"chat_session", "chat_message"}:
        raise ProductReadAuthorityError("unsupported typed read-authority list")
    result = []
    for record in product_store.list_records(kind, actor, "Admin"):
        document = record.get("document") if isinstance(record, dict) else None
        if not isinstance(document, dict):
            raise ProductReadAuthorityError("KaveonDB returned an invalid chat document")
        if document.get("user_email") == actor: result.append(dict(document))
    return result


def read_typed(kind: str, family: str, record_id: str, actor: str, role: str) -> Optional[dict]:
    if not enabled(family) or family != "chat_history" or kind not in {"chat_session", "chat_message"}:
        raise ProductReadAuthorityError("unsupported typed read-authority point read")
    target = product_store.read(kind, record_id, actor, "Admin")
    if target is None: return None
    document = target.get("document") if isinstance(target, dict) else None
    if not isinstance(document, dict):
        raise ProductReadAuthorityError("KaveonDB returned an invalid chat document")
    if role != "Admin" and document.get("user_email") != actor: return None
    return dict(document)
