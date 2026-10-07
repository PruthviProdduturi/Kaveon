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
        self.assertEqual(order, ["artifact", "transaction", "transaction"])
        self.assertEqual(result["run_id"], "7-v1")
        self.assertEqual(publish.call_args.args[0]["version"], 1)
        first, second = [call.args[0] for call in transact.call_args_list]
        self.assertEqual([(m.operation, m.kind, m.record_id) for m in first], [
            ("create", "dlm_definition", "7"),
            ("create", "dlm_run", "7-v1"),
        ])
        self.assertEqual([(m.operation, m.kind, m.record_id) for m in second], [
            ("update", "dlm_run", "7-v1"),
        ])
        self.assertEqual(second[0].expected_revision, 1)
        for call in transact.call_args_list:
            self.assertEqual(call.args[1:], ("owner@example.test", "Admin"))

    def test_orphaned_artifact_version_is_stepped_over_not_overwritten(self):
        """A publication that wrote its bytes then failed to commit its run
        leaves an object no run references, so the version still looks free.
        Retrying rebuilds bytes that differ by at least `built_at`, finds the
        slot taken, and before this would fail reconciliation forever — one
        failed attempt wedged the dataset permanently."""
        dataset = {"revision": 1, "document": {"created_by": "owner"}}
        attempted = []

        def publish(value):
            attempted.append(value["version"])
            if value["version"] < 3:
                raise cutover.dlm_compiled_artifact.VersionOccupied("taken")
            return {"path": f"dlm/7/v{value['version']}/compiled.json",
                    "sha256": "c" * 64, "bytes": 10, "version": value["version"]}

        with patch.object(cutover.product_store, "read", side_effect=[dataset, None]),              patch.object(cutover.product_store, "list_records", return_value=[]),              patch.object(cutover.dlm_compiled_artifact, "publish", side_effect=publish),              patch.object(cutover.product_store, "transact"):
            result = cutover.publish(payload(), "owner")
        self.assertEqual(attempted, [1, 2, 3])
        self.assertEqual(result["run_id"], "7-v3")

    def test_wedged_artifact_store_reports_rather_than_looping(self):
        dataset = {"revision": 1, "document": {"created_by": "owner"}}
        with patch.object(cutover.product_store, "read", side_effect=[dataset, None]),              patch.object(cutover.product_store, "list_records", return_value=[]),              patch.object(cutover.dlm_compiled_artifact, "publish",
                          side_effect=cutover.dlm_compiled_artifact.VersionOccupied("taken")),              patch.object(cutover.product_store, "transact") as transact:
            with self.assertRaisesRegex(RuntimeError, "No free compiled DLM artifact version"):
                cutover.publish(payload(), "owner")
        transact.assert_not_called()

    def test_admin_may_rebuild_a_dataset_it_does_not_own(self):
        """Seeded datasets are owned by `system`, which nobody can sign in as.
        Requiring caller == owner therefore left every one of them permanently
        without context — including the 504M-row events dataset every slow
        dashboard reads. The records are still written as the owner."""
        dataset = {"revision": 1, "document": {"created_by": "system"}}
        artifact = {"path": "dlm/7/v1/compiled.json", "sha256": "a" * 64,
                    "bytes": 10, "version": 1}
        with patch.object(cutover.product_store, "read", side_effect=[dataset, None]),              patch.object(cutover.product_store, "list_records", return_value=[]),              patch.object(cutover.dlm_compiled_artifact, "publish", return_value=artifact),              patch.object(cutover.product_store, "transact") as transact:
            cutover.publish(payload(), "admin@example.test", "Admin")
        for call in transact.call_args_list:
            self.assertEqual(call.args[1], "system")

    def test_a_non_admin_still_cannot_publish_for_another_owner(self):
        dataset = {"revision": 1, "document": {"created_by": "someone@example.test"}}
        with patch.object(cutover.product_store, "read", return_value=dataset),              patch.object(cutover.dlm_compiled_artifact, "publish") as publish:
            for role in ("Viewer", "Analyst", "Editor"):
                with self.subTest(role=role):
                    with self.assertRaisesRegex(RuntimeError, "owner or an Admin"):
                        cutover.publish(payload(), "other@example.test", role)
            publish.assert_not_called()

    def test_no_transaction_changes_one_record_twice(self):
        """A product commit applies each change once against a single base, so
        KaveonDB refuses a transaction that touches a record twice. Publishing
        once built the run's create and its ready update into one transaction
        and could therefore never commit — no DLM was publishable at all. The
        shape, not just the outcome, is what has to stay fixed."""
        dataset = {"revision": 1, "document": {"created_by": "owner@example.test"}}
        artifact = {"path": "dlm/7/v1/compiled.json", "sha256": "a" * 64,
                    "bytes": 10, "version": 1}
        with patch.object(cutover.product_store, "read", side_effect=[dataset, None]),              patch.object(cutover.product_store, "list_records", return_value=[]),              patch.object(cutover.dlm_compiled_artifact, "publish", return_value=artifact),              patch.object(cutover.product_store, "transact") as transact:
            cutover.publish(payload(), "owner@example.test")
        for call in transact.call_args_list:
            addressed = [(m.kind, m.record_id) for m in call.args[0]]
            self.assertEqual(len(addressed), len(set(addressed)), addressed)

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
        definition_update = transact.call_args_list[0].args[0][0]
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
        self.assertEqual([m.kind for call in transact.call_args_list
                          for m in call.args[0]], ["dlm_run", "dlm_run"])

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
