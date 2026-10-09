import unittest
from unittest.mock import patch

from fastapi import HTTPException, Response

from middleware.auth import UserContext
from routers import engine_console
from services import engine_bridge


class EngineConsoleTests(unittest.TestCase):
    def test_cluster_uses_bridge_with_verified_principal_and_read_role(self):
        ctx = UserContext("viewer@example.com", "Viewer")
        payload = {"environment": "aks-test", "active_workers": 3, "workers": []}
        with patch.object(engine_bridge, "_request", return_value=payload) as request:
            self.assertEqual(engine_console.console_cluster(ctx), payload)
        method, path, token_name, actor = request.call_args.args
        self.assertEqual((method, path, token_name, actor), ("GET", "/v1/cluster", "KAVEON_ENGINE_BRIDGE_TOKEN", "viewer@example.com"))
        self.assertEqual(request.call_args.kwargs["role"], "reader")

    def test_queries_pass_through_engine_scoped_listing(self):
        ctx = UserContext("admin@example.com", "Admin")
        listing = [{"id": "q1", "state": "FINISHED"}]
        with patch.object(engine_bridge, "_request", return_value=listing) as request:
            self.assertEqual(engine_console.console_queries(ctx), listing)
        # The console lists queries in summary form; the full per-query record
        # is fetched only when one is opened.
        self.assertEqual(request.call_args.args[1], "/v1/query?summary=true")
        self.assertEqual(request.call_args.kwargs["role"], "admin")

    def test_query_lookup_encodes_id_and_maps_missing_to_404(self):
        ctx = UserContext("analyst@example.com", "Analyst")
        with patch.object(engine_bridge, "_request", return_value={"id": "a/b"}) as request:
            self.assertEqual(engine_console.console_query("a/b", ctx), {"id": "a/b"})
        self.assertEqual(request.call_args.args[1], "/v1/query/a%2Fb")
        with patch.object(engine_bridge, "_request", return_value=None):
            with self.assertRaises(HTTPException) as error:
                engine_console.console_query("missing", ctx)
        self.assertEqual(error.exception.status_code, 404)

    def test_invalid_engine_payloads_fail_closed(self):
        ctx = UserContext("viewer@example.com", "Viewer")
        for handler, bad in ((engine_console.console_cluster, []), (engine_console.console_queries, {})):
            with patch.object(engine_bridge, "_request", return_value=bad):
                with self.assertRaises(HTTPException) as error:
                    handler(ctx)
            self.assertEqual(error.exception.status_code, 502)

    def test_unknown_role_is_rejected_before_reaching_engine(self):
        ctx = UserContext("nobody@example.com", "NoAccess")
        with patch.object(engine_bridge, "_request") as request:
            with self.assertRaises(HTTPException) as error:
                engine_console.console_cluster(ctx)
        self.assertEqual(error.exception.status_code, 403)
        request.assert_not_called()

    def test_statistics_are_admin_scoped_and_bounded_bridge_data(self):
        ctx = UserContext("admin@example.com", "Admin")
        payload = {"statistics": [{"table": "OpenSource.ai_benchmarks.leaderboard", "row_count": 34, "current": True}], "total": 1, "truncated": False}
        with patch.object(engine_bridge, "_request", return_value=payload) as request:
            self.assertEqual(engine_console.console_statistics(ctx), payload)
        self.assertEqual(request.call_args.args[1], "/v1/statistics")
        self.assertEqual(request.call_args.kwargs["role"], "admin")

    def test_qualification_uses_fixed_native_table_and_requires_capability(self):
        ctx = UserContext("admin@example.com", "Admin")
        with patch.object(engine_bridge, "native_analyze_supported", return_value=True), \
             patch.object(engine_bridge, "execute", return_value={"id": "q1", "state": "FINISHED"}) as execute:
            qualified = engine_console.qualify_native_statistics(ctx)
            self.assertEqual(qualified["query"]["id"], "q1")
            self.assertIs(qualified["capability"]["native_analyze"], True)
        execute.assert_called_once_with('ANALYZE "ai_benchmarks"."leaderboard"', "OpenSource", "admin@example.com", "Admin", schema="ai_benchmarks")
        with patch.object(engine_bridge, "native_analyze_supported", return_value=False):
            with self.assertRaises(HTTPException) as error:
                engine_console.qualify_native_statistics(ctx)
        self.assertEqual(error.exception.status_code, 409)

    def test_system_catalog_structure_costs_no_engine_read(self):
        ctx = UserContext("admin@example.com", "Admin")
        with patch.object(engine_bridge, "_request") as request:
            reading = engine_console.system_catalog(Response(), False, False, ctx)
        request.assert_not_called()
        self.assertIs(reading["counted"], False)
        self.assertEqual(reading["catalog"], {"identifier": "kaveon", "schema": "product"})
        self.assertEqual(len(reading["tables"]), 14)
        # Every row carries the identifier the Engine resolves, never the
        # product name: a reader copying a name out of Studio must get one
        # that parses.
        self.assertEqual(reading["tables"][0]["identifier"], "kaveon.product.datasets")
        for row in reading["tables"]:
            self.assertTrue(row["identifier"].startswith("kaveon.product."))
            self.assertIsNone(row["records"])
            self.assertIsNone(row["error"])

    def test_system_catalog_counts_are_bounded_and_snapshot_pinned(self):
        ctx = UserContext("admin@example.com", "Admin")
        engine_console._SYSTEM_CATALOG_CACHE.clear()
        page = {"records": [{"id": "r1", "document": {}}], "snapshot_id": "snap-1", "next_cursor": None}
        with patch.object(engine_bridge, "_request", return_value=page) as request:
            reading = engine_console.system_catalog(Response(), True, False, ctx)
        self.assertIs(reading["counted"], True)
        self.assertEqual(reading["snapshot"], {"id": "snap-1", "consistent": True})
        self.assertTrue(all(row["records"] == 1 for row in reading["tables"]))
        # One listing per family, addressed by record kind, through the bridge.
        self.assertEqual(request.call_count, 14)
        self.assertEqual(request.call_args_list[0].args[1], "/v1/products/dataset?limit=100")
        self.assertEqual(request.call_args_list[0].kwargs["role"], "admin")

    def test_system_catalog_counts_are_held_briefly_and_refresh_rereads(self):
        ctx = UserContext("admin@example.com", "Admin")
        engine_console._SYSTEM_CATALOG_CACHE.clear()
        page = {"records": [], "snapshot_id": "snap-1", "next_cursor": None}
        with patch.object(engine_bridge, "_request", return_value=page) as request:
            engine_console.system_catalog(Response(), True, False, ctx)
            self.assertEqual(request.call_count, 14)
            engine_console.system_catalog(Response(), True, False, ctx)
            self.assertEqual(request.call_count, 14)
            engine_console.system_catalog(Response(), True, True, ctx)
            self.assertEqual(request.call_count, 28)
        engine_console._SYSTEM_CATALOG_CACHE.clear()

    def test_system_catalog_keeps_a_refused_family_as_one_row(self):
        ctx = UserContext("admin@example.com", "Admin")
        engine_console._SYSTEM_CATALOG_CACHE.clear()

        def answer(method, path, *args, **kwargs):
            if path.startswith("/v1/products/chart"):
                raise HTTPException(502, "KaveonDB returned an invalid product list")
            return {"records": [], "snapshot_id": "snap-1", "next_cursor": None}

        with patch.object(engine_bridge, "_request", side_effect=answer):
            reading = engine_console.system_catalog(Response(), True, False, ctx)
        charts = next(row for row in reading["tables"] if row["table"] == "charts")
        self.assertEqual(charts["error"], "KaveonDB returned an invalid product list")
        self.assertIsNone(charts["records"])
        self.assertEqual(next(row for row in reading["tables"] if row["table"] == "datasets")["records"], 0)
        engine_console._SYSTEM_CATALOG_CACHE.clear()

    def test_system_catalog_reports_a_commit_that_landed_mid_reading(self):
        ctx = UserContext("admin@example.com", "Admin")
        engine_console._SYSTEM_CATALOG_CACHE.clear()
        snapshots = iter([f"snap-{index}" for index in range(14)])
        with patch.object(engine_bridge, "_request",
                          side_effect=lambda *a, **k: {"records": [], "snapshot_id": next(snapshots), "next_cursor": None}):
            reading = engine_console.system_catalog(Response(), True, False, ctx)
        self.assertIsNone(reading["snapshot"]["id"])
        self.assertIs(reading["snapshot"]["consistent"], False)
        engine_console._SYSTEM_CATALOG_CACHE.clear()


if __name__ == "__main__":
    unittest.main()
