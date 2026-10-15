"""Direct KaveonDB mutations for owner-isolated chat history."""

import json
import logging
import threading
import uuid
from datetime import datetime, timezone
from typing import Optional

from fastapi import HTTPException

from services import product_store
from services.chat_history_backfill import MAX_DOCUMENT_BYTES, session_document, message_document


# How many messages one owner keeps, and why it is not 1,000.
#
# `product_store.list_records` refuses a `max_records` above 1,000 and raises
# 503 once a listing exceeds the bound it was given, so 1,000 is a hard
# transport ceiling for this family — and every read of chat history is such a
# listing. `routers/chat_history.py` reads one conversation by listing the
# owner's whole message family and filtering it, and `delete_session` below
# lists it too. The bound used to BE the ceiling and nothing enforced it, so
# it was a number an owner discovered by exceeding it: past 1,000 messages
# every conversation answered 503, and deleting one answered 503 as well, so
# there was no way back for the person who had used the feature most.
#
# The population bound therefore sits below the ceiling with room for the
# appends that arrive before the count is re-established. The two numbers are
# related and the relationship is the point — raising this one without raising
# the ceiling reintroduces the fault.
MESSAGE_LIST_CEILING = 1_000
MAX_OWNER_MESSAGES = 900
# How far below the bound a remembered count starts being re-established, so
# the count is current by the time it matters rather than being read on the
# append that needs it.
_COUNT_CHECK_MARGIN = 40
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


# What each owner's message family was last counted at, and who is counting.
#
# Counting is a paged listing — ten round trips at the bound — so it is not
# something an append waits for. An owner nobody has counted is treated as
# worth counting but not as over the bound, so a cold process answers the
# append and establishes the count behind it; the margin above is what keeps
# that from mattering.
_MESSAGE_COUNTS: dict[str, int] = {}
_COUNTING: set[str] = set()
_COUNT_LOCK = threading.Lock()


def _known_count(owner: str) -> Optional[int]:
    with _COUNT_LOCK:
        return _MESSAGE_COUNTS.get(owner)


def _remember_count(owner: str, count: int) -> None:
    with _COUNT_LOCK:
        _MESSAGE_COUNTS[owner] = count


def _forget_count(owner: str) -> None:
    """Drop a remembered count, so the next append re-establishes it.

    The state to be in after messages were removed: a count left too high
    would refuse appends the owner now has room for.
    """
    with _COUNT_LOCK:
        _MESSAGE_COUNTS.pop(owner, None)


def _count_one_more(owner: str) -> None:
    with _COUNT_LOCK:
        known = _MESSAGE_COUNTS.get(owner)
        if known is not None:
            _MESSAGE_COUNTS[owner] = known + 1


def count_owner_messages(owner: str) -> int:
    """Count and remember one owner's stored messages.

    Counted rather than listed: `product_store.count_records` reports a family
    past its bound as counted-to-the-bound, where a listing raises 503 for it.
    That distinction is the whole point here — an owner already over the
    ceiling is the one this bound exists for, and reading their count must not
    be indistinguishable from the store being unreachable. A failure is left
    to the caller, which keeps the count unknown rather than inventing one.
    """
    reading = product_store.count_records(
        "chat_message", owner, "Viewer", bound=MESSAGE_LIST_CEILING)
    count = reading.get("records")
    if type(count) is not int or count < 0:
        raise RuntimeError("KaveonDB returned an invalid chat message count")
    _remember_count(owner, count)
    return count


def _establish_count(owner: str) -> None:
    try:
        count_owner_messages(owner)
    except Exception as error:
        # The count stays unknown and the next append schedules another
        # attempt. This must never reach the message that triggered it, which
        # has already been stored.
        logging.getLogger(__name__).warning(
            "chat_message_count_failed type=%s status=%s",
            type(error).__name__, getattr(error, "status_code", None))
    finally:
        with _COUNT_LOCK:
            _COUNTING.discard(owner)


def _schedule_count(owner: str) -> None:
    """Re-establish *owner*'s count if it is unknown or close to the bound."""
    known = _known_count(owner)
    if known is not None and known < MAX_OWNER_MESSAGES - _COUNT_CHECK_MARGIN:
        return
    with _COUNT_LOCK:
        if owner in _COUNTING:
            return
        _COUNTING.add(owner)
    try:
        threading.Thread(target=_establish_count, args=(owner,),
                         name="chat-message-count", daemon=True).start()
    except RuntimeError:
        # Threads unavailable during interpreter shutdown: count here rather
        # than leave the claim held and the count never established again.
        _establish_count(owner)


def _appended(owner: str, message: dict) -> dict:
    """One message is now stored: account for it and keep the count current."""
    _count_one_more(owner)
    _schedule_count(owner)
    return message


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
    # Refuse at the bound rather than write the record that makes every read
    # of this owner's history fail. Said here, where the person can act on it,
    # instead of as a 503 the next time they open a conversation.
    known = _known_count(owner)
    if known is not None and known >= MAX_OWNER_MESSAGES:
        raise HTTPException(409, {
            "code": "history_full",
            "message": f"This account has reached its limit of {MAX_OWNER_MESSAGES} "
                       "stored chat messages. Delete a conversation to continue.",
        })
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
            return _appended(owner, message)
        except HTTPException as error:
            if error.status_code not in _REJECTED_STATUSES:
                raise
            stored = _stored_message(message_id, owner)
            if stored == message:
                # The transaction committed and only its acknowledgement was
                # lost. Reporting a failure here would have the caller append
                # the same turn a second time.
                return _appended(owner, message)
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
    # Listed at the transport ceiling, not at the population bound: an owner
    # who is already over the bound is exactly the one who needs this to work.
    records = product_store.list_records(
        "chat_message", owner, "Viewer", max_records=MESSAGE_LIST_CEILING)
    _remember_count(owner, len(records))
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
        _forget_count(owner)
    product_store.transact([
        product_store.ProductMutation("delete", "chat_session", session_id,
                                      expected_revision=revision)
    ], owner, "Analyst")
    return True
