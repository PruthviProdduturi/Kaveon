import threading
import unittest
from unittest.mock import patch

from fastapi import FastAPI
from fastapi.testclient import TestClient

from middleware.auth import UserContext, get_user_context
from models.sql import SqlExecuteBody
from routers import sql


class AsyncSqlSecurityTests(unittest.TestCase):
    def setUp(self):
        self.context = UserContext("alice@example.com", "Analyst")
        self.app = FastAPI()
        self.app.include_router(sql.router)
        self.app.dependency_overrides[get_user_context] = lambda: self.context
        self.client = TestClient(self.app)
        with sql._ASYNC_JOBS_LOCK:
            self.saved_jobs = dict(sql._ASYNC_JOBS)
            sql._ASYNC_JOBS.clear()

    def tearDown(self):
        self.client.close()
        with sql._ASYNC_JOBS_LOCK:
            sql._ASYNC_JOBS.clear()
            sql._ASYNC_JOBS.update(self.saved_jobs)

    def test_submit_and_finish_retain_owner_without_exposing_it(self):
        with patch.object(sql.pool, "execute_query", return_value={"rows": [[42]], "columns": ["n"]}), \
                patch.object(sql.history_svc, "create_history"), \
                patch.object(sql, "assert_no_platform_tables"), \
                patch.object(sql.sql_execute_limiter, "check"):
            response = self.client.post("/sql/execute-async", json={"sql_text": "SELECT 42", "database": "sample"})
        self.assertEqual(response.status_code, 200, response.text)
        job_id = response.json()["job_id"]
        self.assertEqual(sql._ASYNC_JOBS[job_id]["owner"], self.context.email)
        self.assertIn("finished_at", sql._ASYNC_JOBS[job_id])
        result = self.client.get(f"/sql/async/{job_id}")
        self.assertEqual(result.json()["rows"], [[42]])
        self.assertNotIn("owner", result.json())

    def test_other_owner_admin_and_legacy_entries_are_inaccessible(self):
        sql._ASYNC_JOBS["owned"] = {"owner": "alice@example.com", "status": "success", "rows": [[42]]}
        sql._ASYNC_JOBS["legacy"] = {"status": "success", "rows": [[99]]}
        for context in [UserContext("bob@example.com", "Analyst"), UserContext("bob@example.com", "Admin")]:
            self.context = context
            for job_id in ["owned", "legacy", "missing"]:
                for method in [self.client.get, self.client.delete]:
                    self.assertEqual(method(f"/sql/async/{job_id}").status_code, 404)
        self.assertIn("owned", sql._ASYNC_JOBS)
        self.context = UserContext("alice@example.com", "Analyst")
        self.assertEqual(self.client.get("/sql/async/legacy").status_code, 404)
        self.assertEqual(self.client.delete("/sql/async/owned").status_code, 200)
        self.assertNotIn("owned", sql._ASYNC_JOBS)

    def test_missing_or_revoked_identity_cannot_poll_or_delete(self):
        sql._ASYNC_JOBS["owned"] = {"owner": "alice@example.com", "status": "running"}
        for context, status in [(None, 401), (UserContext("alice@example.com", "NoAccess"), 403)]:
            self.context = context
            self.assertEqual(self.client.get("/sql/async/owned").status_code, status)
            self.assertEqual(self.client.delete("/sql/async/owned").status_code, status)
        self.assertIn("owned", sql._ASYNC_JOBS)

    def test_deletion_during_execution_cannot_resurrect_success_or_error(self):
        for fails in [False, True]:
            with self.subTest(fails=fails):
                sql._ASYNC_JOBS["running"] = {"owner": self.context.email, "status": "running"}
                entered, release = threading.Event(), threading.Event()

                def execute(*args):
                    entered.set()
                    if not release.wait(5):
                        raise RuntimeError("test release timed out")
                    if fails:
                        raise RuntimeError("database failed")
                    return {"rows": [[42]]}

                with patch.object(sql.pool, "execute_query", side_effect=execute), patch.object(sql.history_svc, "create_history"):
                    thread = threading.Thread(target=sql._async_job_run, args=("running", SqlExecuteBody(sql_text="SELECT 42", database="sample"), self.context.email))
                    thread.start()
                    try:
                        self.assertTrue(entered.wait(5))
                        self.assertEqual(self.client.delete("/sql/async/running").status_code, 200)
                    finally:
                        release.set()
                        thread.join(5)
                    self.assertFalse(thread.is_alive())
                self.assertNotIn("running", sql._ASYNC_JOBS)

    def test_failure_keeps_owner_and_deleted_or_ownerless_jobs_do_not_start(self):
        data = SqlExecuteBody(sql_text="SELECT 42", database="sample")
        sql._ASYNC_JOBS["failure"] = {"owner": self.context.email, "status": "running"}
        with patch.object(sql.pool, "execute_query", side_effect=RuntimeError("database failed")):
            sql._async_job_run("failure", data, self.context.email)
        self.assertEqual(sql._ASYNC_JOBS["failure"]["owner"], self.context.email)
        self.assertEqual(sql._ASYNC_JOBS["failure"]["status"], "error")
        self.assertIn("finished_at", sql._ASYNC_JOBS["failure"])
        sql._ASYNC_JOBS["legacy"] = {"status": "running"}
        with patch.object(sql.pool, "execute_query") as execute:
            sql._async_job_run("missing", data, self.context.email)
            sql._async_job_run("legacy", data, self.context.email)
            sql._async_job_run("failure", data, "bob@example.com")
            execute.assert_not_called()
