"""A name the source registry does not know is a bad request, not a fault.

Removing the data-plane fences replaced a wrong 503 ("Legacy database
passthrough is unavailable after PostgreSQL retirement") with a generic 500 --
"Query execution failed" from `/sql/execute`, "An unexpected error occurred"
from the Lab. A fresh local stack showed it. Less actionable than the wrong
answer it replaced is not an improvement, so an unresolvable name now says
which name and what to do about it.
"""

import sys
import unittest
from types import SimpleNamespace
from unittest.mock import patch

if "pyodbc" not in sys.modules:
    sys.modules["pyodbc"] = SimpleNamespace(Error=Exception)

from fastapi import HTTPException, Response

import database.pool as pool
from middleware.auth import UserContext
from models.sql import SqlExecuteBody
from routers import sql


class UnknownSourceIsNotAFaultTests(unittest.TestCase):
    def test_the_pool_distinguishes_an_unknown_name_from_a_broken_one(self):
        """`UnknownDataSource` means nothing was attempted. A source that exists
        but has no endpoint is still a ValueError, because that one is broken
        rather than absent."""
        self.assertTrue(issubclass(pool.UnknownDataSource, Exception))
        self.assertFalse(issubclass(pool.UnknownDataSource, ValueError))

    def test_an_unknown_name_raises_it_rather_than_a_generic_value_error(self):
        with patch.object(pool, "_live_meta_db", return_value="__absent__"),              patch.object(pool, "_aad_dbs", return_value=set()),              patch.object(pool, "_resolve_data_source", return_value=None),              patch.object(pool.settings, "DATAWAREHOUSE_ENDPOINT", ""):
            with self.assertRaises(pool.UnknownDataSource) as raised:
                pool.get_connection_pool("not-registered", "alice", "Analyst")
        self.assertIn("not-registered", str(raised.exception))

    def test_sql_execute_answers_400_naming_the_source(self):
        body = SqlExecuteBody(sql_text="SELECT 1", database="warehouse", source="lab")
        context = UserContext("analyst@example.com", "Analyst")
        with patch.object(sql, "_engine_source_for_catalog", return_value=None),              patch.object(sql, "assert_no_platform_tables"),              patch.object(sql.sql_execute_limiter, "check"),              patch.object(sql.history_svc, "create_history"),              patch.object(sql.pool, "execute_query",
                          side_effect=pool.UnknownDataSource("warehouse")):
            with self.assertRaises(HTTPException) as raised:
                sql.execute_sql(body, Response(), context)
        self.assertEqual(raised.exception.status_code, 400)
        self.assertIn("warehouse", raised.exception.detail)
        self.assertIn("Register it", raised.exception.detail)
        # It must not read as a failed query: nothing ran.
        self.assertNotIn("failed", raised.exception.detail.lower())

    def test_the_handler_names_the_source_and_leaks_nothing_else(self):
        import asyncio
        from middleware.errors import unknown_data_source_handler
        request = SimpleNamespace(method="POST",
                                  url=SimpleNamespace(path="/api/v1/lab/tables"))
        response = asyncio.run(
            unknown_data_source_handler(request, pool.UnknownDataSource("warehouse")))
        self.assertEqual(response.status_code, 400)
        body = response.body.decode()
        self.assertIn("warehouse", body)
        self.assertIn("unknown_data_source", body)
        # The database name is the caller's own input. Nothing else goes out.
        for leak in ("password", "sslmode", "vault.azure.net", "Traceback"):
            self.assertNotIn(leak, body)


if __name__ == "__main__":
    unittest.main()
