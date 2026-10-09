from unittest.mock import patch

from services import charts, datasets


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


def _library_documents(count: int = 70) -> list[dict]:
    """A Library-sized chart list spread across the eight datasets it uses."""
    dataset_ids = ["144", "139", "140", "132", "133", "137", "135", "138"]
    documents = []
    for index in range(count):
        document = _document()
        document["id"] = f"chart-{index}"
        document["dataset_id"] = dataset_ids[index % len(dataset_ids)]
        documents.append(document)
    return documents


def _dataset_records(dataset_ids) -> list[dict]:
    return [{"document": {"id": identity, "name": f"Dataset {identity}",
                          "dataset_name": f"Dataset {identity}"}}
            for identity in dataset_ids]


@patch.dict("os.environ", {"KAVEONDB_READ_AUTHORITY_FAMILIES": "charts,datasets"}, clear=False)
def test_the_chart_list_names_every_dataset_in_one_index_read():
    documents = _library_documents()
    datasets._name_index_cache = None
    records = _dataset_records(sorted({item["dataset_id"] for item in documents}))
    with patch("services.product_read_authority.list_documents", return_value=documents), \
         patch.object(datasets.product_store, "list_records", return_value=records) as listing, \
         patch.object(charts.product_store, "read",
                      side_effect=AssertionError("per-chart dataset read")), \
         patch.object(charts, "_chart_schema", side_effect=AssertionError("PostgreSQL reached")):
        result = charts.list_charts("owner@example.com", "Admin")
    assert len(result) == 70
    assert listing.call_count == 1
    assert {item["dataset_name"] for item in result} == {
        f"Dataset {identity}" for identity in
        {document["dataset_id"] for document in documents}}


@patch.dict("os.environ", {"KAVEONDB_READ_AUTHORITY_FAMILIES": "charts"}, clear=False)
def test_a_chart_list_on_postgresql_datasets_names_them_in_one_query():
    documents = _library_documents()
    datasets._name_index_cache = None
    rows = {"rows": [{"id": identity, "dataset_name": f"Dataset {identity}"}
                     for identity in {item["dataset_id"] for item in documents}]}
    with patch("services.product_read_authority.list_documents", return_value=documents), \
         patch.object(datasets.db, "query", return_value=rows) as query, \
         patch.object(charts, "_chart_schema", side_effect=AssertionError("PostgreSQL reached")):
        result = charts.list_charts("owner@example.com", "Admin")
    assert query.call_count == 1
    assert all(item["dataset_name"] for item in result)


@patch.dict("os.environ", {"KAVEONDB_READ_AUTHORITY_FAMILIES": "charts,datasets"}, clear=False)
def test_a_single_chart_read_reuses_a_recent_dataset_name_index():
    document = _document()
    datasets._name_index_cache = None
    records = _dataset_records([document["dataset_id"]])
    with patch("services.product_read_authority.read_document", return_value=dict(document)), \
         patch.object(datasets.product_store, "list_records", return_value=records) as listing, \
         patch.object(charts, "_chart_schema", side_effect=AssertionError("PostgreSQL reached")):
        first = charts.get_chart_by_id("chart-1", "owner@example.com", "Admin")
        second = charts.get_chart_by_id("chart-1", "owner@example.com", "Admin")
    assert first["dataset_name"] == second["dataset_name"] == f"Dataset {document['dataset_id']}"
    assert listing.call_count == 1


@patch.dict("os.environ", {"KAVEONDB_READ_AUTHORITY_FAMILIES": "charts,datasets"}, clear=False)
def test_a_chart_whose_dataset_is_absent_reports_no_dataset_name():
    document = _document()
    datasets._name_index_cache = None
    with patch("services.product_read_authority.list_documents", return_value=[document]), \
         patch.object(datasets.product_store, "list_records", return_value=[]), \
         patch.object(charts, "_chart_schema", side_effect=AssertionError("PostgreSQL reached")):
        result = charts.list_charts("owner@example.com", "Admin")
    assert result[0]["dataset_id"] == document["dataset_id"]
    assert result[0]["dataset_name"] is None
