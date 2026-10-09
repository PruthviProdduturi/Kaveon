"""Direct KaveonDB mutations for owner-isolated chat history."""

import json
import uuid
from datetime import datetime, timezone
from typing import Optional

from fastapi import HTTPException

from services import product_store
from services.chat_history_backfill import MAX_DOCUMENT_BYTES, session_document, message_document


MAX_OWNER_MESSAGES = 1_000
DELETE_BATCH_SIZE = 100
# Appending a message also touches the conversation it belongs to, so two
# appends to one conversation contend on that record's revision and KaveonDB
# rejects whichever commit loses. A chat turn is two appends — the question and
# the answer — so a lost append is half the conversation gone. Each attempt
# reloads the conversation and commits against the revision it actually holds.
APPEND_ATTEMPTS = 8
# KaveonDB reports a rejected transaction as a revision conflict, and as an
# upstream refusal when the coordinator declines the commit outright. Neither
# status says which record lost, so an append confirms itself by reading back
# rather than by the status alone.
_REJECTED_STATUSES = frozenset({409, 502})


def _new_id() -> str:
    return str(1 + uuid.uuid4().int % 2_147_483_646)


def _now() -> str:
    return datetime.now(timezone.utc).isoformat().replace("+00:00", "Z")


def _document_bytes(document: dict) -> int:
    return len(json.dumps(document, sort_keys=True, separators=(",", ":"),
                          ensure_ascii=False).encode())


def _bounded(document: dict) -> dict:
    if _document_bytes(document) > MAX_DOCUMENT_BYTES:
        raise HTTPException(422, "Chat record exceeds its byte bound")
    return document


def _persistable(fields: dict) -> dict:
    """The message as it will be stored, trimmed only where it can be recovered.

    An assistant turn carries its result rows so a reopened conversation
    renders the same chart without re-querying. A wide enough result can push
    the record past KaveonDB's document bound, and refusing the write there
    would lose the answer itself. The rows are the one part the stored
    statement reproduces, so they are what gives way — marked rather than
    silently dropped — while the text, the statement, the route and the
    answer's evidence are always kept.
    """
    document = message_document(fields)
    if _document_bytes(document) <= MAX_DOCUMENT_BYTES:
        return document
    data = document.get("data")
    if isinstance(data, dict) and ("rows" in data or "columns" in data):
        trimmed = {key: value for key, value in data.items()
                   if key not in {"rows", "columns"}}
        trimmed["rows_omitted"] = True
        return _bounded(message_document({**fields, "data": trimmed}))
    return _bounded(document)


def _owned_session(session_id: str, owner: str) -> tuple[dict, int]:
    """The caller's conversation and the revision an append must commit against."""
    session = product_store.read("chat_session", session_id, owner, "Viewer")
    document = session.get("document") if isinstance(session, dict) else None
    revision = session.get("revision") if isinstance(session, dict) else None
    if not isinstance(document, dict) or document.get("user_email") != owner \
            or type(revision) is not int or revision < 1:
        raise HTTPException(404, {"code": "not_found", "message": "Session not found."})
    return document, revision


def _stored_message(message_id: str, owner: str) -> Optional[dict]:
    record = product_store.read("chat_message", message_id, owner, "Viewer")
    document = record.get("document") if isinstance(record, dict) else None
    return document if isinstance(document, dict) else None


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
    if role not in {"user", "assistant"}:
        raise HTTPException(400, {"code": "invalid_role",
                                  "message": "Role must be 'user' or 'assistant'."})
    message_id, now = _new_id(), _now()
    session_value, revision = _owned_session(session_id, owner)
    for _attempt in range(APPEND_ATTEMPTS):
        message = _persistable({
            "id": message_id, "session_id": session_id, "user_email": owner,
            "role": role, "content": content, "sql_query": sql_query,
            "chart_type": chart_type, "data": data, "route": route,
            "created_at": now,
        })
        try:
            product_store.transact([
                product_store.ProductMutation("create", "chat_message", message_id, message),
                product_store.ProductMutation("update", "chat_session", session_id,
                                              {**session_value, "updated_at": now}, revision),
            ], owner, "Analyst")
            return message
        except HTTPException as error:
            if error.status_code not in _REJECTED_STATUSES:
                raise
            stored = _stored_message(message_id, owner)
            if stored == message:
                # The transaction committed and only its acknowledgement was
                # lost. Reporting a failure here would have the caller append
                # the same turn a second time.
                return message
            if stored is not None:
                # Another record already holds the generated ID, so nothing
                # about this append conflicts. Allocate a new one and retry.
                message_id = _new_id()
                continue
            current_value, current_revision = _owned_session(session_id, owner)
            if current_revision == revision:
                # The conversation is exactly as this attempt found it, so the
                # rejection was not contention and a retry cannot change it.
                raise
            session_value, revision = current_value, current_revision
    raise HTTPException(409, {"code": "conflict", "message":
                              "Another message was appended to this conversation; retry."})


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
