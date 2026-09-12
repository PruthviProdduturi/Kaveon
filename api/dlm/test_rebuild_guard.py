"""Rebuilds happen because data changed, never because a clock ran out; one
rebuild per dataset at a time; build-time scans carry a statement bound so a
single large table cannot hold a small server for hours."""
import sys
import threading
import unittest
from types import SimpleNamespace
from unittest.mock import patch

if "pyodbc" not in sys.modules:
    sys.modules["pyodbc"] = SimpleNamespace(Error=Exception)

import database.pool as pool
from dlm import engine


def _artifact(status="ready", built_hours_ago=48.0):
    from datetime import datetime, timedelta, timezone
    built = datetime.now(timezone.utc).replace(tzinfo=None) - timedelta(hours=built_hours_ago)
    return {"built_at": built.isoformat(), "status": status,
            "stats_rollup": '{"row_counts": {"events": 1000}}',
            "manifest": '{"fact_table": "events", "schema": "public"}'}


class FreshnessTests(unittest.TestCase):
    def _freshness(self, art, live):
        with patch.object(engine, "ensure_tables", lambda: None), \
             patch.object(engine.meta, "query_one", return_value=art), \
             patch.object(engine.datasets_svc, "get_dataset_by_id", return_value={"database_name": "kaveon"}), \
             patch("dlm.validity._live_change", return_value=live):
            return engine.check_freshness("7")

    def test_an_old_artifact_over_unchanged_data_is_served_not_rebuilt(self):
        f = self._freshness(_artifact(built_hours_ago=72), {"events": {"mods_since_analyze": 0}})
        self.assertTrue(f["fresh"])
        self.assertFalse(f["data_modified"])
        self.assertEqual(f["recommendation"], "use_context")
        self.assertLess(f["score"], 0.5)   # age still lowers confidence; it just is not a trigger

    def test_a_large_change_recommends_a_rebuild(self):
        f = self._freshness(_artifact(built_hours_ago=1), {"events": {"mods_since_analyze": 900}})
        self.assertFalse(f["fresh"])
        self.assertTrue(f["data_modified"])
        self.assertEqual(f["recommendation"], "rebuild")

    def test_a_small_change_keeps_serving_context(self):
        f = self._freshness(_artifact(built_hours_ago=1), {"events": {"mods_since_analyze": 3}})
        self.assertTrue(f["fresh"])
        self.assertEqual(f["recommendation"], "use_context")

    def test_no_change_signal_means_no_automatic_rebuild(self):
        f = self._freshness(_artifact(built_hours_ago=200), {})
        self.assertEqual(f["recommendation"], "use_context")

    def test_a_building_artifact_is_not_context(self):
        f = self._freshness(_artifact(status="building"), {"events": {"mods_since_analyze": 900}})
        self.assertEqual(f["recommendation"], "no_context")


class RebuildGuardTests(unittest.TestCase):
    def setUp(self):
        engine._REBUILD_ACTIVE.clear()
        engine._REBUILD_COMPLETED_AT.clear()

    def test_a_second_trigger_while_one_runs_is_refused(self):
        started = threading.Event()
        release = threading.Event()

        def slow_generate(dataset_id, force=False):
            started.set()
            release.wait(5)

        with patch.object(engine, "generate_dlm", slow_generate):
            self.assertTrue(engine._trigger_background_rebuild("7"))
            self.assertTrue(started.wait(5))
            with patch.object(engine, "_REBUILD_COOLDOWN_SECONDS", 0.0):
                self.assertFalse(engine._trigger_background_rebuild("7"))
            release.set()
            for _ in range(50):
                if "7" not in engine._REBUILD_ACTIVE:
                    break
                threading.Event().wait(0.05)
        self.assertNotIn("7", engine._REBUILD_ACTIVE)

    def test_cooldown_counts_from_completion(self):
        with patch.object(engine, "generate_dlm", lambda dataset_id, force=False: None):
            self.assertTrue(engine._trigger_background_rebuild("7"))
            for _ in range(50):
                if "7" in engine._REBUILD_COMPLETED_AT:
                    break
                threading.Event().wait(0.05)
            self.assertFalse(engine._trigger_background_rebuild("7"))
            with patch.object(engine, "_REBUILD_COOLDOWN_SECONDS", 0.0):
                self.assertTrue(engine._trigger_background_rebuild("7"))


class BoundedScanTests(unittest.TestCase):
    def test_postgres_scan_carries_a_local_statement_timeout(self):
        seen = {}

        class Conn:
            connection = SimpleNamespace(timeout=0)
            def execute_query(self, sql, params=None, max_rows=None):
                seen["sql"] = sql
                return {"rows": []}

        fake_pool = SimpleNamespace(db_type="postgresql", get_connection=lambda: Conn(),
                                    return_connection=lambda c: None, discard_connection=lambda c: None)
        with patch.object(pool, "get_connection_pool", return_value=fake_pool):
            pool.execute_query("SELECT 1", "kaveon", timeout_seconds=120)
        self.assertTrue(seen["sql"].startswith("SET LOCAL statement_timeout = 120000; SELECT 1"))

    def test_mysql_scan_uses_the_execution_time_hint(self):
        seen = {}

        class Conn:
            def execute_query(self, sql, params=None, max_rows=None):
                seen["sql"] = sql
                return {"rows": []}

        fake_pool = SimpleNamespace(db_type="mysql", get_connection=lambda: Conn(),
                                    return_connection=lambda c: None, discard_connection=lambda c: None)
        with patch.object(pool, "get_connection_pool", return_value=fake_pool):
            pool.execute_query("select region from t", "shop", timeout_seconds=30)
        self.assertEqual(seen["sql"], "select /*+ MAX_EXECUTION_TIME(30000) */ region from t")

    def test_sql_server_scan_bounds_the_driver_timeout_and_restores_it(self):
        state = SimpleNamespace(timeout=0)
        seen = {}

        class Conn:
            connection = state
            def execute_query(self, sql, params=None, max_rows=None):
                seen["timeout_during"] = state.timeout
                return {"rows": []}

        fake_pool = SimpleNamespace(db_type="azure_sql", get_connection=lambda: Conn(),
                                    return_connection=lambda c: None, discard_connection=lambda c: None)
        with patch.object(pool, "get_connection_pool", return_value=fake_pool):
            pool.execute_query("SELECT 1", "wh", timeout_seconds=45)
        self.assertEqual(seen["timeout_during"], 45)
        self.assertEqual(state.timeout, 0)

    def test_distinct_scan_is_bounded(self):
        with patch.object(engine, "_execute_dataset_query", return_value={"rows_objects": []}) as run:
            engine._scan_distinct("kaveon", "public", "events", "surface", 50)
        self.assertEqual(run.call_args.kwargs.get("timeout_seconds"), engine._SCAN_TIMEOUT_SECONDS)

    def test_build_queries_default_to_the_build_bound_on_the_pool_path(self):
        with patch.object(engine.meta, "query_one", return_value=None), \
             patch.object(engine.pool, "execute_query", return_value={"rows": []}) as run:
            engine._execute_dataset_query("SELECT COUNT(*) FROM t", "kaveon")
        self.assertEqual(run.call_args.kwargs.get("timeout_seconds"), engine._BUILD_QUERY_TIMEOUT_SECONDS)


if __name__ == "__main__":
    unittest.main()
