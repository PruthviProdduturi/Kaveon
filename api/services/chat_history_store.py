"""Direct KaveonDB mutations for owner-isolated chat history."""

import uuid
from datetime import datetime, timezone

from fastapi import HTTPException

from services import product_store
from services.chat_history_backfill import MAX_DOCUMENT_BYTES, session_document, message_document


MAX_OWNER_MESSAGES = 1_000
DELETE_BATCH_SIZE = 100


def _new_id() -> str:
    return str(1 + uuid.uuid4().int % 2_147_483_646)


def _now() -> str:
    return datetime.now(timezone.utc).isoformat().replace("+00:00", "Z")


def _bounded(document: dict) -> dict:
    import json
    if len(json.dumps(document, sort_keys=True, separators=(",", ":"),
                      ensure_ascii=False).encode()) > MAX_DOCUMENT_BYTES:
        raise HTTPException(422, "Chat record exceeds its byte bound")
    return document


def create_session(owner: str, title: str) -> dict:
    for _attempt in range(5):
        record_id, now = _new_id(), _now()
        document = _bounded(session_document({
            "id": record_id, "user_email": owner, "title": title,
            "created_at": now, "updated_at": now,
        }))
        try:
            product_store.transact([
                product_store.ProductMutation("create", "chat_session", record_id, document)
            ], owner, "Analyst")
            return document
        except HTTPException as error:
            if error.status_code != 409:
                raise
    raise RuntimeError("KaveonDB could not allocate a unique chat session ID")


def add_message(session_id: str, owner: str, *, role: str, content: str,
                sql_query=None, chart_type=None, data=None, route=None) -> dict:
    session = product_store.read("chat_session", session_id, owner, "Viewer")
    if session is None:
        raise HTTPException(404, {"code": "not_found", "message": "Session not found."})
    revision, session_value = session.get("revision"), session.get("document")
    if type(revision) is not int or revision < 1 or not isinstance(session_value, dict) \
            or session_value.get("user_email") != owner:
        raise HTTPException(404, {"code": "not_found", "message": "Session not found."})
    if role not in {"user", "assistant"}:
        raise HTTPException(400, {"code": "invalid_role", "message": "Role must be 'user' or 'assistant'."})
    now = _now()
    for _attempt in range(5):
        message_id = _new_id()
        message = _bounded(message_document({
            "id": message_id, "session_id": session_id, "user_email": owner,
            "role": role, "content": content, "sql_query": sql_query,
            "chart_type": chart_type, "data": data, "route": route, "created_at": now,
        }))
        updated_session = {**session_value, "updated_at": now}
        try:
            product_store.transact([
                product_store.ProductMutation("create", "chat_message", message_id, message),
                product_store.ProductMutation("update", "chat_session", session_id,
                                              updated_session, revision),
            ], owner, "Analyst")
            return message
        except HTTPException as error:
            if error.status_code != 409:
                raise
            # Distinguish a random message-ID collision from a stale session.
            current = product_store.read("chat_session", session_id, owner, "Viewer")
            if current is None or current.get("revision") != revision:
                raise HTTPException(409, "Chat session changed concurrently; retry") from error
    raise RuntimeError("KaveonDB could not allocate a unique chat message ID")


def delete_session(session_id: str, owner: str) -> bool:
    session = product_store.read("chat_session", session_id, owner, "Viewer")
    if session is None:
        return False
    document, revision = session.get("document"), session.get("revision")
    if not isinstance(document, dict) or document.get("user_email") != owner \
            or type(revision) is not int or revision < 1:
        raise RuntimeError("KaveonDB chat session delete state is invalid")
    records = product_store.list_records(
        "chat_message", owner, "Viewer", max_records=MAX_OWNER_MESSAGES)
    messages = []
    for record in records:
        value = record.get("document")
        if isinstance(value, dict) and value.get("user_email") == owner \
                and str(value.get("session_id")) == session_id:
            message_revision = record.get("revision")
            if type(message_revision) is not int or message_revision < 1:
                raise RuntimeError("KaveonDB chat message delete state is invalid")
            messages.append((str(value["id"]), message_revision))
    # Remove bounded batches first. A retry after interruption is safe; the
    # referenced session remains until every message has been removed.
    for offset in range(0, len(messages), DELETE_BATCH_SIZE):
        product_store.transact([
            product_store.ProductMutation("delete", "chat_message", record_id,
                                          expected_revision=message_revision)
            for record_id, message_revision in messages[offset:offset + DELETE_BATCH_SIZE]
        ], owner, "Analyst")
    product_store.transact([
        product_store.ProductMutation("delete", "chat_session", session_id,
                                      expected_revision=revision)
    ], owner, "Analyst")
    return True
