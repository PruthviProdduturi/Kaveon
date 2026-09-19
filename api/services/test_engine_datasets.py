import json
import unittest
from unittest.mock import patch

from fastapi import HTTPException

from middleware.auth import UserContext
from models.datasets import DatasetCreate
from routers import datasets as datasets_router
from services import engine_bridge, engine_datasets
from services import datasets as datasets_svc

CATALOG = {"id": "local-opensource", "name": "OpenSource", "revision": 2, "adapter": "Native",
           "storage": {"Local": {"base_path": "/data"}}, "credential": None, "lifecycle": "Active"}
SCHEMA = {"id": "local-opensource-sales", "catalog_id": "local-opensource", "name": "sales",
          "revision": 2, "lifecycle": "Active"}
# A realistic table document: the columns as the Engine serializes them and
# the shape `ALTER TABLE … SET SHAPE` records, with a parameterized type.
TABLE = {
    "id": "local-opensource-sales-orders", "schema_id": "local-opensource-sales", "name": "orders",
    "revision": 3, "location": "sales/orders", "access": "Shortcut", "format": "Parquet", "lifecycle": "Active",
    "columns": [
        {"name": "order_id", "data_type": "Int64", "nullable": False},
        {"name": "region", "data_type": "Utf8", "nullable": True},
        {"name": "status", "data_type": "Utf8", "nullable": True},
        {"name": "channel", "data_type": "Utf8", "nullable": True},
        {"name": "total", "data_type": {"Decimal128": [18, 2]}, "nullable": True},
        {"name": "user_id", "data_type": "Int64", "nullable": True},
        {"name": "order_date", "data_type": "Date32", "nullable": True},
        {"name": "placed_at", "data_type": {"Timestamp": ["Microsecond", None]}, "nullable": True},
    ],
    "shape": {
        "dimensions": [{"name": "region", "cap": 20}, {"name": "status", "cap": 50}],
        "measures": [{"column": "total", "aggregates": ["sum", "count"]},
                     {"column": "user_id", "aggregates": ["count_distinct"]}],
        "time": {"column": "order_date", "grain": "day", "cap": 3660},
    },
}
ANALYST = UserContext("analyst@example.com", "Analyst")


def _bridge():
    return (patch.object(engine_bridge, "table_definition_by_id", return_value=TABLE),
            patch.object(engine_bridge, "schema_definition", return_value=SCHEMA),
            patch.object(engine_bridge, "catalog_definition", return_value=CATALOG))


class ResolveTests(unittest.TestCase):
    def test_types_are_the_platform_spellings(self):
        self.assertEqual(engine_datasets.sql_type("Int64"), "bigint")
        self.assertEqual(engine_datasets.sql_type("Utf8"), "varchar")
        self.assertEqual(engine_datasets.sql_type("Date32"), "date")
        self.assertEqual(engine_datasets.sql_type({"Timestamp": ["Microsecond", None]}), "timestamp")
        self.assertEqual(engine_datasets.sql_type({"Decimal128": [18, 2]}), "decimal(18, 2)")
        self.assertEqual(engine_datasets.sql_type({"Dictionary": ["Int32", "Utf8"]}), "varchar")

    def test_resolve_reads_the_names_columns_and_shape_from_the_engine(self):
        a, b, c = _bridge()
        with a, b, c:
            resolved = engine_datasets.resolve_table("local-opensource-sales-orders", "analyst@example.com", "Analyst")
        self.assertEqual((resolved["catalog"], resolved["schema"], resolved["table"]), ("OpenSource", "sales", "orders"))
        self.assertEqual(resolved["columns"][4], {"name": "total", "data_type": "decimal(18, 2)", "nullable": True})
        self.assertEqual(resolved["shape"]["time"]["column"], "order_date")

    def test_an_unknown_or_inactive_table_is_refused(self):
        with patch.object(engine_bridge, "table_definition_by_id", return_value=None):
            with self.assertRaises(HTTPException) as refused:
                engine_datasets.resolve_table("missing", "analyst@example.com", "Analyst")
        self.assertEqual(refused.exception.status_code, 404)
        draft = {**TABLE, "lifecycle": "Draft"}
        with patch.object(engine_bridge, "table_definition_by_id", return_value=draft), \
             patch.object(engine_bridge, "schema_definition", return_value=SCHEMA), \
             patch.object(engine_bridge, "catalog_definition", return_value=CATALOG):
            with self.assertRaises(HTTPException) as refused:
                engine_datasets.resolve_table(TABLE["id"], "analyst@example.com", "Analyst")
        self.assertEqual(refused.exception.status_code, 409)


