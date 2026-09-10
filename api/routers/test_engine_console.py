import unittest
from unittest.mock import patch

from fastapi import HTTPException

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
        self.assertEqual(request.call_args.args[1], "/v1/query")
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


if __name__ == "__main__":
    unittest.main()
