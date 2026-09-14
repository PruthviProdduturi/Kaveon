from unittest.mock import patch

from fastapi import HTTPException

from middleware.auth import UserContext
from routers import chat_history
from services import chat_history_store, query_history


@patch.dict("os.environ", {"KAVEONDB_READ_AUTHORITY_FAMILIES": "query_history"}, clear=False)
def test_query_history_create_and_retention_are_one_kaveondb_transaction():
    old = {"revision": 3, "document": {
        "id": "old", "user_email": "alice", "executed_at": "2026-01-01T00:00:00Z"}}
    mutations = []
    with patch.object(query_history.product_store, "list_records",
                      return_value=[old] * query_history.MAX_HISTORY_PER_OWNER), \
         patch.object(query_history.product_store, "transact",
                      side_effect=lambda values, *_: mutations.extend(values) or {}), \
         patch.object(query_history, "_supports_engine_details",
                      side_effect=AssertionError("PostgreSQL schema reached")), \
         patch.object(query_history.db, "query", side_effect=AssertionError("PostgreSQL reached")):
        result = query_history.create_history(
            {"sql_text": "SELECT 1", "status": "success", "started_at": 1_789_000_000_000}, "alice")
    assert result["sql_text"] == "SELECT 1"
    assert [(item.operation, item.expected_revision) for item in mutations] == [
        ("create", None), ("delete", 3)]
    assert mutations[0].document["user_email"] == "alice"


@patch.dict("os.environ", {"KAVEONDB_READ_AUTHORITY_FAMILIES": "query_history"}, clear=False)
def test_query_history_delete_is_owner_bounded_and_revision_checked():
    records = [{"revision": index + 1, "document": {
        "id": f"q{index}", "user_email": "alice"}} for index in range(3)]
    calls = []
    with patch.object(query_history.product_store, "list_records", return_value=records), \
         patch.object(query_history.product_store, "transact",
                      side_effect=lambda values, *_: calls.extend(values) or {}), \
         patch.object(query_history.db, "execute", side_effect=AssertionError("PostgreSQL reached")):
        assert query_history.delete_all_history("alice") == 3
    assert [item.expected_revision for item in calls] == [1, 2, 3]


def test_chat_message_create_and_session_touch_commit_atomically():
    session = {"revision": 4, "document": {
        "id": "11", "user_email": "alice", "title": "Chat",
        "created_at": "2026-09-14T18:00:00Z", "updated_at": "2026-09-14T18:00:00Z"}}
    mutations = []
    with patch.object(chat_history_store.product_store, "read", return_value=session), \
         patch.object(chat_history_store, "_new_id", return_value="12"), \
         patch.object(chat_history_store.product_store, "transact",
                      side_effect=lambda values, *_: mutations.extend(values) or {}):
        result = chat_history_store.add_message(
            "11", "alice", role="user", content="hello", data={"safe": True})
    assert result["id"] == "12"
    assert [(item.kind, item.operation, item.expected_revision) for item in mutations] == [
        ("chat_message", "create", None), ("chat_session", "update", 4)]


def test_chat_session_delete_removes_messages_in_bounded_batches_then_session():
    session = {"revision": 7, "document": {"id": "11", "user_email": "alice"}}
    messages = [{"revision": index + 1, "document": {
        "id": str(index), "session_id": "11", "user_email": "alice"}}
        for index in range(205)]
    batches = []
    with patch.object(chat_history_store.product_store, "read", return_value=session), \
         patch.object(chat_history_store.product_store, "list_records", return_value=messages), \
         patch.object(chat_history_store.product_store, "transact",
                      side_effect=lambda values, *_: batches.append(values) or {}):
        assert chat_history_store.delete_session("11", "alice") is True
    assert [len(batch) for batch in batches] == [100, 100, 5, 1]
    assert all(item.kind == "chat_message" for batch in batches[:-1] for item in batch)
    assert batches[-1][0].kind == "chat_session"


@patch.dict("os.environ", {"KAVEONDB_READ_AUTHORITY_FAMILIES": "chat_history"}, clear=False)
def test_chat_routes_create_and_append_without_postgresql():
    ctx = UserContext("alice", "Viewer")
    session = {"id": "11", "user_email": "alice", "title": "Chat",
               "created_at": "2026-09-14T18:00:00Z", "updated_at": "2026-09-14T18:00:00Z"}
    message = {"id": "12", "session_id": "11", "user_email": "alice", "role": "user",
               "content": "hello", "sql_query": None, "chart_type": None, "data": None,
               "route": None, "created_at": "2026-09-14T18:01:00Z"}
    with patch.object(chat_history.chat_history_store, "create_session", return_value=session), \
         patch.object(chat_history, "_assert_session_owner", return_value=session), \
         patch.object(chat_history.chat_history_store, "add_message", return_value=message), \
         patch.object(chat_history.db, "query_one", side_effect=AssertionError("PostgreSQL reached")):
        created = chat_history.create_session(chat_history.SessionCreate(title="Chat"), ctx)
        appended = chat_history.add_message(11, chat_history.MessageCreate(role="user", content="hello"), ctx)
    assert created["id"] == 11
    assert appended["id"] == 12


def test_stale_chat_session_conflict_does_not_retry_as_id_collision():
    session = {"revision": 4, "document": {
        "id": "11", "user_email": "alice", "title": "Chat",
        "created_at": "x", "updated_at": "x"}}
    with patch.object(chat_history_store.product_store, "read",
                      side_effect=[session, {**session, "revision": 5}]), \
         patch.object(chat_history_store.product_store, "transact",
                      side_effect=HTTPException(409, "conflict")):
        try:
            chat_history_store.add_message("11", "alice", role="user", content="hello")
            assert False, "stale session must fail"
        except HTTPException as error:
            assert error.status_code == 409