class SemanticsTests(unittest.TestCase):
    def test_the_declared_shape_decides_dimensions_measures_and_the_date_column(self):
        a, b, c = _bridge()
        with a, b, c:
            resolved = engine_datasets.resolve_table(TABLE["id"], "analyst@example.com", "Analyst")
        derived = engine_datasets.semantics(resolved)
        by_name = {column["column_name"]: column for column in derived["columns"]}
        self.assertTrue(by_name["region"]["is_dimension"] and by_name["status"]["is_dimension"])
        # `channel` is text but undeclared: with a shape nothing is inferred.
        self.assertFalse(by_name["channel"]["is_dimension"])
        self.assertTrue(by_name["total"]["is_metric"] and by_name["user_id"]["is_metric"])
        self.assertEqual(by_name["total"]["data_type"], "decimal(18, 2)")
        self.assertEqual([m["name"] for m in derived["metrics"]],
                         ["Rows", "total", "count of total", "distinct user_id"])
        self.assertEqual(derived["metrics"][1]["expression"], "SUM(total)")
        self.assertEqual(derived["metrics"][3], {"name": "distinct user_id", "expression": "COUNT(DISTINCT user_id)",
                                                 "metric_type": "count_distinct", "format": None})
        self.assertEqual(derived["date_column"], "order_date")

    def test_without_a_shape_the_column_types_decide(self):
        unshaped = {**TABLE, "shape": None}
        with patch.object(engine_bridge, "table_definition_by_id", return_value=unshaped), \
             patch.object(engine_bridge, "schema_definition", return_value=SCHEMA), \
             patch.object(engine_bridge, "catalog_definition", return_value=CATALOG):
            resolved = engine_datasets.resolve_table(TABLE["id"], "analyst@example.com", "Analyst")
        derived = engine_datasets.semantics(resolved)
        dims = [c["column_name"] for c in derived["columns"] if c["is_dimension"]]
        self.assertEqual(dims, ["region", "status", "channel"])
        # Identifier columns are never summed; `total` is.
        self.assertEqual([m["name"] for m in derived["metrics"]], ["Rows", "total"])
        self.assertEqual(derived["date_column"], "order_date")


class BindingTests(unittest.TestCase):
    def test_the_binding_fills_names_and_semantics_and_keeps_what_the_caller_sent(self):
        a, b, c = _bridge()
        body = DatasetCreate(name="Orders", source={"kind": "engine", "table_id": TABLE["id"]},
                             metrics=[{"name": "Revenue", "expression": "SUM(total)", "metric_type": "sum"}])
        with a, b, c:
            payload = engine_datasets.apply_binding(body.model_dump(exclude_none=True), "analyst@example.com", "Analyst")
        self.assertEqual(payload["source"], {"kind": "engine", "table_id": TABLE["id"]})
        self.assertEqual((payload["database_name"], payload["schema_name"], payload["table_name"]),
                         ("OpenSource", "sales", "orders"))
        self.assertEqual(payload["metrics"], [{"name": "Revenue", "expression": "SUM(total)", "metric_type": "sum"}])
        self.assertEqual(len(payload["columns"]), 8)
        self.assertEqual(payload["date_column"], "order_date")
        self.assertEqual(payload["dimensions"], [])

    def test_a_warehouse_payload_is_untouched(self):
        payload = {"name": "Events", "database_name": "kaveon", "table_name": "events"}
        with patch.object(engine_bridge, "table_definition_by_id", side_effect=AssertionError("Engine reached")):
            self.assertEqual(engine_datasets.apply_binding(payload, "analyst@example.com", "Analyst"), payload)

    def test_create_route_binds_before_the_service_stores_the_dataset(self):
        a, b, c = _bridge()
        stored = {}

        def create(payload, actor):
            stored.update(payload)
            return {"id": "9", **payload}

        body = DatasetCreate(name="Orders", source={"kind": "engine", "table_id": TABLE["id"]})
        with a, b, c, patch.object(datasets_router.svc, "create_dataset", side_effect=create):
            created = datasets_router.create_dataset(body, ANALYST)
        self.assertEqual(created["id"], "9")
        self.assertEqual(stored["table_name"], "orders")
        self.assertEqual(stored["source"]["table_id"], TABLE["id"])
        self.assertIn("distinct user_id", [m["name"] for m in stored["metrics"]])

    def test_the_source_binding_survives_the_tables_used_envelope(self):
        row = {"id": 9, "dataset_name": "Orders", "fact_table": "orders", "schema_name": "sales",
               "database_name": "OpenSource",
               "tables_used": json.dumps({"filters": [], "source": {"kind": "engine", "table_id": TABLE["id"]}})}
        adapted = datasets_svc._adapt(row)
        self.assertEqual(adapted["source"], {"kind": "engine", "table_id": TABLE["id"]})
        self.assertIsNone(datasets_svc._adapt({**row, "tables_used": json.dumps({"filters": []})})["source"])
        self.assertIsNone(datasets_svc.source_binding({"kind": "postgresql", "table_id": "x"}))
        document = datasets_svc._dataset_product_document(
            {"name": "Orders", "source": {"kind": "engine", "table_id": TABLE["id"]}}, "9", "analyst@example.com")
        self.assertEqual(document["source"], {"kind": "engine", "table_id": TABLE["id"]})
        self.assertEqual(json.loads(document["tables_used"])["source"]["table_id"], TABLE["id"])


if __name__ == "__main__":
    unittest.main()
