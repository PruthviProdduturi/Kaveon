"""Build-time Engine queries carry the DLM's own bound instead of the bridge's
60-second interactive default, and a breakdown the Engine could not deliver is
recorded with its reason rather than vanishing."""
import sys
import unittest
from types import SimpleNamespace
from unittest.mock import patch

if "pyodbc" not in sys.modules:
    sys.modules["pyodbc"] = SimpleNamespace(Error=Exception)

import httpx
from fastapi import HTTPException

from dlm import engine
from services import engine_bridge


class BridgeTimeoutTests(unittest.TestCase):
    def _run(self, timeout=None, side_effect=None):
        seen = {}

        def fake_request(method, url, **kw):
            if method == "POST":            # the statement; the details fetch that follows keeps its own default
                seen.update(kw)
            if side_effect:
                raise side_effect
            return SimpleNamespace(status_code=200, is_success=True, json=lambda: {"id": "q1", "columns": [], "data": []})

        with patch.dict("os.environ", {"KAVEON_ENGINE_BRIDGE_TOKEN": "t", "KAVEON_ENGINE_URL": "https://engine"}), \
             patch.object(engine_bridge, "_verify_context", lambda: False), \
             patch.object(engine_bridge.httpx, "request", fake_request):
            kwargs = {"timeout": timeout} if timeout else {}
            return engine_bridge.execute("SELECT 1", "OpenSource", "kaveon-system", "Admin", "s", **kwargs), seen

    def test_interactive_default_stays_sixty_seconds(self):
        _, seen = self._run()
        self.assertEqual(seen["timeout"], 60)

    def test_a_caller_bound_reaches_httpx(self):
        _, seen = self._run(timeout=300)
        self.assertEqual(seen["timeout"], 300)

    def test_a_timeout_is_reported_as_such(self):
        with self.assertRaises(HTTPException) as ctx:
            self._run(timeout=5, side_effect=httpx.ReadTimeout("slow"))
        self.assertEqual(ctx.exception.status_code, 504)
        self.assertIn("bound", str(ctx.exception.detail))


class BuildBoundTests(unittest.TestCase):
    def test_native_build_queries_carry_the_build_bound(self):
        with patch.object(engine.meta, "query_one", return_value={"engine_catalog": "OpenSource"}), \
             patch("services.engine_bridge.execute", return_value={"columns": [], "data": []}) as execute:
            engine._execute_dataset_query('SELECT COUNT(*) FROM "s"."t"', "OpenSource")
            engine._execute_dataset_query('SELECT COUNT(*) FROM "s"."t"', "OpenSource", timeout_seconds=120)
        self.assertEqual(execute.call_args_list[0].kwargs.get("timeout"), engine._BUILD_QUERY_TIMEOUT_SECONDS)
        self.assertEqual(execute.call_args_list[1].kwargs.get("timeout"), 120)


class SkippedBreakdownTests(unittest.TestCase):
    def test_a_failed_breakdown_is_recorded_with_its_reason(self):
        columns = [{"column_name": "surface", "is_dimension": True}, {"column_name": "country", "is_dimension": True}]
        metrics = [{"name": "Total actions", "expression": "SUM(actions)"}]
        calls = []

        def run(sql, database, timeout_seconds=None):
            calls.append(sql)
            if "GROUP BY" not in sql:
                return {"rows": [[42]]}
            if '"country"' in sql:
                raise HTTPException(504, "Engine statement exceeded the client bound (300s)")
            return {"rows": [["Chat", 10], ["API", 5]]}

        report = {}
        with patch.object(engine.meta, "execute", lambda *a, **k: None), \
             patch.object(engine, "_execute_dataset_query", run), \
             patch.object(engine, "_store_answer", lambda *a, **k: None):
            stored = engine._precompute_answers("24", "OpenSource", "kaveon_product", "events",
                                                columns, [], metrics, {}, report=report)
        self.assertEqual(stored, 2)                 # total + surface breakdown
        self.assertEqual([s["dimension"] for s in report["skipped"]], ["country"])
        self.assertIn("exceeded the client bound", report["skipped"][0]["reason"])


if __name__ == "__main__":
    unittest.main()
