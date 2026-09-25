from unittest.mock import patch

import pytest
from fastapi import HTTPException

from services import engine_system_store as store


def _row(row_id="r1"):
    return {"table": "datasets", "id": row_id, "revision": 1,
            "generation": 4, "snapshot_id": "s4", "columns": {"name": "demo"}}


def test_read_system_row_uses_admin_engine_boundary():
    with patch.object(store.engine_bridge, "_request", return_value=_row()) as request:
        result = store.read_row("datasets", "r1", "admin@example.com", "Admin")
    assert result["columns"] == {"name": "demo"}
    request.assert_called_once_with("GET", "/v1/system/datasets/r1",
                                    "KAVEON_ENGINE_BRIDGE_TOKEN", "admin@example.com",
                                    role="admin")


def test_system_row_page_is_bounded_and_snapshot_pinned():
    page = {"generation": 4, "snapshot_id": "s4", "rows": [_row()], "next_cursor": "next"}
    with patch.object(store.engine_bridge, "_request", return_value=page):
        result = store.list_rows("datasets", "admin@example.com", "Admin", limit=10)
    assert result["snapshot_id"] == "s4"
    assert result["rows"][0]["id"] == "r1"


@pytest.mark.parametrize("role", ["Viewer", "Analyst", "Editor"])
def test_system_rows_are_admin_only(role):
    with pytest.raises(HTTPException) as error:
        store.list_rows("datasets", "user@example.com", role)
    assert error.value.status_code == 403


def test_system_row_page_rejects_malformed_engine_response():
    with patch.object(store.engine_bridge, "_request", return_value={"rows": []}):
        with pytest.raises(HTTPException) as error:
            store.list_rows("datasets", "admin@example.com", "Admin")
    assert error.value.status_code == 502


def test_create_row_uses_transaction_boundary_and_commits():
    responses = [
        {"transaction_id": "tx-1"},
        {"generation": 5},
        {"generation": 6, "snapshot_id": "s6"},
    ]
    with patch.object(store.engine_bridge, "_request", side_effect=responses) as request:
        store.create_row("datasets", "d1", {"name": {"type": "string", "value": "Orders"}},
                         "admin@example.com", "Admin")
    assert request.call_args_list[0].kwargs["payload"] == {"sql": "BEGIN"}
    staged = request.call_args_list[1].kwargs["payload"]["sql"]
    assert "typed_rows" in staged and '"primary_key":"d1"' in staged
    assert request.call_args_list[2].kwargs["payload"] == {"sql": "COMMIT", "transaction_id": "tx-1"}


def test_create_row_accepts_bounded_json_columns():
    responses = [{"transaction_id": "tx-1"}, {"generation": 5}, {"generation": 6}]
    with patch.object(store.engine_bridge, "_request", side_effect=responses):
        store.create_row("datasets", "d1", {
            "config": {"type": "json", "value": {"enabled": True, "tags": ["a"]}},
        }, "admin@example.com", "Admin")


def test_create_rows_uses_one_multi_value_insert():
    responses = [{"transaction_id": "tx-b"}, {"generation": 9}, {"generation": 9}, {"generation": 10}]
    rows = [("d1", {"name": {"type": "string", "value": "A"}}),
            ("d2", {"name": {"type": "string", "value": "B"}})]
    with patch.object(store.engine_bridge, "_request", side_effect=responses) as request:
        store.create_rows(rows, "admin@example.com", "Admin", table="datasets")
    sqls = [request.call_args_list[i].kwargs["payload"]["sql"] for i in (1, 2)]
    assert all("typed_rows" in sql for sql in sqls)
    assert "'d1'" in sqls[0] and "'d2'" in sqls[1]
    assert request.call_args_list[3].kwargs["payload"] == {"sql": "COMMIT", "transaction_id": "tx-b"}


def test_update_row_uses_compare_and_swap_revision():
    responses = [{"transaction_id": "tx-2"}, {"generation": 7}, {"generation": 8}]
    with patch.object(store.engine_bridge, "_request", side_effect=responses) as request:
        store.update_row("datasets", "d1", {"name": {"type": "string", "value": "new"}},
                         3, "admin@example.com", "Admin")
    sql = request.call_args_list[1].kwargs["payload"]["sql"]
    assert "revision = 3" in sql and '"revision":4' in sql


def test_system_json_value_is_bounded():
    with pytest.raises(HTTPException) as error:
        store.create_row("datasets", "d1", {
            "config": {"type": "json", "value": "x" * (512 * 1024 + 1)},
        }, "admin@example.com", "Admin")
    assert error.value.status_code == 422
