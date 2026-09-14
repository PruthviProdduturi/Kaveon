from unittest.mock import patch

from fastapi import HTTPException

from services import datasets


def _document():
    return {
        "id": "71", "name": "Orders", "dataset_name": "Orders",
        "description": "Order facts", "table_name": "orders", "schema_name": "silver",
        "database_name": "OpenSource", "date_column": "order_date", "sql_text": None,
        "tables_used": '{"filters":[]}', "visibility": "internal",
        "created_at": "2026-09-14T18:00:00Z", "updated_at": "2026-09-14T18:00:00Z",
        "created_by": "owner@example.com", "modified_by": "owner@example.com",
        "modified_at": "2026-09-14T18:00:00Z",
        "dimensions": [{"dimension_table": "silver.customers", "table_name": "customers",
                        "join_condition": "[customer_id] = [customer_id]", "fact_key": "customer_id",
                        "join_key": "customer_id", "dim_name": "customers", "display_name": "Customers"}],
        "columns": [{"table_name": "orders", "column_name": "amount", "data_type": "Int64",
                     "is_dimension": False, "is_metric": True, "semantic_type": None}],
        "metrics": [{"name": "Revenue", "expression": "SUM(amount)",
                     "metric_type": "sum", "format": "currency"}],
        "filters": [],
    }


@patch.dict("os.environ", {"KAVEONDB_READ_AUTHORITY_FAMILIES": "datasets"}, clear=False)
def test_create_commits_parent_and_all_semantics_as_one_product_mutation():
    mutations = []
    payload = {
        "name": "Orders", "table_name": "orders", "schema_name": "silver",
        "database_name": "OpenSource",
        "dimensions": [{"dimension_table": "silver.customers",
                        "join_condition": "[customer_id] = [customer_id]"}],
        "columns": [{"table_name": "orders", "column_name": "amount",
                     "data_type": "Int64", "is_metric": True}],
        "metrics": [{"name": "Revenue", "expression": "SUM(amount)"}],
    }
    with patch.object(datasets, "_new_dataset_id", return_value="71"), \
         patch.object(datasets.product_store, "transact", side_effect=lambda values, *_: mutations.extend(values) or {}), \
         patch.object(datasets, "get_dataset_by_id", return_value={"id": "71"}), \
         patch.object(datasets.db, "transaction", side_effect=AssertionError("PostgreSQL reached")):
        assert datasets.create_dataset(payload, "owner@example.com") == {"id": "71"}
    assert len(mutations) == 1
    document = mutations[0].document
    assert mutations[0].operation == "create"
    assert document["dimensions"][0]["fact_key"] == "customer_id"
    assert document["columns"][0]["column_name"] == "amount"
    assert document["metrics"][0]["metric_type"] == "sum"


@patch.dict("os.environ", {"KAVEONDB_READ_AUTHORITY_FAMILIES": "datasets"}, clear=False)
def test_update_replaces_selected_semantics_and_uses_revision_cas():
    current = {"revision": 8, "document": _document()}
    mutations = []
    with patch.object(datasets.product_store, "read", return_value=current), \
         patch.object(datasets.product_store, "transact", side_effect=lambda values, *_: mutations.extend(values) or {}), \
         patch.object(datasets, "get_dataset_by_id", return_value={"id": "71"}), \
         patch.object(datasets.db, "transaction", side_effect=AssertionError("PostgreSQL reached")):
        result = datasets.update_dataset("71", {
            "name": "Orders v2",
            "metrics": [{"name": "Orders", "expression": "COUNT(*)", "metric_type": "count"}],
        }, "owner@example.com")
    assert result == {"id": "71"}
    assert len(mutations) == 1
    mutation = mutations[0]
    assert (mutation.operation, mutation.expected_revision) == ("update", 8)
    assert mutation.document["name"] == "Orders v2"
    assert mutation.document["columns"] == _document()["columns"]
    assert mutation.document["metrics"][0]["expression"] == "COUNT(*)"


@patch.dict("os.environ", {"KAVEONDB_READ_AUTHORITY_FAMILIES": "datasets"}, clear=False)
def test_delete_uses_revision_cas_and_engine_reference_validation():
    mutations = []
    with patch.object(datasets.product_store, "read", return_value={"revision": 8, "document": _document()}), \
         patch.object(datasets.product_store, "transact", side_effect=lambda values, *_: mutations.extend(values) or {}), \
         patch.object(datasets.db, "transaction", side_effect=AssertionError("PostgreSQL reached")):
        assert datasets.delete_dataset("71", "owner@example.com") is True
    assert len(mutations) == 1
    assert (mutations[0].operation, mutations[0].expected_revision) == ("delete", 8)


def test_create_retries_numeric_id_collision_without_postgresql():
    identifiers = iter(("71", "72"))
    calls = []
    with patch.dict("os.environ", {"KAVEONDB_READ_AUTHORITY_FAMILIES": "datasets"}, clear=False), \
         patch.object(datasets, "_new_dataset_id", side_effect=lambda: next(identifiers)), \
         patch.object(datasets.product_store, "transact", side_effect=[HTTPException(409, "collision"), {}]) as transact, \
         patch.object(datasets, "get_dataset_by_id", return_value={"id": "72"}), \
         patch.object(datasets.db, "transaction", side_effect=AssertionError("PostgreSQL reached")):
        result = datasets.create_dataset({"name": "Orders"}, "owner@example.com")
        calls = [call.args[0][0].record_id for call in transact.call_args_list]
    assert result == {"id": "72"}
    assert calls == ["71", "72"]


def test_component_and_document_bounds_fail_before_write():
    with patch.dict("os.environ", {"KAVEONDB_READ_AUTHORITY_FAMILIES": "datasets"}, clear=False), \
         patch.object(datasets.product_store, "transact") as transact:
        try:
            datasets.create_dataset({"name": "Bad", "columns": ["not-an-object"]}, "owner@example.com")
            assert False, "invalid components must fail"
        except ValueError as error:
            assert "entries must be objects" in str(error)
    transact.assert_not_called()


@patch.dict("os.environ", {"KAVEONDB_READ_AUTHORITY_FAMILIES": "datasets"}, clear=False)
def test_count_uses_kaveondb_list_only():
    with patch.object(datasets.product_read_authority, "list_documents", return_value=[{}, {}]), \
         patch.object(datasets.db, "query_one", side_effect=AssertionError("PostgreSQL reached")):
        assert datasets.count_datasets() == 2
