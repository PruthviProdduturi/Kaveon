"""After the PostgreSQL retirement, a statement over an Engine catalog is
executed on the Engine; only a statement over a retired warehouse database
is refused."""
import unittest
from unittest.mock import patch

from fastapi import HTTPException, Response

from routers import sql as sql_router
from models.sql import SqlExecuteBody


class ExecuteAfterRetirementTests(unittest.TestCase):
    def body(self, database: str) -> SqlExecuteBody:
        return SqlExecuteBody(sql_text="SELECT 1", database=database, source="chat")

    def test_an_engine_catalog_is_executed_on_the_engine(self):
        context = object()
        with patch.object(sql_router.postgresql_retirement_runtime, "requested", return_value=True), \
             patch.object(sql_router, "_engine_source_for_catalog", return_value={"engine_catalog": "OpenSource"}), \
             patch.object(sql_router, "execute_engine_sql", return_value={"success": True}) as engine:
            result = sql_router.execute_sql(self.body("OpenSource"), Response(), context)
        self.assertEqual(result, {"success": True})
        engine.assert_called_once()
        self.assertEqual(engine.call_args.args[0].database, "OpenSource")

    def test_a_retired_warehouse_database_is_still_refused(self):
        with patch.object(sql_router.postgresql_retirement_runtime, "requested", return_value=True), \
             patch.object(sql_router, "_engine_source_for_catalog", return_value=None):
            with self.assertRaises(HTTPException) as error:
                sql_router.execute_sql(self.body("kaveon"), Response(), object())
        self.assertEqual(error.exception.status_code, 503)
        self.assertIn("/sql/engine", error.exception.detail)


if __name__ == "__main__":
    unittest.main()
