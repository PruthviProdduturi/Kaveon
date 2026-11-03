"""After the PostgreSQL retirement, a statement over an Engine catalog is
executed on the Engine; only a statement over a retired warehouse database
is refused."""
import unittest
from types import SimpleNamespace
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

    def test_a_database_that_is_not_an_engine_catalog_goes_to_the_pool(self):
        """This asserted a 503 and the advice "use /sql/engine". A database that
        is not an Engine catalog is a registered source now -- resolved from
        KaveonDB, queried through the pool -- so refusing it was refusing the
        product's own connector list. What must still hold is that an Engine
        catalog is not sent down this path."""
        pooled = {}

        def execute_query(sql_text, database, *args, **kwargs):
            pooled["database"] = database
            pooled["actor"] = kwargs.get("actor")
            return {"columns": [], "rows": [], "row_count": 0}

        context = SimpleNamespace(email="a@b.c", role="Analyst")
        with patch.object(sql_router, "_engine_source_for_catalog", return_value=None), \
             patch.object(sql_router.pool, "execute_query", execute_query), \
             patch.object(sql_router, "assert_no_platform_tables"), \
             patch.object(sql_router.sql_execute_limiter, "check"), \
             patch.object(sql_router.history_svc, "create_history"):
            sql_router.execute_sql(self.body("kaveon"), Response(), context)
        self.assertEqual(pooled["database"], "kaveon")
        self.assertEqual(pooled["actor"], "a@b.c",
                         "the pool needs the caller to read the registry as")


if __name__ == "__main__":
    unittest.main()
