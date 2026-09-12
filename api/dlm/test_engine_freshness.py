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
        execute.assert_called_once_with(
            'SELECT country, SUM(trips) AS total FROM silver.trips GROUP BY country',
            "OpenSource", "kaveon-system", "Admin", "silver", timeout=engine._BUILD_QUERY_TIMEOUT_SECONDS,
        )
        pool_execute.assert_not_called()

    def test_native_analyze_carries_the_relation_schema(self):
        with patch.object(engine.meta, "query_one", return_value={"engine_catalog": "OpenSource"}), \
             patch("services.engine_bridge.execute", return_value={"columns": [], "data": []}) as execute:
            engine._execute_dataset_query('ANALYZE "kaveon_product"."kaveon_events_enriched"', "OpenSource")
        execute.assert_called_once_with(
            "ANALYZE kaveon_product.kaveon_events_enriched", "OpenSource", "kaveon-system", "Admin", "kaveon_product",
            timeout=engine._BUILD_QUERY_TIMEOUT_SECONDS,
        )

    def test_native_catalog_builds_its_value_index_by_bounded_scan(self):
        columns = [{"table_name": "trips", "column_name": "borough", "is_dimension": True},
                   {"table_name": "trips", "column_name": "fare", "is_dimension": False}]
        with patch.object(engine, "_native_catalog", return_value={"engine_catalog": "OpenSource"}), \
             patch.object(engine, "_scan_distinct", return_value=[("Brooklyn", 10.0), ("Queens", 4.0)]) as scan:
            rows = engine._value_inventory("7", "OpenSource", "nyc_taxi", columns, [], {}, stats_supported=False)
        scan.assert_called_once_with("OpenSource", "nyc_taxi", "trips", "borough", engine._MAX_CARDINALITY_FOR_VALUES)
        self.assertEqual([r["value_text"] for r in rows], ["Brooklyn", "Queens"])
        self.assertEqual({r["source"] for r in rows}, {"scan.group_by"})

    def test_force_rebuilds_a_ready_artifact_even_without_statistics(self):
        dataset = {"id": "24", "dataset_name": "T", "database_name": "OpenSource", "schema_name": "s",
                   "fact_table": "t", "columns": [], "metrics": [], "dimensions": []}
        stubs = {"_analyze_tables": None, "_value_inventory": [], "_usage_rollup": {}, "_stats_rollup": {},
                 "_native_row_counts": {}, "_manifest": {}, "_persist_value_index": None, "_upsert_artifact": None,
                 "_upsert_router": None, "_effective_spec": {}, "_curate_linked_dashboards": 0}
        from contextlib import ExitStack
        with ExitStack() as stack:
            stack.enter_context(patch.object(engine, "ensure_tables", lambda: None))
            stack.enter_context(patch.object(engine.datasets_svc, "get_dataset_by_id", return_value=dataset))
            stack.enter_context(patch.object(engine.profiler, "build_context", return_value={"supported": False}))
            stack.enter_context(patch.object(engine.meta, "query_one", return_value={"status": "ready"}))
            stack.enter_context(patch.object(engine.meta, "execute", lambda *a, **k: None))
            for name, value in stubs.items():
                stack.enter_context(patch.object(engine, name, lambda *a, _v=value, **k: _v))
            precompute = stack.enter_context(patch.object(engine, "_precompute_answers", return_value=3))
            result = engine.generate_dlm("24", force=True)
        self.assertTrue(result.get("rebuilt"), result)
        precompute.assert_called_once()

    def test_external_source_without_statistics_gets_no_value_index(self):
        columns = [{"table_name": "t", "column_name": "region", "is_dimension": True}]
        with patch.object(engine, "_native_catalog", return_value=None), \
             patch.object(engine, "_scan_distinct") as scan:
            rows = engine._value_inventory("7", "warehouse", "public", columns, [], {}, stats_supported=False)
        scan.assert_not_called()
        self.assertEqual(rows, [])

    def test_external_catalog_build_queries_keep_database_pool(self):
        expected = {"rows": [[42]]}
        with patch.object(engine.meta, "query_one", return_value=None), \
             patch.object(engine.pool, "execute_query", return_value=expected) as execute:
            result = engine._execute_dataset_query("SELECT COUNT(*)", "warehouse")

        self.assertIs(result, expected)
        execute.assert_called_once_with("SELECT COUNT(*)", "warehouse", timeout_seconds=engine._BUILD_QUERY_TIMEOUT_SECONDS)

    def test_native_row_counts_are_exact_engine_queries(self):
        with patch.object(engine.meta, "query_one", return_value={"engine_catalog": "OpenSource"}), \
             patch.object(engine, "_execute_dataset_query", side_effect=[
                 {"rows_objects": [{"row_count": 12}]},
                 {"rows": [[34]]},
             ]) as execute:
            counts = engine._native_row_counts(
                "OpenSource", "silver", ["orders", "customers", "orders"]
            )

        self.assertEqual(counts, {"customers": 12, "orders": 34})
        self.assertEqual(execute.call_count, 2)
        self.assertEqual(
            execute.call_args_list[0].args,
            ('SELECT COUNT(*) AS row_count FROM "silver"."customers"', "OpenSource"),
        )

    def test_external_catalog_row_count_never_scans_postgres(self):
        with patch.object(engine.meta, "query_one", return_value=None), \
             patch.object(engine, "_execute_dataset_query") as execute:
            counts = engine._native_row_counts("metadata", "public", ["datasets"])

        self.assertEqual(counts, {})
        execute.assert_not_called()

    def test_native_catalog_uses_engine_analyze(self):
        with patch.object(engine, "_native_catalog", return_value={"engine_catalog": "ai_benchmarks"}), \
             patch.object(engine.profiler, "supports_database", return_value=False), \
             patch("services.engine_bridge.native_analyze_supported", return_value=True), \
             patch.object(engine, "_execute_dataset_query") as execute:
            engine._analyze_tables("ai_benchmarks", "ai_benchmarks", ["leaderboard"])

        execute.assert_called_once_with(
            'ANALYZE "ai_benchmarks"."leaderboard"', "ai_benchmarks"
        )

    def test_native_analyze_stays_off_until_engine_contract_is_enabled(self):
        with patch.object(engine, "_native_catalog", return_value={"engine_catalog": "ai_benchmarks"}), \
             patch("services.engine_bridge.native_analyze_supported", return_value=False), \
             patch.object(engine, "_execute_dataset_query") as execute:
            engine._analyze_tables("ai_benchmarks", "ai_benchmarks", ["leaderboard"])

        execute.assert_not_called()

    def test_postgres_catalog_keeps_best_effort_analyze(self):
        with patch.object(engine, "_native_catalog", return_value=None), \
             patch.object(engine.profiler, "supports_database", return_value=True), \
             patch.object(engine, "_execute_dataset_query") as execute:
            engine._analyze_tables("metadata", "public", ["datasets", ""])

        execute.assert_called_once_with('ANALYZE "public"."datasets"', "metadata")

    def test_old_ready_artifact_backfills_engine_count_and_watermark(self):
        dataset = {
            "id": "7", "database_name": "OpenSource", "schema_name": "silver",
            "table_name": "trips", "columns": [], "dimensions": [],
        }
        stats = {"generation": {"answers_precomputed": 5}}
        with patch.object(engine.datasets_svc, "get_dataset_by_id", return_value=dataset), \
             patch.object(engine, "_native_row_counts", return_value={"trips": 5000}), \
             patch.object(engine.meta, "execute") as persist:
            counts = engine._backfill_native_row_counts("7", stats)

        self.assertEqual(counts, {"trips": 5000})
        self.assertEqual(stats["watermark"]["row_count"], 5000)
        self.assertEqual(stats["row_count_source"], "kaveon_engine_exact")
        stored = persist.call_args.args[1]
        self.assertIn('"row_counts": {"trips": 5000}', stored[0])
        self.assertEqual(stored[1], "7")

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
