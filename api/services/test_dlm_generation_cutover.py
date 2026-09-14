import unittest
from unittest.mock import patch

from services import dlm_generation_cutover as cutover


def payload():
    return {
        "dataset_id": "7", "manifest": {}, "stats_rollup": {}, "usage_rollup": {},
        "source_hash": "source", "built_at": "2026-09-14T00:00:00Z",
        "status": "ready", "values_indexed": 0,
    }


class DlmGenerationCutoverTests(unittest.TestCase):
    def test_new_definition_and_run_commit_atomically_after_artifact(self):
        dataset = {"revision": 4, "document": {"created_by": "owner@example.test"}}
        artifact = {"path": "dlm/7/v1/compiled.json", "sha256": "a" * 64,
                    "bytes": 10, "version": 1}
        order = []
        with patch.object(cutover.product_store, "read", side_effect=[dataset, None]), \
             patch.object(cutover.product_store, "list_records", return_value=[]), \
             patch.object(cutover.dlm_compiled_artifact, "publish",
                          side_effect=lambda value: order.append("artifact") or artifact) as publish, \
             patch.object(cutover.product_store, "transact",
                          side_effect=lambda *args: order.append("transaction")) as transact:
            result = cutover.publish(payload(), "owner@example.test")
        self.assertEqual(order, ["artifact", "transaction"])
        self.assertEqual(result["run_id"], "7-v1")
        self.assertEqual(publish.call_args.args[0]["version"], 1)
        mutations = transact.call_args.args[0]
        self.assertEqual([(m.operation, m.kind, m.record_id) for m in mutations], [
            ("create", "dlm_definition", "7"),
            ("create", "dlm_run", "7-v1"),
            ("update", "dlm_run", "7-v1"),
        ])
        self.assertEqual(mutations[-1].expected_revision, 1)
        self.assertEqual(transact.call_args.args[1:], ("owner@example.test", "Admin"))

    def test_dataset_revision_change_uses_definition_cas_and_next_run(self):
        dataset = {"revision": 9, "document": {"created_by": "owner"}}
        definition = {"revision": 3, "document": {"dataset_id": "7", "dataset_revision": 8}}
        runs = [{"id": "7-v4", "revision": 2,
                 "document": {"definition_id": "7", "definition_revision": 3,
                              "status": "ready", "artifact": {}}}]
        artifact = {"path": "dlm/7/v5/compiled.json", "sha256": "b" * 64,
                    "bytes": 10, "version": 5}
        with patch.object(cutover.product_store, "read", side_effect=[dataset, definition]), \
             patch.object(cutover.product_store, "list_records", return_value=runs), \
             patch.object(cutover.dlm_compiled_artifact, "publish", return_value=artifact), \
             patch.object(cutover.product_store, "transact") as transact:
            result = cutover.publish(payload(), "owner")
        definition_update = transact.call_args.args[0][0]
        self.assertEqual((definition_update.operation, definition_update.expected_revision), ("update", 3))
        self.assertEqual(definition_update.document["dataset_revision"], 9)
        self.assertEqual((result["definition_revision"], result["run_id"]), (4, "7-v5"))

    def test_unchanged_definition_is_not_rewritten(self):
        dataset = {"revision": 9, "document": {"created_by": "owner"}}
        definition = {"revision": 3, "document": {"dataset_id": "7", "dataset_revision": 9}}
        artifact = {"path": "dlm/7/v1/compiled.json", "sha256": "b" * 64,
                    "bytes": 10, "version": 1}
        with patch.object(cutover.product_store, "read", side_effect=[dataset, definition]), \
             patch.object(cutover.product_store, "list_records", return_value=[]), \
             patch.object(cutover.dlm_compiled_artifact, "publish", return_value=artifact), \
             patch.object(cutover.product_store, "transact") as transact:
            cutover.publish(payload(), "owner")
        self.assertEqual([m.kind for m in transact.call_args.args[0]], ["dlm_run", "dlm_run"])

    def test_owner_or_invalid_run_fails_before_artifact_publication(self):
        cases = (
            ({"revision": 1, "document": {"created_by": "other"}}, [], "owner"),
            ({"revision": 1, "document": {"created_by": "owner"}},
             [{"id": "7-v1", "revision": 2, "document": {"definition_id": "8"}}], "identity"),
        )
        for dataset, runs, message in cases:
            with self.subTest(message=message), \
                 patch.object(cutover.product_store, "read", side_effect=[dataset, None]), \
                 patch.object(cutover.product_store, "list_records", return_value=runs), \
                 patch.object(cutover.dlm_compiled_artifact, "publish") as publish, \
                 self.assertRaisesRegex(RuntimeError, message):
                cutover.publish(payload(), "owner")
            publish.assert_not_called()

    def test_disabled_artifact_publication_prevents_transaction(self):
        dataset = {"revision": 1, "document": {"created_by": "owner"}}
        with patch.object(cutover.product_store, "read", side_effect=[dataset, None]), \
             patch.object(cutover.product_store, "list_records", return_value=[]), \
             patch.object(cutover.dlm_compiled_artifact, "publish", return_value=None), \
             patch.object(cutover.product_store, "transact") as transact, \
             self.assertRaisesRegex(RuntimeError, "disabled"):
            cutover.publish(payload(), "owner")
        transact.assert_not_called()


if __name__ == "__main__":
    unittest.main()
