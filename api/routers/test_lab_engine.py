import json
import os
import threading
import time
import unittest
from unittest.mock import patch

import httpx
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


class FakeCoordinator:
    """Answers the bridge's `_send` calls for one paged statement. The
    statement stays RUNNING until `finish()` (or `fail()` / a DELETE)
    releases the held POST; pages are readable by the submitting actor only."""

    def __init__(self, columns=("order_id",), pages=()):
        self.columns = [{"name": name, "type": "BIGINT"} for name in columns]
        self.pages = [list(page) for page in pages]
        self.released = threading.Event()
        self.registered = threading.Event()
        self.state = "RUNNING"
        self.error = None
        self.written = 0            # pages the writer has reached
        self.owner = None
        self.tag = None
        self.query_id = "q-stream-1"
        self.requests = []
        self.post_status = 200

    def finish(self):
        self.state, self.written = "FINISHED", len(self.pages)
        self.released.set()

    def fail(self, message):
        self.state, self.error, self.post_status = "FAILED", message, 500
        self.released.set()

    def record(self):
        finished = self.state == "FINISHED"
        return {"id": self.query_id, "state": self.state, "elapsed_ms": 1234 if self.state != "RUNNING" else 0,
                "columns": self.columns, "error": self.error, "next_uri": f"/v1/query/{self.query_id}/results/0",
                "execution": {"mode": "distributed", "detail": "fragments"} if finished else {"mode": "pending"},
                "stages": [{"stage_id": 0, "state": self.state, "task_count": 2, "completed_tasks": 1,
                            "tasks": [{"node_id": "worker-1"}, {"node_id": "worker-2"}]}],
                "scans": [{"rows_selected": 500, "rows_emitted": 7}], "rows": [], "plan": {}, "sql": "x",
                "context": {"client_tags": [self.tag]}}

    @staticmethod
    def _response(status, body=None, headers=None):
        return httpx.Response(status, json=body, headers=headers, request=httpx.Request("GET", "http://engine"))

    def send(self, method, path, token_name, actor, *, payload=None, revision=None, role=None, timeout=60):
        self.requests.append((method, path, actor, role, payload))
        if method == "POST" and path == "/v1/statement":
            self.owner, self.tag = actor, payload["client_tags"][0]
            assert payload["result_delivery"] == "paged"
            self.registered.set()
            self.released.wait(10)
            if self.post_status != 200:
                return self._response(self.post_status, {"error": self.error, "code": "DISTRIBUTED_EXECUTION_ERROR"})
            return self._response(200, {"id": self.query_id, "state": self.state, "columns": self.columns,
                                        "data": [], "elapsed_ms": 1234,
                                        "next_uri": f"/v1/query/{self.query_id}/results/0"})
        if method == "GET" and path == "/v1/query":
            self.registered.wait(10)
            return self._response(200, [self.record()] if actor == self.owner else [])
        if method == "GET" and path == f"/v1/query/{self.query_id}":
            if actor != self.owner:
                return self._response(404, {"error": "not found"})
            return self._response(200, self.record())
        if method == "GET" and path.startswith(f"/v1/query/{self.query_id}/results/"):
            if actor != self.owner:
                return self._response(404)
            if self.state in {"FAILED", "CANCELED"}:
                return self._response(410)
            page = int(path.rsplit("/", 1)[1])
            complete = self.state == "FINISHED"
            written = sum(len(rows) for rows in self.pages[:self.written])
            if page < self.written:
                more = page + 1 < len(self.pages) or not complete
                return self._response(200, {
                    "id": self.query_id, "data": self.pages[page],
                    "next_uri": f"/v1/query/{self.query_id}/results/{page + 1}" if more else None,
                    "row_count": written, "complete": complete,
                })
            if page == self.written and not complete:
                return self._response(202, {"id": self.query_id, "row_count": written, "complete": False},
                                      headers={"Retry-After": "1"})
            return self._response(404)
        if method == "DELETE" and path == f"/v1/query/{self.query_id}":
            if actor != self.owner:
                return self._response(404)
            if self.state == "RUNNING":
                self.state, self.error, self.post_status = "CANCELED", "query canceled by client", 409
                self.released.set()
            return self._response(204)
        raise AssertionError(f"unexpected Engine call {method} {path}")


def _stream_body(**overrides):
    return LabQueryBody(query="SELECT order_id FROM silver.orders", engineSourceId="source-1",
                        engineSchema="silver", stream=True, **overrides)


def _wait_for(history):
    deadline = time.monotonic() + 5
    while not history and time.monotonic() < deadline:
        time.sleep(0.02)
    return history


