"""A failure must be reported as itself, and must not carry the server's guts.

Each test here pins one place where a request path answered with the wrong
thing: an empty list where a read had failed, a driver message where the
caller needed an action, or no ceiling at all where another route on the same
result had one.
"""

import os
import sys
import unittest
from types import SimpleNamespace
from unittest.mock import patch

if "pyodbc" not in sys.modules:
    sys.modules["pyodbc"] = SimpleNamespace(Error=Exception)

from fastapi import HTTPException, Response
from fastapi.responses import JSONResponse

from middleware.auth import UserContext
from models.sql import SqlExecuteBody
from routers import dlm as dlm_router
from routers import health
from routers import lab
from routers import setup
from routers import sql


class AbsenceIsNotAVerdictTests(unittest.TestCase):
    def test_lab_databases_reports_a_failed_read_rather_than_no_sources(self):
        """`success: true` with an empty list told the source picker there was
        nothing registered, so the reader went looking for a missing source
        instead of a control plane that was not answering."""
        with patch.object(lab.product_read_authority, "enabled", return_value=False), \
             patch.object(lab.meta_db, "query", side_effect=RuntimeError("pool exhausted")):
            with self.assertRaises(HTTPException) as caught:
                lab.list_databases(Response(), "analyst@example.com")
        self.assertEqual(caught.exception.status_code, 502)
        self.assertNotIn("pool exhausted", str(caught.exception.detail))

    def test_a_present_but_empty_registry_still_answers_empty(self):
        with patch.object(lab.product_read_authority, "enabled", return_value=False), \
             patch.object(lab.meta_db, "query", return_value={"rows": []}):
            answer = lab.list_databases(Response(), "analyst@example.com")
        self.assertEqual(answer, {"success": True, "databases": []})


class ErrorBodiesCarryNoInternalsTests(unittest.TestCase):
    def test_health_does_not_echo_the_driver_message(self):
        """`/api/health` carries no authentication — it is what the container
        platform probes — and a psycopg or pyodbc connection failure names the
        host, port, database and user it tried."""
        failure = RuntimeError(
            'connection to server at "kaveon-pg.postgres.database.azure.com", '
            'port 5432 failed: FATAL: password authentication failed for user "kaveon"')
        with patch.object(health.postgresql_retirement_runtime, "requested", return_value=False), \
             patch.object(health.db, "query", side_effect=failure):
            result = health.health()
        self.assertIsInstance(result, JSONResponse)
        self.assertEqual(result.status_code, 503)
        self.assertNotIn(b"kaveon-pg", result.body)
        self.assertNotIn(b"password authentication", result.body)

    def test_setup_status_publishes_the_class_of_failure_and_nothing_else(self):
        """Unauthenticated, so what it returns is public: it used to classify
        the driver's message and then return the message, the endpoint and the
        database name with it."""
        failure = RuntimeError('login failed for user "kaveon" on sql-prod.database.windows.net')
        with patch.dict(os.environ, {"KAVEON_POSTGRESQL_RETIREMENT_MODE": "false"}), \
             patch.object(setup, "_read_env_vars", return_value={
                 "db_type": "azure_sql", "endpoint": "sql-prod.database.windows.net",
                 "database": "kaveonmeta", "host": "", "port": ""}), \
             patch.object(setup.pool, "execute_query", side_effect=failure):
            answer = setup.setup_status()
        self.assertEqual(answer["status"], "auth_failed")
        body = str(answer)
        self.assertNotIn("sql-prod.database.windows.net", body)
        self.assertNotIn("kaveonmeta", body)
        self.assertNotIn("login failed", body)
        # The classification a reader can act on is still there.
        self.assertEqual(answer["errors"][0]["error_type"], "auth_failed")

    def test_setup_status_does_not_publish_the_location_when_the_schema_is_absent(self):
        with patch.dict(os.environ, {"KAVEON_POSTGRESQL_RETIREMENT_MODE": "false"}), \
             patch.object(setup, "_read_env_vars", return_value={
                 "db_type": "azure_sql", "endpoint": "sql-prod.database.windows.net",
                 "database": "kaveonmeta", "host": "", "port": ""}), \
             patch.object(setup.pool, "execute_query",
                          return_value={"rows_objects": [{"cnt": 0}]}):
            answer = setup.setup_status()
        self.assertEqual(answer, {"status": "schema_missing"})

    def test_ctas_failure_is_not_reported_as_a_bad_request_with_the_driver_text(self):
        from models.lab import CtasBody
        body = CtasBody(sql="SELECT 1", database="warehouse", schema="dbo", table_name="t")
        failure = RuntimeError('Login timeout expired [host=sql-prod, user=kaveon]')
        with patch.object(lab, "_require_legacy_data_plane"), \
             patch.object(lab.sql_execute_limiter, "check"), \
             patch.object(lab, "assert_no_platform_tables"), \
             patch.object(lab.pool, "execute_query", side_effect=failure):
            with self.assertRaises(HTTPException) as caught:
                lab.create_table_as_select(body, SimpleNamespace(email="a@b.c", role="Analyst"))
        self.assertEqual(caught.exception.status_code, 422)
        self.assertNotIn("sql-prod", caught.exception.detail)
        self.assertNotIn("Login timeout", caught.exception.detail)


