"""Demo mode: the read-only dependency, the statement classification, the
route inventory it governs, and the Engine quota pass-through."""

import unittest
from unittest.mock import patch

import httpx
from fastapi import Depends, FastAPI, HTTPException
from fastapi.testclient import TestClient

from middleware import demo
from middleware.auth import UserContext, get_current_user, get_user_context
from routers import charts, engine_console, favorites, lab, sql
from services import engine_bridge


def _demo(enabled):
    return patch.object(demo.settings, "KAVEON_DEMO_MODE", enabled)


class ReadOnlyDependencyTests(unittest.TestCase):
    """The dependency as `main.py` applies it: at the application level,
    over the real routers, with the identity overridden."""

    def setUp(self):
        self.context = UserContext("editor@example.com", "Editor")
        self.app = FastAPI(dependencies=[Depends(demo.demo_read_only)])
        for router in (charts.router, favorites.router, lab.router, sql.router):
            self.app.include_router(router, prefix="/api/v1")
        self.app.dependency_overrides[get_user_context] = lambda: self.context
        self.app.dependency_overrides[get_current_user] = lambda: self.context.email
        self.client = TestClient(self.app)

    def tearDown(self):
        self.client.close()

    def _create_chart(self):
        with patch.object(charts.svc, "create_chart", return_value={"id": "c1"}):
            return self.client.post("/api/v1/charts", json={"name": "n", "dataset_id": 1, "chart_type": "bar"})

    def test_a_mutating_route_is_refused_for_an_editor_only_while_demo_mode_is_on(self):
        with _demo(True):
            refused = self._create_chart()
        self.assertEqual(refused.status_code, 403, refused.text)
        self.assertEqual(refused.json()["detail"], {
            "code": "demo_read_only",
            "message": "This demo is read-only. Sign in as an administrator to make changes.",
        })
        with _demo(False):
            self.assertEqual(self._create_chart().status_code, 201)

    def test_every_role_below_admin_is_refused_and_an_admin_is_unaffected(self):
        with _demo(True):
            for role in ("Viewer", "Analyst", "Editor"):
                self.context = UserContext("someone@example.com", role)
                self.assertEqual(self.client.delete("/api/v1/lab/saved-queries/q1").status_code, 403, role)
            self.context = UserContext("admin@example.com", "Admin")
            with patch.object(lab.saved_q_svc, "delete_saved_query", return_value=True):
                self.assertEqual(self.client.delete("/api/v1/lab/saved-queries/q1").status_code, 204)

    def test_reads_and_per_user_preferences_stay_available(self):
        with _demo(True):
            with patch.object(charts.svc, "list_charts", return_value=[]):
                self.assertEqual(self.client.get("/api/v1/charts").status_code, 200)
            with patch.object(favorites.svc, "toggle_favorite", return_value={"favorited": True}):
                toggled = self.client.post("/api/v1/favorites/toggle",
                                           json={"object_type": "chart", "object_id": "c1", "object_name": "n"})
            self.assertEqual(toggled.status_code, 200, toggled.text)
            with patch.object(charts.svc, "get_chart_by_id", return_value={"id": "c1", "name": "n"}), \
                    patch.object(charts.fav_svc, "toggle_favorite", return_value={"favorited": True}):
                pinned = self.client.put("/api/v1/charts/c1/favorite")
            self.assertEqual(pinned.status_code, 200, pinned.text)

    def test_an_unauthenticated_mutation_is_left_to_the_route_authentication(self):
        self.context = None
        self.app.dependency_overrides[get_current_user] = lambda: None
        with _demo(True):
            self.assertEqual(self._create_chart().status_code, 401)

    def test_sql_execution_stays_available_for_read_statements_only(self):
        self.context = UserContext("analyst@example.com", "Analyst")
        with _demo(True):
            for statement in ("DROP TABLE orders", "SET SESSION result_cache = false; SELECT 1",
                              "ANALYZE orders", "OPTIMIZE orders", "CALL refresh()", "CREATE TABLE t AS SELECT 1"):
                refused = self.client.post("/api/v1/sql/execute", json={"sql_text": statement, "database": "d"})
                self.assertEqual(refused.status_code, 403, statement)
                self.assertEqual(refused.json()["detail"]["code"], "demo_read_only")
                refused = self.client.post("/api/v1/lab/query", json={"query": statement})
                self.assertEqual(refused.status_code, 403, statement)
            with patch.object(sql.pool, "execute_query", return_value={"rows": [[1]], "columns": ["n"], "row_count": 1}), \
                    patch.object(sql.history_svc, "create_history"), \
                    patch.object(sql, "assert_no_platform_tables"), \
                    patch.object(sql.sql_execute_limiter, "check"), \
                    patch.object(sql.postgresql_retirement_runtime, "requested", return_value=False):
                allowed = self.client.post("/api/v1/sql/execute", json={"sql_text": "SELECT 1", "database": "d"})
            self.assertEqual(allowed.status_code, 200, allowed.text)