class EngineLabStreamTests(unittest.IsolatedAsyncioTestCase):
    def setUp(self):
        self.ctx = UserContext("analyst@example.com", "Analyst")
        env = patch.dict(os.environ, {"KAVEON_ENGINE_URL": "http://localhost:8081",
                                      "KAVEON_ENGINE_BRIDGE_TOKEN": "bridge-token"})
        env.start()
        self.addCleanup(env.stop)

    def _mock(self, coordinator, history):
        for item in (
            patch.object(lab, "_engine_source", return_value={"engine_catalog": "kavedb"}),
            patch.object(lab.sql_execute_limiter, "check"),
            patch("services.engine_bridge._send", side_effect=coordinator.send),
            patch.object(lab.history_svc, "create_history", side_effect=lambda row, user: history.append((row, user))),
        ):
            item.start()
            self.addCleanup(item.stop)

    async def _submit(self, coordinator, history, body=None):
        self._mock(coordinator, history)
        return await lab.run_query(None, body or _stream_body(), self.ctx)

    async def test_stream_submit_returns_the_record_id_while_the_statement_runs(self):
        coordinator, history = FakeCoordinator(pages=[[[1]], [[2]]]), []
        response = await self._submit(coordinator, history)
        self.assertEqual(response["success"], True)
        self.assertEqual(response["queryId"], "q-stream-1")
        self.assertTrue(response["tag"].startswith("kaveon-api:stream:"))
        self.assertEqual(response["state"], "RUNNING")
        self.assertEqual(response["nextUri"], "/v1/query/q-stream-1/results/0")
        post = next(r for r in coordinator.requests if r[0] == "POST")
        self.assertEqual((post[2], post[3]), ("analyst@example.com", "analyst"))
        self.assertEqual(post[4]["query"], "SELECT order_id FROM silver.orders")
        self.assertEqual((post[4]["catalog"], post[4]["schema"]), ("kavedb", "silver"))
        self.assertEqual(post[4]["result_delivery"], "paged")
        self.assertEqual(history, [])            # nothing recorded until it finishes
        coordinator.finish()

    async def test_record_and_pages_pass_the_coordinator_statuses_through(self):
        coordinator, history = FakeCoordinator(pages=[[[1], [2]], [[3]]]), []
        await self._submit(coordinator, history)
        coordinator.written = 1
        view = (await lab.get_lab_query("q-stream-1", Response(), self.ctx))["query"]
        self.assertEqual((view["state"], view["columns"], view["workers"]), ("RUNNING", ["order_id"], 2))
        self.assertEqual(view["next_uri"], "/v1/query/q-stream-1/results/0")
        self.assertEqual(view["stages"][0]["completed_tasks"], 1)
        self.assertEqual(view["scans"][0]["rows_selected"], 500)
        for hidden in ("rows", "plan", "sql", "context"):
            self.assertNotIn(hidden, view)
        page0 = await lab.get_lab_query_page("q-stream-1", 0, Response(), self.ctx)
        self.assertEqual(page0.status_code, 200)
        body = json.loads(page0.body)
        self.assertEqual((body["data"], body["next_uri"], body["row_count"], body["complete"]),
                         ([[1], [2]], "/v1/query/q-stream-1/results/1", 2, False))
        page1 = await lab.get_lab_query_page("q-stream-1", 1, Response(), self.ctx)
        self.assertEqual(page1.status_code, 202)
        self.assertEqual(page1.headers["Retry-After"], "1")
        self.assertEqual(json.loads(page1.body), {"id": "q-stream-1", "row_count": 2, "complete": False})
        coordinator.finish()
        page1 = await lab.get_lab_query_page("q-stream-1", 1, Response(), self.ctx)
        self.assertEqual(page1.status_code, 200)
        body = json.loads(page1.body)
        self.assertEqual((body["data"], body["next_uri"], body["row_count"], body["complete"]), ([[3]], None, 3, True))
        past = await lab.get_lab_query_page("q-stream-1", 2, Response(), self.ctx)
        self.assertEqual(past.status_code, 404)

    async def test_history_is_recorded_when_the_streamed_statement_finishes(self):
        coordinator, history = FakeCoordinator(pages=[[[1], [2]], [[3]]]), []
        await self._submit(coordinator, history, _stream_body(datasetId=7))
        coordinator.finish()
        self.assertEqual(len(_wait_for(history)), 1)
        row, user = history[0]
        self.assertEqual(user, "analyst@example.com")
        self.assertEqual((row["status"], row["row_count"], row["duration_ms"]), ("success", 3, 1234))
        self.assertEqual(row["database_name"], "engine:kavedb")
        self.assertEqual(row["dataset_id"], 7)
        self.assertEqual(row["sql_text"], "SELECT order_id FROM silver.orders")
        self.assertEqual(row["engine_query_id"], "q-stream-1")
        self.assertEqual(row["engine_details"]["execution"], {"mode": "distributed", "detail": "fragments"})

    async def test_a_failed_statement_answers_410_and_records_the_error(self):
        coordinator, history = FakeCoordinator(pages=[[[1]]]), []
        await self._submit(coordinator, history)
        coordinator.fail("storage: projection references unknown column 'nope'")
        page = await lab.get_lab_query_page("q-stream-1", 0, Response(), self.ctx)
        self.assertEqual(page.status_code, 410)
        view = (await lab.get_lab_query("q-stream-1", Response(), self.ctx))["query"]
        self.assertEqual((view["state"], view["error"]), ("FAILED", "storage: projection references unknown column 'nope'"))
        row, _ = _wait_for(history)[0]
        self.assertEqual((row["status"], row["row_count"]), ("error", 0))
        self.assertEqual(row["error_message"], "storage: projection references unknown column 'nope'")

    async def test_cancel_releases_the_statement_and_records_it_as_cancelled(self):
        coordinator, history = FakeCoordinator(pages=[[[1]]]), []
        await self._submit(coordinator, history)
        cancelled = await lab.cancel_lab_query("q-stream-1", self.ctx)
        self.assertEqual(cancelled.status_code, 204)
        self.assertEqual(coordinator.state, "CANCELED")
        page = await lab.get_lab_query_page("q-stream-1", 0, Response(), self.ctx)
        self.assertEqual(page.status_code, 410)
        row, _ = _wait_for(history)[0]
        self.assertEqual((row["status"], row["row_count"]), ("cancelled", 0))
        self.assertEqual(row["error_message"], "query canceled by client")

    async def test_another_principal_cannot_read_the_record_or_pages_or_cancel(self):
        coordinator, history = FakeCoordinator(pages=[[[1]]]), []
        await self._submit(coordinator, history)
        other = UserContext("someone-else@example.com", "Analyst")
        with self.assertRaises(HTTPException) as error:
            await lab.get_lab_query("q-stream-1", Response(), other)
        self.assertEqual(error.exception.status_code, 404)
        page = await lab.get_lab_query_page("q-stream-1", 0, Response(), other)
        self.assertEqual(page.status_code, 404)
        with self.assertRaises(HTTPException) as error:
            await lab.cancel_lab_query("q-stream-1", other)
        self.assertEqual(error.exception.status_code, 404)
        self.assertEqual(coordinator.state, "RUNNING")
        # Every read the owner makes is stamped with the actor that submitted.
        actors = {r[2] for r in coordinator.requests if r[2] == "analyst@example.com"}
        self.assertEqual(actors, {"analyst@example.com"})
        coordinator.finish()

    async def test_streamed_routes_require_the_analyst_role(self):
        seen = set()
        for api_route in lab.router.routes:
            if not getattr(api_route, "path", "").startswith("/lab/query/"):
                continue
            guards = [
                cell.cell_contents
                for dependency in api_route.dependant.dependencies
                if "require_min_role" in dependency.call.__qualname__
                for cell in dependency.call.__closure__ or ()
            ]
            self.assertEqual(guards, ["Analyst"], api_route.path)
            seen.update((api_route.path, method) for method in api_route.methods)
        self.assertEqual(seen, {("/lab/query/{query_id}", "GET"), ("/lab/query/{query_id}", "DELETE"),
                                ("/lab/query/{query_id}/results/{page}", "GET")})

    async def test_a_refusal_before_the_record_exists_is_returned_to_the_caller(self):
        history = []
        self._mock(FakeCoordinator(), history)

        def refuse(method, path, token_name, actor, **kwargs):
            if method == "POST":
                return httpx.Response(400, json={"code": "SYNTAX_ERROR", "error": "SQL parse error: Expected: an expression"},
                                      request=httpx.Request("POST", "http://engine"))
            return httpx.Response(200, json=[], request=httpx.Request("GET", "http://engine"))

        with patch("services.engine_bridge._send", side_effect=refuse):
            with self.assertRaises(HTTPException) as error:
                await lab.run_query(None, _stream_body(), self.ctx)
        self.assertEqual(error.exception.status_code, 400)
        self.assertEqual(error.exception.detail, "SQL parse error: Expected: an expression")
        self.assertEqual(history, [])

    async def test_inline_path_is_unchanged_without_stream(self):
        body = LabQueryBody(query="SELECT 1", engineSourceId="source-1", engineSchema="silver")
        self.assertFalse(body.stream)
        with patch.object(lab, "_engine_source", return_value={"engine_catalog": "kavedb"}), \
             patch.object(lab.sql_execute_limiter, "check"), \
             patch("services.engine_bridge.execute", return_value={"columns": [{"name": "n"}], "data": [[1]]}) as execute, \
             patch("services.engine_bridge.submit_streamed") as streamed, \
             patch.object(lab.history_svc, "create_history"):
            response = await lab.run_query(None, body, self.ctx)
        self.assertEqual(response["rows"], [[1]])
        execute.assert_called_once()
        streamed.assert_not_called()


if __name__ == "__main__":
    unittest.main()
