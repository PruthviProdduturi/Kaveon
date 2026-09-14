from unittest.mock import Mock

import pytest

from services import favorites, saved_queries, user_recents


@pytest.fixture(autouse=True)
def authority(monkeypatch):
    monkeypatch.setenv("KAVEONDB_READ_AUTHORITY_FAMILIES", "saved_queries,favorites,user_recents")


def _forbid_postgres(monkeypatch, module):
    forbidden = Mock(side_effect=AssertionError("PostgreSQL must not be called"))
    monkeypatch.setattr(module.db, "transaction", forbidden)
    monkeypatch.setattr(module.db, "query", forbidden)
    monkeypatch.setattr(module.db, "query_one", forbidden)
    monkeypatch.setattr(module.db, "execute", forbidden)


def test_saved_query_mutations_use_revision_cas_without_postgres(monkeypatch):
    _forbid_postgres(monkeypatch, saved_queries)
    transact = Mock(return_value={})
    monkeypatch.setattr("services.product_store.transact", transact)
    monkeypatch.setattr("services.saved_queries.uuid.uuid4", lambda: "query-1")

    created = saved_queries.create_saved_query({"name": "Q", "sql": "SELECT 1"}, "owner")
    assert created["id"] == "query-1"
    assert transact.call_args.args[0][0].operation == "create"

    monkeypatch.setattr("services.product_store.read", lambda *_: {
        "revision": 7, "document": {"id": "query-1", "name": "Q", "sql": "SELECT 1",
                                      "created_by": "owner", "created_at": "2026-01-01"}})
    updated = saved_queries.update_saved_query("query-1", {"sql": "SELECT 2"}, "owner")
    mutation = transact.call_args.args[0][0]
    assert (mutation.operation, mutation.expected_revision, updated["sql"]) == ("update", 7, "SELECT 2")
    assert saved_queries.delete_saved_query("query-1", "owner") is True
    mutation = transact.call_args.args[0][0]
    assert (mutation.operation, mutation.expected_revision) == ("delete", 7)


def test_saved_query_invalid_target_fails_closed(monkeypatch):
    _forbid_postgres(monkeypatch, saved_queries)
    monkeypatch.setattr("services.product_store.read", lambda *_: {"revision": 0, "document": {}})
    transact = Mock()
    monkeypatch.setattr("services.product_store.transact", transact)
    with pytest.raises(RuntimeError, match="invalid saved query"):
        saved_queries.delete_saved_query("query-1", "owner")
    transact.assert_not_called()


def test_favorite_is_idempotent_and_delete_uses_revision(monkeypatch):
    _forbid_postgres(monkeypatch, favorites)
    document = {"user_email": "owner", "object_type": "chart", "object_id": "12", "object_name": "Chart"}
    monkeypatch.setattr("services.product_store.read", lambda *_: {"revision": 4, "document": document})
    transact = Mock()
    monkeypatch.setattr("services.product_store.transact", transact)

    result = favorites.create_favorite({"object_type": "chart", "object_id": 12, "object_name": "Chart"}, "owner")
    assert result["object_id"] == "12"
    transact.assert_not_called()
    assert favorites.delete_favorite("owner", "chart", "12") is True
    mutation = transact.call_args.args[0][0]
    assert (mutation.operation, mutation.expected_revision) == ("delete", 4)


def test_data_source_favorite_normalizes_for_toggle_reads(monkeypatch):
    _forbid_postgres(monkeypatch, favorites)
    read = Mock(return_value={"document": {}})
    monkeypatch.setattr("services.product_read_authority.read_document", read)
    assert favorites.is_favorite("owner", "data_source", "9") is True
    assert read.call_args.args[1] == favorites._record_id("owner", "source", "data-9")


def _recent(owner, item, revision, created="2026-01-01T00:00:00+00:00"):
    normalized = user_recents.normalize_item_id(item, "chart")
    return {"id": user_recents.record_id(owner, normalized), "revision": revision,
            "document": {"user_email": owner, "item_id": normalized, "label": item,
                         "href": "/", "type": "chart", "created_at": created}}


def test_recent_upsert_and_retention_are_one_kaveondb_transaction(monkeypatch):
    _forbid_postgres(monkeypatch, user_recents)
    records = [_recent("owner", str(index), index + 1, f"2026-01-{index + 1:02d}") for index in range(20)]
    monkeypatch.setattr("services.product_store.list_records", lambda *_args, **_kwargs: records)
    transact = Mock()
    monkeypatch.setattr("services.product_store.transact", transact)

    user_recents.add_recent("owner", "new", "New", "/new", "chart")
    mutations = transact.call_args.args[0]
    assert [mutation.operation for mutation in mutations] == ["create", "delete"]
    assert mutations[1].expected_revision == 1


def test_cross_owner_recent_purge_groups_owner_transactions(monkeypatch):
    _forbid_postgres(monkeypatch, user_recents)
    records = [_recent("a", "7", 2), _recent("b", "7", 3), _recent("b", "8", 4)]
    monkeypatch.setattr("services.product_store.list_records", lambda *_args, **_kwargs: records)
    transact = Mock()
    monkeypatch.setattr("services.product_store.transact", transact)

    user_recents.remove_recent_all_users("7", "chart")
    assert {call.args[1] for call in transact.call_args_list} == {"a", "b"}
    assert {call.args[0][0].expected_revision for call in transact.call_args_list} == {2, 3}


def test_cross_owner_recent_purge_fails_closed_above_bound(monkeypatch):
    _forbid_postgres(monkeypatch, user_recents)
    records = [_recent(f"owner-{index}", "7", 1) for index in range(101)]
    monkeypatch.setattr("services.product_store.list_records", lambda *_args, **_kwargs: records)
    transact = Mock()
    monkeypatch.setattr("services.product_store.transact", transact)
    with pytest.raises(RuntimeError, match="fanout bound"):
        user_recents.remove_recent_all_users("7", "chart")
    transact.assert_not_called()