class StatementClassificationTests(unittest.TestCase):
    def test_read_statements(self):
        for statement in (
            "SELECT 1",
            "  select * from t where x = 'delete'",
            "WITH c AS (SELECT 1 AS n) SELECT n FROM c",
            "with recursive r(n) as (select 1 union all select n + 1 from r where n < 3) select * from r",
            "SHOW TABLES",
            "DESCRIBE orders",
            "DESC orders",
            "EXPLAIN SELECT 1",
            "EXPLAIN ANALYZE WITH c AS (SELECT 1) SELECT * FROM c",
            "VALUES (1), (2)",
            "-- a comment\nSELECT 1;",
            "/* update */ SELECT \"delete\" FROM [drop]",
            "SELECT * FROM t WHERE note = 'SELECT 1; DROP TABLE t'",
        ):
            self.assertTrue(demo.is_read_statement(statement), statement)

    def test_everything_else_is_refused(self):
        for statement in (
            "",
            "   ",
            "INSERT INTO t VALUES (1)",
            "UPDATE t SET x = 1",
            "DELETE FROM t",
            "MERGE INTO t USING s ON t.id = s.id WHEN MATCHED THEN DELETE",
            "CREATE TABLE t (id INT)",
            "DROP TABLE t",
            "ALTER TABLE t ADD COLUMN c INT",
            "TRUNCATE TABLE t",
            "CALL sp()",
            "ANALYZE t",
            "OPTIMIZE t",
            "SET SESSION result_cache = false",
            "SET SESSION x = 1; SELECT 1",
            "SELECT 1; DROP TABLE t",
            "WITH c AS (SELECT 1) INSERT INTO t SELECT * FROM c",
            "WITH c AS (SELECT 1) DELETE FROM t WHERE id IN (SELECT * FROM c)",
            "SELECT * INTO new_table FROM t",
            "EXPLAIN DROP TABLE t",
            "EXPLAIN ANALYZE INSERT INTO t VALUES (1)",
            "USE sales",
            "GRANT SELECT ON t TO alice",
            "COPY t FROM 'file'",
            "BEGIN",
        ):
            self.assertFalse(demo.is_read_statement(statement), statement)
        self.assertFalse(demo.is_read_statement(None))
        with self.assertRaises(HTTPException) as error:
            demo.assert_read_statement("DROP TABLE t")
        self.assertEqual(error.exception.status_code, 403)
        self.assertEqual(error.exception.detail["code"], "demo_read_only")


class RouteInventoryTests(unittest.TestCase):
    """The application carries the dependency, and exactly these mutating
    routes are allowed below Admin in demo mode: per-user preferences, the
    user's own history and statements, questions, and the execution routes
    (which are then confined to read statements). A new allowance shows
    up here."""

    ALLOWED = {
        "POST /api/connect", "POST /api/disconnect",
        "POST /api/v1/favorites", "POST /api/v1/favorites/toggle", "DELETE /api/v1/favorites/{fav_id}",
        "PUT /api/v1/theme", "DELETE /api/v1/theme",
        "POST /api/v1/user/recents", "DELETE /api/v1/user/recents", "DELETE /api/v1/user/recents/{item_id}",
        "POST /api/v1/chat/history", "POST /api/v1/chat/history/{session_id}/messages",
        "DELETE /api/v1/chat/history/{session_id}",
        "PUT /api/v1/charts/{chart_id}/favorite", "PUT /api/v1/dashboards/{dashboard_id}/favorite",
        "PUT /api/v1/datasets/{dataset_id}/favorite",
        "POST /api/v1/data-sources/{ds_id}/favorite", "DELETE /api/v1/data-sources/{ds_id}/favorite",
        "DELETE /api/v1/lab/query/{query_id}", "POST /api/v1/lab/switch-database",
        "DELETE /api/v1/lab/query-history", "POST /api/v1/lab/record-query",
        "POST /api/v1/sql/generate", "DELETE /api/v1/sql/async/{job_id}",
        "POST /api/v1/dlm/ask", "POST /api/v1/dlm/serve-chart", "POST /api/v1/chat",
    }
    STATEMENT = {
        "POST /api/v1/lab/execute": "sql", "POST /api/v1/lab/query": "query",
        "POST /api/v1/sql/execute": "sql_text", "POST /api/v1/sql/execute-async": "sql_text",
        "POST /api/v1/sql/engine": "sql_text", "POST /api/v1/dlm/reproduce": "sql",
        "POST /api/v1/context/ask": "sql",
    }

    def test_the_application_dependency_and_the_allowances(self):
        import main
        self.assertTrue(any(dependency.dependency is demo.demo_read_only for dependency in main.app.router.dependencies))
        allowed, statement, governed = set(), {}, set()
        for route in main.app.routes:
            endpoint = getattr(route, "endpoint", None)
            methods = getattr(route, "methods", None) or set()
            if endpoint is None or endpoint is main.not_found:
                continue
            for method in methods & demo.MUTATING_METHODS:
                key = f"{method} {route.path}"
                marker = getattr(endpoint, "__kaveon_demo__", None)
                if marker == "allowed":
                    allowed.add(key)
                elif isinstance(marker, tuple):
                    statement[key] = marker[1]
                else:
                    governed.add(key)
        self.assertEqual(allowed, self.ALLOWED)
        self.assertEqual(statement, self.STATEMENT)
        for key in ("POST /api/v1/charts", "PUT /api/v1/dashboards/{dashboard_id}", "DELETE /api/v1/datasets/{dataset_id}",
                    "POST /api/v1/lab/saved-queries", "POST /api/v1/lab/ctas", "POST /api/v1/catalog-sources",
                    "POST /api/v1/engine/catalog/tables", "PUT /api/v1/datasets/{dataset_id}/dlm/context",
                    "POST /api/v1/datasets/{dataset_id}/dlm/generate", "PUT /api/v1/engine/admin/resource-groups",
                    "POST /api/v1/admin/reset", "DELETE /api/v1/sql/cache"):
            self.assertIn(key, governed)


