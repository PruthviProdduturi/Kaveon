import unittest
from unittest.mock import patch

import pytest

pytest.importorskip("pyodbc")

from fastapi import HTTPException, Response

from middleware.auth import UserContext
from routers import catalog, lab
from services import engine_bridge


DEFINITION = {
    "id": "t-1", "name": "green_trips", "revision": 4, "lifecycle": "active",
    "location": "abfss://opensource@kvtest.dfs.core.windows.net/snapshots/2026-09-09-v1/nyc_taxi/green_trips",
    "access": "Shortcut", "format": "Parquet",
    "columns": [
        {"name": "vendor_id", "data_type": "Int64", "nullable": False},
        {"name": "trip_distance", "data_type": {"Decimal128": [10, 2]}, "nullable": True},
    ],
}


class CatalogTableTests(unittest.TestCase):
    def test_definition_is_read_through_metadata_with_the_source_catalog(self):
        ctx = UserContext("viewer@example.com", "Viewer")
        with patch.object(lab, "_engine_source", return_value={"engine_catalog": "OpenSource"}), \
             patch.object(engine_bridge, "table_definition", return_value=DEFINITION) as definition:
            result = catalog.get_table_definition("source-1", "nyc_taxi", "green_trips", Response(), ctx)
        definition.assert_called_once_with("OpenSource", "nyc_taxi", "green_trips", "viewer@example.com", "Viewer")
        table = result["table"]
        self.assertEqual((table["catalog"], table["schema"], table["name"]), ("OpenSource", "nyc_taxi", "green_trips"))
        self.assertEqual((table["access"], table["format"], table["revision"]), ("Shortcut", "Parquet", 4))
        self.assertTrue(table["location"].startswith("abfss://"))
        self.assertEqual(table["columns"][0], {"name": "vendor_id", "dataType": "Int64", "isNullable": False})
        self.assertEqual(table["columns"][1]["dataType"], "{'Decimal128': [10, 2]}")

    def test_unknown_access_and_format_values_are_not_passed_through(self):
        ctx = UserContext("viewer@example.com", "Viewer")
        odd = {**DEFINITION, "access": "Weird", "format": "CSV", "revision": "4"}
        with patch.object(lab, "_engine_source", return_value={"engine_catalog": "OpenSource"}), \
             patch.object(engine_bridge, "table_definition", return_value=odd):
            table = catalog.get_table_definition("source-1", "nyc_taxi", "green_trips", Response(), ctx)["table"]
        self.assertIsNone(table["access"])
        self.assertIsNone(table["format"])
        self.assertIsNone(table["revision"])

    def test_invalid_column_fails_closed(self):
        ctx = UserContext("viewer@example.com", "Viewer")
        broken = {**DEFINITION, "columns": [{"name": 7}]}
        with patch.object(lab, "_engine_source", return_value={"engine_catalog": "OpenSource"}), \
             patch.object(engine_bridge, "table_definition", return_value=broken):
            with self.assertRaises(HTTPException) as error:
                catalog.get_table_definition("source-1", "nyc_taxi", "green_trips", Response(), ctx)
        self.assertEqual(error.exception.status_code, 502)

    def test_table_columns_still_returns_columns_only(self):
        with patch.object(engine_bridge, "table_definition", return_value=DEFINITION):
            self.assertEqual(engine_bridge.table_columns("OpenSource", "nyc_taxi", "green_trips", "a", "Viewer"), DEFINITION["columns"])


class CatalogUsageTests(unittest.TestCase):
    def test_usage_joins_datasets_charts_dashboards_and_dlm_with_visibility(self):
        ctx = UserContext("alice@example.com", "Analyst")
        calls = []
        def fake_query(sql, params=None):
            calls.append((sql, params))
            if "FROM dbo.datasets" in sql:
                self.assertIn("d.database_name = @param0", sql)
                self.assertEqual(params[:4], ["OpenSource", "nyc_taxi", "green_trips", '%"green_trips"%'])
                self.assertEqual(params[4:], ["Analyst", "alice@example.com"])
                return {"rows": [{"id": 7, "dataset_name": "Green trips", "visibility": "internal", "created_by": "bob"}]}
            if "FROM dbo.charts" in sql:
                self.assertEqual(params, [7, "Analyst", "alice@example.com"])
                return {"rows": [{"id": 31, "name": "Trips by hour", "dataset_id": 7}, {"id": 32, "name": "Fare mix", "dataset_id": 7}]}
            if "FROM dbo.dashboards" in sql:
                return {"rows": [
                    {"id": "d1", "name": "NYC Yellow Taxi", "slug": "nyc", "charts": "[31, 99]"},
                    {"id": "d2", "name": "Unrelated", "slug": "u", "charts": "[99]"},
                    {"id": "d3", "name": "Broken", "slug": "b", "charts": "not json"},
                ]}
            if "FROM dbo.dlm_artifact" in sql:
                self.assertEqual(params, ["7"])
                return {"rows": [{"dataset_id": "7", "status": "ready", "built_at": "2026-09-09",
                                  "stats_rollup": '{"row_counts": {"green_trips": 48131}, "row_count_source": "kaveon_engine_exact"}'}]}
            raise AssertionError(sql)
        with patch.object(lab, "_engine_source", return_value={"engine_catalog": "OpenSource"}), \
             patch.object(catalog.db, "query", side_effect=fake_query):
            result = catalog.get_table_usage("source-1", "nyc_taxi", "green_trips", Response(), ctx)
        self.assertEqual([d["name"] for d in result["datasets"]], ["Green trips"])
        self.assertEqual([c["id"] for c in result["charts"]], [31, 32])
        self.assertEqual([d["id"] for d in result["dashboards"]], ["d1"])
        self.assertEqual(result["dlm"][0]["rowCount"], 48131)
        self.assertEqual(result["dlm"][0]["rowCountSource"], "kaveon_engine_exact")
        self.assertEqual(len(calls), 4)

    def test_usage_without_datasets_skips_the_dependent_queries(self):
        ctx = UserContext("viewer@example.com", "Viewer")
        calls = []
        def fake_query(sql, params=None):
            calls.append(sql)
            return {"rows": []}
        with patch.object(lab, "_engine_source", return_value={"engine_catalog": "OpenSource"}), \
             patch.object(catalog.db, "query", side_effect=fake_query):
            result = catalog.get_table_usage("source-1", "covid", "who_daily", Response(), ctx)
        self.assertEqual(result, {"success": True, "datasets": [], "charts": [], "dashboards": [], "dlm": []})
        self.assertEqual(len(calls), 1)


if __name__ == "__main__":
    unittest.main()
