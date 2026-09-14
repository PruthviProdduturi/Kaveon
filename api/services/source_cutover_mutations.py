"""Atomic KaveonDB source metadata and activity mutations."""

import json
import secrets
import uuid
from datetime import datetime, timezone

from services import product_read_authority, product_store, source_mutations
from services.activity_backfill import document as activity_document


def enabled() -> bool:
    selected = product_read_authority.enabled("sources")
    if selected and not product_read_authority.enabled("activity"):
        raise RuntimeError("source cutover requires activity cutover")
    return selected


def new_catalog_id() -> str:
    return str(uuid.uuid4())


def new_data_id() -> str:
    return str(secrets.randbelow(9_000_000_000_000_000_000) + 1)


def _current(record_id: str, actor: str):
    record = product_store.read("source", record_id, actor, "Admin")
    if record is None:
        return None
    if (not isinstance(record.get("document"), dict)
            or not isinstance(record.get("revision"), int) or record["revision"] < 1):
        raise RuntimeError("KaveonDB returned an invalid source record")
    return record


def _audit(action: str, source_id: str, name: str, actor: str, details=None):
    event_id = str(uuid.uuid4())
    row = {"id": event_id, "action": action, "object_type": "catalog_source" if source_id.startswith("catalog-") else "data_source",
           "object_id": source_id.split("-", 1)[1], "object_name": name,
           "timestamp": datetime.now(timezone.utc).isoformat(), "user_email": actor,
           "details": details}
    return product_store.ProductMutation("create", "activity", event_id, activity_document(row))


def audit_only(action: str, object_id: str, name: str, actor: str, details=None,
               *, object_type="catalog_source") -> None:
    prefix = "catalog-" if object_type == "catalog_source" else "data-"
    product_store.transact([_audit(action, prefix + str(object_id), name, actor, details)], actor, "Admin")


def create(family: str, row: dict, actor: str, *, details=None) -> dict:
    document = source_mutations.catalog_document(row) if family == "catalog_sources" else source_mutations.data_document(row)
    if _current(document["source_id"], actor) is not None:
        raise RuntimeError("KaveonDB source already exists")
    product_store.transact([
        product_store.ProductMutation("create", "source", document["source_id"], document),
        _audit("created", document["source_id"], str(document.get("name") or ""), actor, details),
    ], actor, "Admin")
    return document


def update(record_id: str, changes: dict, actor: str, *, action="updated", details=None) -> dict | None:
    current = _current(record_id, actor)
    if current is None:
        return None
    document = dict(current["document"])
    document.update(changes)
    document["modified_by"] = actor
    document["modified_at"] = datetime.now(timezone.utc).isoformat()
    product_store.transact([
        product_store.ProductMutation("update", "source", record_id, document, current["revision"]),
        _audit(action, record_id, str(document.get("name") or ""), actor, details),
    ], actor, "Admin")
    return document


def delete(record_id: str, actor: str) -> bool:
    current = _current(record_id, actor)
    if current is None:
        return False
    document = current["document"]
    product_store.transact([
        product_store.ProductMutation("delete", "source", record_id, expected_revision=current["revision"]),
        _audit("deleted", record_id, str(document.get("name") or ""), actor),
    ], actor, "Admin")
    return True
