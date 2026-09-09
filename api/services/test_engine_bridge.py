import unittest
from unittest.mock import patch
from types import SimpleNamespace
from fastapi import HTTPException
from services import engine_bridge as bridge


class EngineBridgeTests(unittest.TestCase):
    def setUp(self):
        self.source = {"id": "abc", "engine_catalog": "warehouse", "storage_type": "local",
                       "storage_config": {"base_path": "/data"}, "adapter_type": "native",
                       "lifecycle": "draft"}

    def test_stable_mapping_and_secret_fields_rejected(self):
        self.assertEqual(bridge.definition(self.source)["id"], "platform-abc")
        self.source["storage_config"]["password"] = "forbidden"
        with self.assertRaises(HTTPException):
            bridge.definition(self.source)

    def test_create_retry_is_idempotent(self):
        current = bridge.definition(self.source)
        with patch.object(bridge, "_request", side_effect=[None, current]) as request:
            self.assertEqual(bridge.sync_catalog(self.source, "admin")["catalog"], current)
            self.assertEqual(request.call_count, 2)
        with patch.object(bridge, "_request", return_value=current) as request:
            self.assertFalse(bridge.sync_catalog(self.source, "admin")["changed"])
            self.assertEqual(request.call_count, 1)

    def test_revision_conflict_prevents_overwrite(self):
        current = bridge.definition(self.source)
        self.source["engine_catalog"] = "renamed"
        with patch.object(bridge, "_request", return_value=current) as request:
            with self.assertRaises(HTTPException) as error:
                bridge.sync_catalog(self.source, "admin", 9)
            self.assertEqual(error.exception.status_code, 409)
            self.assertEqual(request.call_count, 1)

    def test_update_sends_compare_and_swap_and_actor(self):
        current = bridge.definition(self.source)
        self.source["lifecycle"] = "active"
        updated = {**current, "lifecycle": "Active", "revision": 2}
        with patch.object(bridge, "_request", side_effect=[current, updated]) as request:
            self.assertTrue(bridge.sync_catalog(self.source, "admin", 1)["changed"])
            args, kwargs = request.call_args
            self.assertEqual(args[0], "PUT")
            self.assertEqual(args[3], "admin")
            self.assertEqual(kwargs["revision"], 1)
            self.assertEqual(kwargs["payload"]["revision"], 2)

    def test_replayed_update_does_not_increment_revision(self):
        current = {**bridge.definition(self.source), "revision": 5}
        with patch.object(bridge, "_request", return_value=current) as request:
            self.assertFalse(bridge.sync_catalog(self.source, "admin", 4)["changed"])
            self.assertEqual(request.call_count, 1)

    def test_viewer_and_unknown_roles_cannot_delegate_sql(self):
        for role in ["Viewer", "NoAccess", "Owner"]:
            with patch.object(bridge, "_request") as request:
                with self.assertRaises(HTTPException):
                    bridge.execute("SELECT 1", "warehouse", "alice", role)
                request.assert_not_called()

    def test_verified_actor_and_role_forwarded(self):
        with patch.object(bridge, "_request", return_value={"id": "query"}) as request:
            bridge.execute("SELECT 1", "warehouse", "alice", "Analyst")
            self.assertEqual(request.call_args.args[3], "alice")
            self.assertEqual(request.call_args.kwargs["role"], "analyst")

    def test_successful_statement_is_enriched_with_its_query_record(self):
        details = {"id": "query-1", "timings": {"planning_us": 12}, "stages": []}
        with patch.object(bridge, "_request", side_effect=[{"id": "query-1", "data": []}, details]) as request:
            result = bridge.execute("SELECT 1", "warehouse", "alice", "Analyst")
        self.assertEqual(result["query_details"], details)
        self.assertEqual(request.call_args_list[1].args[1], "/v1/query/query-1")

    def test_external_plaintext_transport_fails_closed(self):
        with patch.dict("os.environ", {"KAVEON_ENGINE_URL": "http://engine.example", "KAVEON_ENGINE_PRIVATE_HTTP": "false"}):
            with self.assertRaises(HTTPException):
                bridge._endpoint()

    def test_catalog_reads_forward_only_a_valid_scoped_role(self):
        with patch.object(bridge, "_request", return_value={"schemas": ["test"]}) as request:
            self.assertEqual(bridge.schemas("warehouse", "viewer@example.com", "Viewer"), {"schemas": ["test"]})
            self.assertEqual(request.call_args.args[:4], ("GET", "/v1/catalog/warehouse/schema", "KAVEON_ENGINE_BRIDGE_TOKEN", "viewer@example.com"))
            self.assertEqual(request.call_args.kwargs["role"], "reader")
        with patch.object(bridge, "_request") as request:
            with self.assertRaises(HTTPException):
                bridge.schemas("warehouse", "unknown@example.com", "NoAccess")
            request.assert_not_called()

    def test_table_columns_follow_catalog_definition_metadata(self):
        responses = [
            [{"id": "catalog-id", "name": "kavedb"}],
            [{"id": "schema-id", "name": "silver"}],
            [{"name": "orders", "columns": [{"name": "order_id", "data_type": "Int64", "nullable": False}]}],
        ]
        with patch.object(bridge, "_request", side_effect=responses) as request:
            columns = bridge.table_columns("kavedb", "silver", "orders", "viewer@example.com", "Viewer")
        self.assertEqual(columns, [{"name": "order_id", "data_type": "Int64", "nullable": False}])
        self.assertEqual([call.args[1] for call in request.call_args_list], [
            "/v1/catalog/definitions", "/v1/catalog/definitions/catalog-id/schemas", "/v1/catalog/schemas/schema-id/tables",
        ])
        self.assertTrue(all(call.kwargs["role"] == "reader" for call in request.call_args_list))

    def test_private_ca_uses_a_verifying_context(self):
        with patch.dict("os.environ", {"KAVEON_ENGINE_CA_CERT": "C:/missing-ca.pem"}):
            with self.assertRaises(HTTPException) as error:
                bridge._verify_context()
            self.assertEqual(error.exception.status_code, 503)

    def test_engine_admission_rejection_is_retryable(self):
        response = SimpleNamespace(status_code=429, is_success=False)
        with patch.dict("os.environ", {"KAVEON_ENGINE_URL": "https://engine.example", "KAVEON_ENGINE_BRIDGE_TOKEN": "test"}), \
             patch.object(bridge, "_verify_context", return_value=True), \
             patch.object(bridge.httpx, "request", return_value=response):
            with self.assertRaises(HTTPException) as error:
                bridge._request("POST", "/v1/statement", "KAVEON_ENGINE_BRIDGE_TOKEN", "alice", payload={})
        self.assertEqual(error.exception.status_code, 429)
        self.assertEqual(error.exception.headers, {"Retry-After": "1"})


if __name__ == "__main__":
    unittest.main()
