from unittest.mock import patch

from services import charts


def _document():
    return {
        "id": "chart-1", "name": "Trips", "description": None,
        "dataset_id": "7", "dataset_revision": 4, "chart_type": "bar",
        "query_config": {"dataset_id": 7, "dimension": "borough"},
        "viz_config": {"legend": True}, "visibility": "internal",
        "created_at": "2026-09-14T18:00:00Z", "updated_at": "2026-09-14T18:00:00Z",
        "created_by": "owner@example.com", "modified_by": "owner@example.com",
    }


@patch.dict("os.environ", {"KAVEONDB_READ_AUTHORITY_FAMILIES": "charts"}, clear=False)
def test_create_chart_validates_dataset_revision_and_avoids_postgresql():
    mutations = []
    with patch.object(charts.product_store, "read", return_value={"revision": 4, "document": {}}), \
         patch.object(charts.product_store, "transact", side_effect=lambda values, *_: mutations.extend(values) or {}), \
         patch.object(charts, "get_chart_by_id", return_value={"id": "new"}), \
         patch.object(charts, "_chart_schema", side_effect=AssertionError("PostgreSQL schema reached")), \
         patch.object(charts.db, "execute", side_effect=AssertionError("PostgreSQL reached")):
        result = charts.create_chart({"name": "Trips", "dataset_id": 7, "chart_type": "bar",
                                      "query_config": {"dimension": "borough"}}, "owner@example.com")
    assert result == {"id": "new"}
    assert len(mutations) == 1
    mutation = mutations[0]
    assert mutation.operation == "create"
    assert mutation.document["dataset_id"] == "7"
    assert mutation.document["dataset_revision"] == 4
    assert mutation.document["query_config"]["dataset_id"] == 7


@patch.dict("os.environ", {"KAVEONDB_READ_AUTHORITY_FAMILIES": "charts"}, clear=False)
def test_update_and_delete_use_chart_revision_cas_without_postgresql():
    current = {"revision": 9, "document": _document()}
    dataset = {"revision": 5, "document": {}}
    mutations = []
    with patch.object(charts.product_store, "read", side_effect=[current, dataset, current]), \
         patch.object(charts.product_store, "transact", side_effect=lambda values, *_: mutations.extend(values) or {}), \
         patch.object(charts, "get_chart_by_id", return_value={"id": "chart-1"}), \
         patch.object(charts, "_chart_schema", side_effect=AssertionError("PostgreSQL schema reached")), \
         patch.object(charts.db, "execute", side_effect=AssertionError("PostgreSQL reached")):
        assert charts.update_chart("chart-1", {"name": "Updated"}, "owner@example.com")
        assert charts.delete_chart("chart-1", "owner@example.com")
    assert [(item.operation, item.expected_revision) for item in mutations] == [
        ("update", 9), ("delete", 9)]
    assert mutations[0].document["dataset_revision"] == 5
    assert mutations[0].document["viz_config"] == {"legend": True}


def test_missing_dataset_fails_before_chart_write():
    with patch.dict("os.environ", {"KAVEONDB_READ_AUTHORITY_FAMILIES": "charts"}, clear=False), \
         patch.object(charts.product_store, "read", return_value=None), \
         patch.object(charts.product_store, "transact") as transact:
        try:
            charts.create_chart({"name": "Broken", "dataset_id": 77, "chart_type": "bar"},
                                "owner@example.com")
            assert False, "missing dataset must fail"
        except ValueError as error:
            assert "unavailable" in str(error)
    transact.assert_not_called()


@patch.dict("os.environ", {"KAVEONDB_READ_AUTHORITY_FAMILIES": "charts"}, clear=False)
def test_count_uses_bounded_kaveondb_list():
    with patch("services.product_read_authority.list_documents", return_value=[{}, {}]) as listing, \
         patch.object(charts.db, "query_one", side_effect=AssertionError("PostgreSQL reached")):
        assert charts.count_charts() == 2
    listing.assert_called_once_with("charts", "kaveon-system", "Admin")
