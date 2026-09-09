import unittest
from unittest.mock import patch

import pytest

pytest.importorskip("pyodbc")

from fastapi import HTTPException, Response

from middleware.auth import UserContext
from models.lab import LabQueryBody
from routers import lab


class EngineLabTests(unittest.TestCase):
    def test_engine_query_rejects_write_batch_and_other_catalog(self):
        self.assertEqual(lab._engine_query("SELECT * FROM kavedb.test.orders;", "kavedb"), "SELECT * FROM kavedb.test.orders")
        self.assertEqual(
            lab._engine_query('WITH q AS (SELECT \'kavedb.other.table;\' AS note) SELECT * FROM "kavedb"."test"."orders"', "kavedb"),
            'WITH q AS (SELECT \'kavedb.other.table;\' AS note) SELECT * FROM "kavedb"."test"."orders"',
        )
        self.assertEqual(
            lab._engine_query("SELECT $$kavedb.other.table; /* literal */$$ AS note", "kavedb"),
            "SELECT $$kavedb.other.table; /* literal */$$ AS note",
        )
        self.assertEqual(lab._engine_query("SELECT * FROM KAVEDB.test.orders", "kavedb"), "SELECT * FROM KAVEDB.test.orders")
        for sql in ("DELETE FROM orders", "SELECT 1; SELECT 2", "SELECT * FROM other.test.orders"):
            with self.assertRaises(HTTPException):
                lab._engine_query(sql, "kavedb")

    def test_engine_source_is_active_native_and_server_resolved(self):
        source = {"id": "source-1", "name": "ADLS", "engine_catalog": "kavedb"}
        with patch.object(lab.meta_db, "query_one", return_value=source) as query:
            self.assertEqual(lab._engine_source("source-1"), source)
            self.assertEqual(query.call_args.args[1], ["source-1"])
        with patch.object(lab.meta_db, "query_one", return_value=None):
            with self.assertRaises(HTTPException) as error:
                lab._engine_source("deleted-or-inactive")
            self.assertEqual(error.exception.status_code, 404)

    def test_engine_discovery_uses_source_catalog_not_a_client_catalog(self):
        ctx = UserContext("viewer@example.com", "Viewer")
        with patch.object(lab, "_engine_source", return_value={"engine_catalog": "kavedb"}), patch(
            "services.engine_bridge.schemas", return_value={"schemas": ["bronze", "silver"]}
        ) as schemas:
            response = lab.list_engine_schemas("source-1", Response(), ctx)
        self.assertEqual(response, {"success": True, "schemas": ["bronze", "silver"]})
        schemas.assert_called_once_with("kavedb", "viewer@example.com", "Viewer")

    def test_column_discovery_uses_definition_metadata_and_studio_shape(self):
        ctx = UserContext("viewer@example.com", "Viewer")
        with patch.object(lab, "_engine_source", return_value={"engine_catalog": "kavedb"}), patch(
            "services.engine_bridge.table_columns",
            return_value=[{"name": "order_id", "data_type": "Int64", "nullable": False}],
        ) as table_columns:
            response = lab.get_engine_table_columns("source-1", "silver", "orders", Response(), ctx)
        self.assertEqual(response, {"success": True, "schema": {"columns": [
            {"name": "order_id", "dataType": "Int64", "isNullable": False}
        ]}})
        table_columns.assert_called_once_with("kavedb", "silver", "orders", "viewer@example.com", "Viewer")

    def test_query_body_keeps_engine_source_context(self):
        body = LabQueryBody(query="SELECT 1", engineSourceId="source-1", engineSchema="silver")
        self.assertEqual((body.engineSourceId, body.engineSchema), ("source-1", "silver"))


class EngineLabQueryTests(unittest.IsolatedAsyncioTestCase):
    async def test_engine_query_normalizes_columns_without_using_relational_pool(self):
        ctx = UserContext("analyst@example.com", "Analyst")
        body = LabQueryBody(query="SELECT * FROM kavedb.silver.orders", engineSourceId="source-1", engineSchema="silver")
        with patch.object(lab, "_engine_source", return_value={"engine_catalog": "kavedb"}), \
             patch.object(lab.sql_execute_limiter, "check"), \
             patch("services.engine_bridge.execute", return_value={
                 "columns": [{"name": "order_id", "type": "BIGINT"}], "data": [[1]]
             }) as execute, \
             patch.object(lab.history_svc, "create_history"):
            response = await lab.run_query(None, body, ctx)
        self.assertEqual(response["columns"], ["order_id"])
        self.assertEqual(response["rows"], [[1]])
        execute.assert_called_once_with("SELECT * FROM kavedb.silver.orders", "kavedb", "analyst@example.com", "Analyst", "silver")


if __name__ == "__main__":
    unittest.main()
