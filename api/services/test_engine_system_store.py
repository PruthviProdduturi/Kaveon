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