class EngineQuotaTests(unittest.TestCase):
    def _response(self, status, body):
        return httpx.Response(status, json=body, request=httpx.Request("GET", "http://engine/x"))

    def test_the_engine_quota_refusal_is_surfaced_as_is_and_admission_exhaustion_stays_short(self):
        refusal = {"error": "5 live queries per 6 hours in this demo; the next is allowed at 2026-09-22T14:20:00Z",
                   "code": "RATE_LIMITED", "message": "5 live queries per 6 hours in this demo; the next is allowed at 2026-09-22T14:20:00Z",
                   "retry_after_seconds": 1234, "next_allowed_at": "2026-09-22T14:20:00Z", "resource_group": "demo",
                   "limit": {"max_statements": 5, "per_seconds": 21600, "count": "live"}}
        with patch.object(engine_bridge, "_send", return_value=self._response(429, refusal)), \
                patch.dict("os.environ", {"KAVEON_ENGINE_URL": "http://localhost:8080", "KAVEON_ENGINE_BRIDGE_TOKEN": "t"}):
            with self.assertRaises(HTTPException) as error:
                engine_bridge.execute("SELECT 1", "lake", "alice@example.com", "Analyst")
        self.assertEqual(error.exception.status_code, 429)
        self.assertEqual(error.exception.detail["code"], "RATE_LIMITED")
        self.assertEqual(error.exception.detail["message"], refusal["message"])
        self.assertEqual(error.exception.detail["retry_after_seconds"], 1234)
        self.assertEqual(error.exception.detail["next_allowed_at"], "2026-09-22T14:20:00Z")
        self.assertEqual(error.exception.headers["Retry-After"], "1234")
        exhausted = {"error": "memory admission", "code": "MEMORY_ADMISSION_REJECTED"}
        with patch.object(engine_bridge, "_send", return_value=self._response(429, exhausted)), \
                patch.dict("os.environ", {"KAVEON_ENGINE_URL": "http://localhost:8080", "KAVEON_ENGINE_BRIDGE_TOKEN": "t"}):
            with self.assertRaises(HTTPException) as error:
                engine_bridge.execute("SELECT 1", "lake", "alice@example.com", "Analyst")
        self.assertEqual(error.exception.detail, "Engine query capacity is temporarily exhausted")
        self.assertEqual(error.exception.headers["Retry-After"], "1")

    def test_the_quota_route_combines_the_platform_flag_with_the_engine_document(self):
        analyst = UserContext("alice@example.com", "Analyst")
        engine = {"demo": {"enabled": True},
                  "quota": {"max_statements": 5, "per_seconds": 21600, "count": "live", "used": 2, "remaining": 3,
                            "resets_at": "2026-09-22T14:20:00Z", "exempt": False, "resource_group": "demo"}}
        with _demo(True), patch.object(engine_bridge, "_send", return_value=self._response(200, engine)) as send:
            document = engine_console.engine_quota(analyst)
        self.assertEqual(send.call_args.args[:2], ("GET", "/v1/quota"))
        self.assertEqual(send.call_args.kwargs["role"], "analyst")
        self.assertEqual(document, {"demo": {"read_only": True, "engine": True}, "quota": engine["quota"]})
        with _demo(True), patch.object(engine_bridge, "_send", return_value=self._response(200, engine)):
            self.assertFalse(engine_console.engine_quota(UserContext("root@example.com", "Admin"))["demo"]["read_only"])
        with _demo(False), patch.object(engine_bridge, "_send", return_value=self._response(404, {"error": "not found"})):
            self.assertEqual(engine_console.engine_quota(analyst), {"demo": {"read_only": False, "engine": False}, "quota": None})
        with _demo(True), patch.object(engine_bridge, "_endpoint", side_effect=HTTPException(503, "Engine integration is not configured")):
            self.assertEqual(engine_console.engine_quota(analyst), {"demo": {"read_only": True, "engine": False}, "quota": None})
