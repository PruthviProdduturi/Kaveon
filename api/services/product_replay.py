"""Ordered, reconciliation-first replay from PostgreSQL into KaveonDB."""

import hashlib
import json

from fastapi import HTTPException

from services import product_outbox, product_store


_KINDS = {
    "datasets": "dataset",
    "charts": "chart",
    "dashboards": "dashboard",
    "saved_queries": "saved_query",
    "user_themes": "user_theme",
    "favorites": "favorite",
    "catalog_sources": "source",
    "data_sources": "source",
    "user_recents": "user_recent",
    "query_history": "query_history",
    "activity": "activity",
    "chat_sessions": "chat_session",
    "chat_messages": "chat_message",
    "dlm_definitions": "dlm_definition",
    "dlm_runs": "dlm_run",
}


def _document(event: dict) -> dict:
    raw = event.get("payload_json")
    if not isinstance(raw, str):
        raise RuntimeError("Product outbox payload is missing")
    if hashlib.sha256(raw.encode("utf-8")).hexdigest() != event.get("payload_sha256"):
        raise RuntimeError("Product outbox payload hash mismatch")
    value = json.loads(raw)
    if not isinstance(value, dict):
        raise RuntimeError("Product outbox payload must be an object")
    return value


def _matches(target: dict | None, document: dict) -> bool:
    return bool(target) and target.get("document") == document


def apply_event(event: dict) -> int | None:
    """Apply one event, resolving a lost response from committed target state."""
    try:
        kind = _KINDS[event["family"]]
    except (KeyError, TypeError):
        raise RuntimeError("Unsupported product outbox family") from None
    operation = event.get("operation")
    owner = event.get("owner_principal")
    record_id = str(event.get("record_id") or "")
    if operation not in {"create", "update", "delete"} or not owner or not record_id:
        raise RuntimeError("Product outbox event is invalid")
    document = _document(event)
    target = product_store.read(kind, record_id, owner, "Admin")

    if operation == "delete":
        if target is None:
            return None
        mutation = product_store.ProductMutation(
            "delete", kind, record_id, expected_revision=int(target["revision"])
        )
    elif _matches(target, document):
        return int(target["generation"])
    elif operation == "create":
        if target is not None:
            raise RuntimeError("KaveonDB create target exists with different content")
        mutation = product_store.ProductMutation("create", kind, record_id, document)
    elif target is None:
        raise RuntimeError("KaveonDB update target is missing; backfill is incomplete")
    else:
        mutation = product_store.ProductMutation(
            "update", kind, record_id, document, int(target["revision"])
        )

    if kind == "dlm_run" and operation == "create":
        building = {**document, "status": "building", "artifact": None}
        mutations = [
            product_store.ProductMutation("create", kind, record_id, building),
            product_store.ProductMutation("update", kind, record_id, document, 1),
        ]
    else:
        mutations = [mutation]
    try:
        committed = product_store.transact(mutations, owner, "Admin")
    except HTTPException as error:
        if error.status_code != 409:
            raise
        resolved = product_store.read(kind, record_id, owner, "Admin")
        if operation == "delete" and resolved is None:
            return None
        if _matches(resolved, document):
            return int(resolved["generation"])
        raise RuntimeError("KaveonDB replay conflict did not resolve to the source event") from error
    generation = committed.get("generation") if isinstance(committed, dict) else None
    if generation is None:
        raise RuntimeError("KaveonDB commit did not return a generation")
    return int(generation)


def replay_pending(limit: int = 50, through: int | None = None) -> dict:
    """Replay a bounded prefix; stop before acknowledging the first failure."""
    applied = []
    for event in product_outbox.pending(limit, through):
        try:
            generation = apply_event(event)
        except Exception as error:
            code = (
                f"engine_http_{error.status_code}"
                if isinstance(error, HTTPException)
                else "reconciliation_failed"
            )
            try:
                product_outbox.record_failure(
                    str(event["event_id"]), str(event["payload_sha256"]), code
                )
            except Exception:
                pass
            raise
        product_outbox.mark_applied(
            str(event["event_id"]), str(event["payload_sha256"]), generation
        )
        applied.append({
            "source_sequence": int(event["source_sequence"]),
            "target_generation": generation,
        })
    return {"applied": applied, "count": len(applied)}


# The most source events a request-path caller replays before it gives up on
# reaching its own record. The background worker owns any longer backlog.
MAX_RECORD_REPLAY_EVENTS = 1000


def replay_record(family: str, record_id: str) -> int:
    """Replay the ordered prefix through one record's newest pending event.

    A source-side writer that must observe its own record in KaveonDB now
    (rather than after the background worker's next pass) calls this. Source
    order is the replay contract, so every older pending event is applied
    first; the work is bounded and fails closed on the first failed event,
    which stays recorded in the outbox. Returns the number of events applied.
    """
    applied = 0
    through = product_outbox.newest_pending_sequence(family, record_id)
    while through is not None:
        if applied >= MAX_RECORD_REPLAY_EVENTS:
            raise RuntimeError(
                "KaveonDB replay backlog ahead of the record exceeds the request bound"
            )
        batch = min(100, MAX_RECORD_REPLAY_EVENTS - applied)
        count = int(replay_pending(batch, through)["count"])
        applied += count
        # The background worker may have acknowledged the same prefix in the
        # meantime; an empty batch is only a fault while the event stays pending.
        latest = product_outbox.newest_pending_sequence(family, record_id)
        if count == 0 and latest == through:
            raise RuntimeError("Product outbox event is pending but was not replayable")
        through = latest
    return applied
