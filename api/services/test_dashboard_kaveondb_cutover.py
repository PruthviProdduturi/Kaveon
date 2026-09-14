from unittest.mock import patch

from fastapi import Response

from middleware.auth import UserContext
from routers import metadata_summary
from services import dashboards, product_store


def _document():
    return {
        "id": "dashboard-1", "name": "Operations", "description": None,
        "layout": [], "charts": ["chart-1"], "chart_revisions": {"chart-1": 3},
        "filters": [], "theme": "dark", "visibility": "internal",
        "is_published": False, "is_archived": False,
        "created_by": "owner@example.com", "modified_by": "owner@example.com",
        "created_at": "2026-09-14T18:00:00Z", "updated_at": "2026-09-14T18:00:00Z",
    }


@patch.dict("os.environ", {"KAVEONDB_READ_AUTHORITY_FAMILIES": "dashboards"}, clear=False)
def test_dashboard_create_writes_only_to_kaveondb():
    captured = []

    def transact(mutations, actor, role):
        captured.extend(mutations)
        return {"committed": True}

    with patch.object(dashboards.product_store, "read", return_value={"revision": 3, "document": {}}), \
         patch.object(dashboards.product_store, "transact", side_effect=transact), \
         patch.object(dashboards, "get_dashboard_by_id", return_value={"id": "new"}), \
         patch.object(dashboards.db, "execute", side_effect=AssertionError("PostgreSQL reached")):
        result = dashboards.create_dashboard(
            {"name": "New", "charts": ["chart-1"], "layout": [], "filters": []},
            "owner@example.com")
    assert result == {"id": "new"}
    assert len(captured) == 1
    assert captured[0].operation == "create"
    assert captured[0].kind == "dashboard"
    assert captured[0].document["chart_revisions"] == {"chart-1": 3}


@patch.dict("os.environ", {"KAVEONDB_READ_AUTHORITY_FAMILIES": "dashboards"}, clear=False)
def test_dashboard_update_and_delete_use_revision_cas_without_postgresql():
    current = {"revision": 7, "document": _document()}
    mutations = []
    with patch.object(dashboards.product_store, "read", side_effect=[current, {"revision": 3, "document": {}}, current]), \
         patch.object(dashboards.product_store, "transact", side_effect=lambda values, *_: mutations.extend(values) or {}), \
         patch.object(dashboards, "get_dashboard_by_id", return_value={"id": "dashboard-1"}), \
         patch.object(dashboards.db, "execute", side_effect=AssertionError("PostgreSQL reached")):
        assert dashboards.update_dashboard("dashboard-1", {"name": "Renamed"}, "owner@example.com")
        assert dashboards.delete_dashboard("dashboard-1", "owner@example.com")
    assert [(item.operation, item.expected_revision) for item in mutations] == [
        ("update", 7), ("delete", 7)]


def test_dashboard_cutover_rejects_missing_chart_revision():
    with patch.dict("os.environ", {"KAVEONDB_READ_AUTHORITY_FAMILIES": "dashboards"}, clear=False), \
         patch.object(dashboards.product_store, "read", return_value=None):
        try:
            dashboards.create_dashboard({"name": "Broken", "charts": ["missing"]}, "owner@example.com")
            assert False, "missing chart must fail"
        except ValueError as error:
            assert "unavailable" in str(error)


def test_metadata_summary_loads_kaveondb_when_postgresql_configuration_is_absent():
    ctx = UserContext("owner@example.com", "Admin")
    with patch.dict("os.environ", {"KAVEON_POSTGRESQL_RETIREMENT_MODE": "true",
                                   "METADATA_HOST": "", "METADATA_DATABASE": ""}, clear=False), \
         patch.object(metadata_summary.datasets_svc, "list_datasets", return_value=[{"id": "1"}]), \
         patch.object(metadata_summary.charts_svc, "list_charts", return_value=[{"id": "2"}]), \
         patch.object(metadata_summary.dashboards_svc, "list_dashboards", return_value=[{"id": "3"}]), \
         patch.object(metadata_summary.favorites_svc, "list_favorites", return_value=[]), \
         patch.object(metadata_summary.saved_queries_svc, "list_saved_queries", return_value=[]):
        result = metadata_summary.metadata_summary(Response(), ctx)
    assert result["datasets"] == [{"id": "1"}]
    assert result["dashboards"] == [{"id": "3"}]
