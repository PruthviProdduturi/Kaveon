import sys
from types import SimpleNamespace
import unittest
from unittest.mock import patch


if "pyodbc" not in sys.modules:
    sys.modules["pyodbc"] = SimpleNamespace(Error=Exception)

from dlm import engine


class ChartFreshnessTests(unittest.TestCase):
    def test_native_catalog_build_queries_use_engine_bridge(self):
        bridge_result = {
            "columns": [{"name": "country"}, {"name": "total"}],
            "data": [["US", 42]],
        }
        with patch.object(engine.meta, "query_one", return_value={"engine_catalog": "OpenSource"}), \
             patch("services.engine_bridge.execute", return_value=bridge_result) as execute, \
             patch.object(engine.pool, "execute_query") as pool_execute:
            result = engine._execute_dataset_query(
                'SELECT "country", SUM("trips") AS total FROM "silver"."trips" GROUP BY "country"',
                "OpenSource",
            )

        self.assertEqual(result["columns"], ["country", "total"])
        self.assertEqual(result["rows"], [["US", 42]])
        self.assertEqual(result["rows_objects"], [{"country": "US", "total": 42}])
        execute.assert_called_once()
        pool_execute.assert_not_called()

    def test_external_catalog_build_queries_keep_database_pool(self):
        expected = {"rows": [[42]]}
        with patch.object(engine.meta, "query_one", return_value=None), \
             patch.object(engine.pool, "execute_query", return_value=expected) as execute:
            result = engine._execute_dataset_query("SELECT COUNT(*)", "warehouse")

        self.assertIs(result, expected)
        execute.assert_called_once_with("SELECT COUNT(*)", "warehouse")

    def test_stale_single_metric_context_falls_back_and_starts_rebuild(self):
        dataset = {"id": "7", "metrics": [{"name": "Trips", "expression": "SUM(trips)"}]}
        with patch.object(engine.datasets_svc, "get_dataset_by_id", return_value=dataset), \
             patch.object(engine, "check_freshness", return_value={
                 "fresh": False, "score": 0.31, "recommendation": "rebuild",
             }), \
             patch.object(engine, "_trigger_background_rebuild", return_value=True) as rebuild, \
             patch.object(engine, "_serve_from_context") as serve:
            result = engine.serve_chart("7", "trips", "SUM")

        self.assertEqual(result, {
            "served": False,
            "reason": "stale_context",
            "freshness": {"score": 0.31, "recommendation": "rebuild"},
            "rebuild_triggered": True,
        })
        rebuild.assert_called_once_with("7")
        serve.assert_not_called()

    def test_stale_multi_metric_context_falls_back_and_respects_cooldown(self):
        dataset = {"id": "7", "metrics": [{"name": "Trips", "expression": "SUM(trips)"}]}
        with patch.object(engine.datasets_svc, "get_dataset_by_id", return_value=dataset), \
             patch.object(engine, "check_freshness", return_value={
                 "fresh": True, "score": 0.64, "recommendation": "rebuild",
             }), \
             patch.object(engine, "_trigger_background_rebuild", return_value=False) as rebuild, \
             patch.object(engine, "_serve_from_context") as serve:
            result = engine.serve_chart_multi(
                "7", [{"column": "trips", "aggregation": "SUM"}]
            )

        self.assertFalse(result["served"])
        self.assertEqual(result["reason"], "stale_context")
        self.assertFalse(result["rebuild_triggered"])
        rebuild.assert_called_once_with("7")
        serve.assert_not_called()

    def test_current_or_unsupported_context_continues_to_normal_resolution(self):
        for recommendation in ("use_context", "no_context"):
            with self.subTest(recommendation=recommendation), \
                 patch.object(engine, "check_freshness", return_value={
                     "fresh": recommendation == "use_context",
                     "score": 0.98 if recommendation == "use_context" else 0.0,
                     "recommendation": recommendation,
                 }), \
                 patch.object(engine, "_trigger_background_rebuild") as rebuild:
                self.assertIsNone(engine._stale_context_fallback("7"))
                rebuild.assert_not_called()

    def test_approximate_single_metric_answer_falls_back_to_exact_sql(self):
        dataset = {"id": "7", "metrics": [{"name": "Trips", "expression": "SUM(trips)"}]}
        with patch.object(engine.datasets_svc, "get_dataset_by_id", return_value=dataset), \
             patch.object(engine, "_stale_context_fallback", return_value=None), \
             patch.object(engine, "_serve_from_context", return_value={
                 "columns": ["Trips"], "rows": [[42]], "approx": True,
             }):
            result = engine.serve_chart("7", "trips", "SUM")

        self.assertEqual(result, {"served": False, "reason": "approximate_context"})

    def test_approximate_multi_metric_answer_falls_back_to_exact_sql(self):
        dataset = {
            "id": "7",
            "metrics": [
                {"name": "Trips", "expression": "SUM(trips)"},
                {"name": "Revenue", "expression": "SUM(revenue)"},
            ],
        }
        with patch.object(engine.datasets_svc, "get_dataset_by_id", return_value=dataset), \
             patch.object(engine, "_stale_context_fallback", return_value=None), \
             patch.object(engine, "_effective_spec", return_value={
                 "dimensions": {"country": {}},
             }), \
             patch.object(engine, "_serve_from_context", return_value={
                 "columns": ["country", "metric"], "rows": [["US", 42]], "approx": True,
             }):
            result = engine.serve_chart_multi("7", [
                {"column": "trips", "aggregation": "SUM"},
                {"column": "revenue", "aggregation": "SUM"},
            ], group_by="country")

        self.assertEqual(result, {
            "served": False, "reason": "approximate_context", "detail": "Trips",
        })


if __name__ == "__main__":
    unittest.main()