class BoundedReadsTests(unittest.TestCase):
    def test_query_history_limit_is_clamped_at_both_ends(self):
        """The limit reached the store unchecked: a large one asked the control
        plane for everything it holds, and a negative one was applied as a
        Python slice, which returned all but the last few records."""
        seen = []
        with patch.object(lab.history_svc, "list_history",
                          side_effect=lambda user, limit: seen.append(limit) or []):
            for requested in (-1, 0, 50, 10_000_000):
                lab.get_query_history(Response(), requested, "analyst@example.com")
        self.assertEqual(seen, [1, 1, 50, lab.MAX_HISTORY_LIMIT])

    def test_engine_chart_results_carry_the_same_ceiling_as_the_legacy_path(self):
        """`/sql/execute` bounds its result at the source and says when it
        truncated; `/sql/engine` returned whatever the Engine produced unless
        the caller happened to send a row limit, so the two routes disagreed
        about the size of a result for the same chart."""
        rows = [[index] for index in range(sql.MAX_RESULT_ROWS + 25)]
        answer = _engine_chart(rows)
        self.assertEqual(len(answer["rows"]), sql.MAX_RESULT_ROWS)
        self.assertTrue(answer["truncated"])

    def test_a_result_inside_the_ceiling_is_not_marked_truncated(self):
        answer = _engine_chart([[1], [2], [3]])
        self.assertEqual(len(answer["rows"]), 3)
        self.assertFalse(answer["truncated"])

    def test_a_callers_own_limit_still_applies(self):
        rows = [[index] for index in range(50)]
        self.assertEqual(len(_engine_chart(rows, row_limit=10)["rows"]), 10)

    def test_a_caller_cannot_ask_for_more_than_the_ceiling(self):
        """`SqlExecuteBody` already refuses a larger `row_limit`; the gap was
        the request that sends none, which had no ceiling at all."""
        from pydantic import ValidationError
        with self.assertRaises(ValidationError):
            SqlExecuteBody(sql_text="SELECT 1", database="OpenSource",
                           row_limit=sql.MAX_RESULT_ROWS + 1)


def _engine_chart(rows, row_limit=None):
    """Run one chart statement through `/sql/engine` with a stubbed Engine."""
    ctx = UserContext("analyst@example.com", "Analyst")
    body = SqlExecuteBody(sql_text="SELECT n FROM t", database="OpenSource",
                          dataset_id=7, source="chart-builder", row_limit=row_limit)
    result = {"id": "query-1", "elapsed_ms": 5,
              "columns": [{"name": "n"}], "data": rows}
    with patch.object(sql, "_engine_source_for_catalog",
                      return_value={"id": "s1", "engine_catalog": "OpenSource"}), \
         patch.object(sql.datasets_svc, "get_dataset_by_id",
                      return_value={"database_name": "OpenSource", "schema_name": "nyc_taxi"}), \
         patch.object(sql, "_execute_engine_chart_statement", return_value=(result, None)), \
         patch.object(sql.history_svc, "create_history"), \
         patch.object(sql.sql_execute_limiter, "check"):
        return sql.execute_engine_sql(body, Response(), ctx)


class DlmDatasetAuthorizationTests(unittest.TestCase):
    """Two DLM routes named a dataset by id and handed it to an operation that
    read it with no identity, or rebuilt it at `role="Admin"`. Neither applied
    the caller's visibility, so an Analyst could have a private dataset
    belonging to someone else recompiled from the id alone."""

    CTX = UserContext("analyst@example.com", "Analyst")

    def test_refresh_refuses_a_dataset_the_caller_cannot_see(self):
        with patch.object(dlm_router.dlm.datasets_svc, "get_dataset_by_id",
                          return_value=None) as read, \
             patch.object(dlm_router.dlm, "incremental_refresh") as refresh:
            with self.assertRaises(HTTPException) as caught:
                dlm_router.incremental_refresh("9", ctx=self.CTX)
        self.assertEqual(caught.exception.status_code, 404)
        read.assert_called_once_with("9", self.CTX.email, self.CTX.role)
        refresh.assert_not_called()

    def test_notify_data_change_refuses_a_dataset_the_caller_cannot_see(self):
        with patch.object(dlm_router.dlm.datasets_svc, "get_dataset_by_id",
                          return_value=None), \
             patch.object(dlm_router.dlm, "invalidate_caches") as invalidate, \
             patch.object(dlm_router.dlm, "_trigger_background_rebuild") as rebuild:
            with self.assertRaises(HTTPException) as caught:
                dlm_router.notify_data_change("9", ctx=self.CTX)
        self.assertEqual(caught.exception.status_code, 404)
        invalidate.assert_not_called()
        rebuild.assert_not_called()

    def test_a_visible_dataset_still_refreshes(self):
        with patch.object(dlm_router.dlm.datasets_svc, "get_dataset_by_id",
                          return_value={"id": "9", "created_by": self.CTX.email}), \
             patch.object(dlm_router.dlm, "incremental_refresh",
                          return_value={"ok": True, "row_count": 3}) as refresh:
            answer = dlm_router.incremental_refresh("9", ctx=self.CTX)
        self.assertTrue(answer["ok"])
        refresh.assert_called_once_with("9")


class NoDebugOutputInRequestPathsTests(unittest.TestCase):
    def test_no_router_writes_to_stdout(self):
        """A print in a request path does not reach the deployed log pipeline,
        and these lines carried the caller's SQL, the database name and the
        driver's raw message into container output."""
        import pathlib
        offenders = []
        for path in sorted(pathlib.Path(__file__).parent.glob("*.py")):
            if path.name.startswith("test_"):
                continue
            for number, line in enumerate(path.read_text(encoding="utf-8").splitlines(), 1):
                stripped = line.strip()
                if stripped.startswith("#"):
                    continue
                if "print(" in stripped:
                    offenders.append(f"{path.name}:{number}")
        self.assertEqual(offenders, [])


if __name__ == "__main__":
    unittest.main()
