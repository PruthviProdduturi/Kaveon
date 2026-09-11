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


if __name__ == "__main__":
    unittest.main()
