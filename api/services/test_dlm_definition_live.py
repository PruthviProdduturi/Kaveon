import os
import unittest
from unittest.mock import Mock, patch

from services import dlm_definition_mutations as mutations
from services import product_shadow_read as shadow


class DlmDefinitionMutationTests(unittest.TestCase):
    def test_ready_publication_binds_owner_and_exact_dataset_revision(self):
        transaction = Mock()
        transaction.query_one.return_value = {"created_by": "owner@example.test", "status": "ready"}
        dataset = {"revision": 7, "document": {"created_by": "owner@example.test"}}
        with patch.object(mutations.product_store, "read", side_effect=[dataset, None]), \
             patch.object(mutations.product_outbox, "enqueue", return_value="event") as enqueue:
            self.assertEqual(mutations.publish_ready(transaction, "42", "actor@example.test"), "event")
        self.assertEqual(enqueue.call_args.kwargs, {
            "family": "dlm_definitions", "operation": "create", "record_id": "42",
            "payload": {"dataset_id": "42", "dataset_revision": 7},
            "actor": "actor@example.test", "owner": "owner@example.test",
        })

    def test_publication_fails_closed_before_outbox_on_source_or_owner_drift(self):
        transaction = Mock()
        cases = (
            ({"created_by": "owner", "status": "building"}, None, "before"),
            ({"created_by": "owner", "status": "ready"}, None, "missing"),
            ({"created_by": "owner", "status": "ready"}, {"revision": 1, "document": {"created_by": "other"}}, "ownership"),
        )
        for source, dataset, message in cases:
            with self.subTest(message=message):
                transaction.query_one.return_value = source
                with patch.object(mutations.product_store, "read", return_value=dataset), \
                     patch.object(mutations.product_outbox, "enqueue") as enqueue, \
                     self.assertRaisesRegex(RuntimeError, message):
                    mutations.publish_ready(transaction, "42", "actor")
                enqueue.assert_not_called()

    def test_existing_definition_emits_revision_checked_update_event(self):
        transaction = Mock()
        transaction.query_one.return_value = {"created_by": "owner", "status": "ready"}
        dataset = {"revision": 8, "document": {"created_by": "owner"}}
        with patch.object(mutations.product_store, "read", side_effect=[dataset, {"revision": 2}]), \
             patch.object(mutations.product_outbox, "enqueue") as enqueue:
            mutations.publish_ready(transaction, "42", "actor")
        self.assertEqual(enqueue.call_args.kwargs["operation"], "update")
        self.assertEqual(enqueue.call_args.kwargs["payload"]["dataset_revision"], 8)


class DlmDefinitionShadowTests(unittest.TestCase):
    def test_default_off_and_requester_scoped_exact_match(self):
        with patch.dict(os.environ, {}, clear=True), patch.object(shadow.product_store, "read") as read:
            self.assertEqual(shadow.observe_dlm_definition("42", "owner", "viewer", "Viewer")["status"], "disabled")
        read.assert_not_called()
        dataset = {"revision": 8, "document": {"created_by": "owner"}}
        definition = {"generation": 3, "document": {"dataset_id": "42", "dataset_revision": 8}}
        with patch.dict(os.environ, {"KAVEON_DLM_DEFINITION_SHADOW_READ_ENABLED": "true"}, clear=True), \
             patch.object(shadow.product_store, "read", side_effect=[dataset, definition]) as read:
            report = shadow.observe_dlm_definition("42", "owner", "viewer", "Viewer")
        self.assertEqual(report["status"], "match")
        self.assertEqual([call.args[2:] for call in read.call_args_list], [("viewer", "Viewer"), ("viewer", "Viewer")])

    def test_shadow_reports_owner_and_revision_mismatch_without_content(self):
        environment = {"KAVEON_DLM_DEFINITION_SHADOW_READ_ENABLED": "true"}
        with patch.dict(os.environ, environment, clear=True), patch.object(
            shadow.product_store, "read", return_value={"revision": 2, "document": {"created_by": "other"}}
        ):
            self.assertEqual(shadow.observe_dlm_definition("42", "owner", "viewer", "Viewer")["status"], "owner_mismatch")
        dataset = {"revision": 2, "document": {"created_by": "owner"}}
        definition = {"document": {"dataset_id": "42", "dataset_revision": 1}}
        with patch.dict(os.environ, environment, clear=True), patch.object(shadow.product_store, "read", side_effect=[dataset, definition]):
            report = shadow.observe_dlm_definition("42", "owner", "viewer", "Viewer")
        self.assertEqual(report["status"], "mismatch")
        self.assertNotIn("document", report)


if __name__ == "__main__":
    unittest.main()
