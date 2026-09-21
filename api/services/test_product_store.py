import unittest
from unittest.mock import call, patch

from fastapi import HTTPException

from services import product_store


class ProductStoreTests(unittest.TestCase):
    def test_dlm_definition_uses_typed_plural_table(self):
        mutation = product_store.ProductMutation(
            "create", "dlm_definition", "7", {"dataset_id": "7", "dataset_revision": 3}
        )
        statement = product_store._statement(mutation)
        self.assertIn("kaveon.product.dlm_definitions", statement)
        self.assertIn('dataset_revision', statement)

    def test_dlm_run_uses_typed_plural_table_and_canonical_document(self):
        mutation = product_store.ProductMutation(
            "create", "dlm_run", "run-1",
            {"status": "building", "artifact": None, "definition_revision": 2, "definition_id": "7"},
        )
        statement = product_store._statement(mutation)
        self.assertIn("kaveon.product.dlm_runs", statement)
        self.assertIn('{"artifact":null,"definition_id":"7","definition_revision":2,"status":"building"}', statement)

    def test_multi_record_transaction_uses_one_session_and_canonical_documents(self):
        mutations = [
            product_store.ProductMutation("create", "chart", "chart-1", {"z": 2, "name": "O'Reilly"}),
            product_store.ProductMutation("update", "dashboard", "dash-1", {"charts": ["chart-1"]}, 4),
        ]
        with patch.object(product_store.engine_bridge, "_request", side_effect=[
            {"transaction_id": "tx-1"}, {}, {}, {"generation": 8},
        ]) as request:
            result = product_store.transact(mutations, "alice@example.com", "Editor")
        self.assertEqual(result, {"generation": 8})
        payloads = [item.kwargs["payload"] for item in request.call_args_list]
        self.assertEqual(payloads[0], {"sql": "BEGIN"})
        self.assertIn("'{\"name\":\"O''Reilly\",\"z\":2}'", payloads[1]["sql"])
        self.assertIn("revision = 4", payloads[2]["sql"])
        self.assertEqual(payloads[3], {"sql": "COMMIT", "transaction_id": "tx-1"})
        self.assertTrue(all(item.kwargs["role"] == "analyst" for item in request.call_args_list))

    def test_stage_failure_attempts_rollback_and_preserves_original_error(self):
        failure = HTTPException(409, "conflict")
        mutation = product_store.ProductMutation("delete", "chart", "chart-1", expected_revision=2)
        with patch.object(product_store.engine_bridge, "_request", side_effect=[
            {"transaction_id": "tx-2"}, failure, HTTPException(502, "rollback failed"),
        ]) as request:
            with self.assertRaises(HTTPException) as error:
                product_store.transact([mutation], "alice@example.com", "Admin")
        self.assertIs(error.exception, failure)
        self.assertEqual(request.call_args_list[-1].kwargs["payload"], {
            "sql": "ROLLBACK", "transaction_id": "tx-2",
        })

    def test_invalid_mutation_fails_before_opening_a_session(self):
        mutation = product_store.ProductMutation("update", "dataset", "dataset-1", {"name": "x"})
        with patch.object(product_store.engine_bridge, "_request") as request:
            with self.assertRaises(HTTPException) as error:
                product_store.transact([mutation], "alice@example.com", "Admin")
        self.assertEqual(error.exception.status_code, 422)
        request.assert_not_called()

    def test_read_is_owner_scoped_and_uses_encoded_path(self):
        with patch.object(product_store.engine_bridge, "_request", return_value={"revision": 3}) as request:
            result = product_store.read("dashboard", "dash 1", "alice@example.com", "Viewer")
        self.assertEqual(result, {"revision": 3})
        self.assertEqual(request.call_args, call(
            "GET", "/v1/product/dashboard/dash%201", "KAVEON_ENGINE_BRIDGE_TOKEN",
            "alice@example.com", role="reader",
        ))

    def test_migration_calls_use_only_an_allowlisted_owner_override(self):
        environment = {
            "KAVEON_MIGRATION_OWNER_PRINCIPAL": "prproddu-test",
            "KAVEON_MIGRATION_OWNER_ALLOWLIST": "migration-bot,prproddu-test",
        }
        with patch.dict("os.environ", environment, clear=False), \
             patch.object(product_store, "read", return_value={"revision": 1}) as read:
            result = product_store.migration_read("source", "source-1", "system", "Admin")
        self.assertEqual(result, {"revision": 1})
        read.assert_called_once_with("source", "source-1", "prproddu-test", "Admin")

    def test_migration_owner_override_fails_closed_without_allowlist(self):
        environment = {
            "KAVEON_MIGRATION_OWNER_PRINCIPAL": "prproddu-test",
            "KAVEON_MIGRATION_OWNER_ALLOWLIST": "migration-bot",
        }
        with patch.dict("os.environ", environment, clear=False), \
             patch.object(product_store, "read") as read, \
             self.assertRaisesRegex(RuntimeError, "not allowlisted"):
            product_store.migration_read("source", "source-1", "system", "Admin")
        read.assert_not_called()

    def test_normal_product_calls_ignore_migration_override(self):
        environment = {
            "KAVEON_MIGRATION_OWNER_PRINCIPAL": "prproddu-test",
            "KAVEON_MIGRATION_OWNER_ALLOWLIST": "prproddu-test",
        }
        with patch.dict("os.environ", environment, clear=False), \
             patch.object(product_store.engine_bridge, "_request", return_value={}) as request:
            product_store.read("source", "source-1", "system", "Admin")
        self.assertEqual(request.call_args.args[3], "system")

    def test_migration_create_retains_canonical_record_owner(self):
        environment = {
            "KAVEON_MIGRATION_OWNER_PRINCIPAL": "prproddu-test",
            "KAVEON_MIGRATION_OWNER_ALLOWLIST": "prproddu-test",
        }
        mutation = product_store.ProductMutation("create", "user_recent", "recent-1", {"id": "1"})
        with patch.dict("os.environ", environment, clear=False), \
             patch.object(product_store, "transact", return_value={}) as transact:
            product_store.migration_transact([mutation], "user@example.com", "Admin")
        transact.assert_called_once_with((mutation,), "user@example.com", "Admin")

    def test_rejects_unknown_role_and_unsafe_identifier(self):
        with self.assertRaises(HTTPException) as role_error:
            product_store.read("dataset", "safe", "alice@example.com", "Owner")
        self.assertEqual(role_error.exception.status_code, 403)
        with self.assertRaises(HTTPException) as id_error:
            product_store.read("dataset", "../unsafe", "alice@example.com", "Admin")
        self.assertEqual(id_error.exception.status_code, 422)

    def test_list_records_paginates_one_immutable_snapshot(self):
        pages = [
            {"snapshot_id": "snapshot-1", "records": [{"document": {"id": "1"}}], "next_cursor": "next"},
            {"snapshot_id": "snapshot-1", "records": [{"document": {"id": "2"}}], "next_cursor": None},
        ]
        with patch.object(product_store.engine_bridge, "_request", side_effect=pages) as request:
            records = product_store.list_records("dataset", "alice@example.com", "Admin")
        self.assertEqual([record["document"]["id"] for record in records], ["1", "2"])
        self.assertIn("cursor=next", request.call_args_list[1].args[1])

    def test_list_records_rejects_snapshot_drift_and_malformed_records(self):
        drift = [
            {"snapshot_id": "one", "records": [], "next_cursor": "next"},
            {"snapshot_id": "two", "records": [], "next_cursor": None},
        ]
        with patch.object(product_store.engine_bridge, "_request", side_effect=drift), \
             self.assertRaisesRegex(HTTPException, "changed during pagination"):
            product_store.list_records("dataset", "alice@example.com", "Admin")
        with patch.object(product_store.engine_bridge, "_request", return_value={
            "snapshot_id": "one", "records": [{"document": "invalid"}], "next_cursor": None,
        }), self.assertRaisesRegex(HTTPException, "invalid product list record"):
            product_store.list_records("dataset", "alice@example.com", "Admin")


if __name__ == "__main__":
    unittest.main()
