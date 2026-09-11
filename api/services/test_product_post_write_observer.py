import hashlib
import json
import os
import sys
import unittest
from types import SimpleNamespace
from unittest.mock import patch

if "pyodbc" not in sys.modules:
    sys.modules["pyodbc"] = SimpleNamespace(Error=Exception)

from services import product_outbox, product_post_write_observer as observer


def event(operation="update"):
    document = {"id": "7", "name": "Orders"}
    digest = hashlib.sha256(json.dumps(document, sort_keys=True, separators=(",", ":")).encode()).hexdigest()
    return product_outbox.OutboxEvent(
        "11111111-1111-1111-1111-111111111111", 9, "datasets", operation,
        "7", digest, "editor@example.test", "owner@example.test",
    ), document


def status_row(value, *, applied=True):
    return {
        "event_id": value.event_id, "source_sequence": value.source_sequence,
        "family": value.family, "operation": value.operation,
        "record_id": value.record_id, "payload_sha256": value.payload_sha256,
        "owner_principal": value.owner_principal,
        "applied_at": "now" if applied else None, "target_generation": 3,
        "apply_attempts": 1, "last_error_code": None,
    }


class PostWriteObserverTests(unittest.TestCase):
    def test_disabled_returns_before_source_or_target_read(self):
        value, _ = event()
        with patch.dict(os.environ, {}, clear=True), \
             patch.object(observer.product_outbox, "status") as source, \
             patch.object(observer.product_store, "read") as target:
            report = observer.observe_dataset(value)
        self.assertEqual(report["status"], "disabled")
        source.assert_not_called()
        target.assert_not_called()

    def test_pending_replay_is_not_reported_as_divergence(self):
        value, _ = event()
        row = status_row(value, applied=False)
        row["apply_attempts"] = 2
        row["last_error_code"] = "target_unavailable"
        with patch.dict(os.environ, {"KAVEON_DATASET_POST_WRITE_VERIFY_ENABLED": "true"}), \
             patch.object(observer.product_outbox, "status", return_value=row), \
             patch.object(observer.product_store, "read") as target:
            report = observer.observe_dataset(value)
        self.assertEqual(report["status"], "pending_replay")
        self.assertEqual(report["apply_attempts"], 2)
        target.assert_not_called()

    def test_applied_event_is_verified_as_stored_owner(self):
        value, document = event()
        with patch.dict(os.environ, {"KAVEON_DATASET_POST_WRITE_VERIFY_ENABLED": "true"}), \
             patch.object(observer.product_outbox, "status", return_value=status_row(value)), \
             patch.object(observer.product_store, "read", return_value={"document": document, "generation": 3}) as target:
            report = observer.observe_dataset(value)
        target.assert_called_once_with("dataset", "7", "owner@example.test", "Admin")
        self.assertEqual(report["status"], "verified")
        self.assertNotIn("owner_principal", report)

    def test_applied_content_difference_is_divergence(self):
        value, _ = event()
        with patch.dict(os.environ, {"KAVEON_DATASET_POST_WRITE_VERIFY_ENABLED": "true"}), \
             patch.object(observer.product_outbox, "status", return_value=status_row(value)), \
             patch.object(observer.product_store, "read", return_value={"document": {"id": "7", "name": "Other"}, "generation": 4}):
            report = observer.observe_dataset(value)
        self.assertEqual(report["status"], "target_divergence")

    def test_missing_or_changed_outbox_is_distinct_from_pending(self):
        value, _ = event()
        with patch.dict(os.environ, {"KAVEON_DATASET_POST_WRITE_VERIFY_ENABLED": "true"}):
            with patch.object(observer.product_outbox, "status", return_value=None):
                self.assertEqual(observer.observe_dataset(value)["status"], "outbox_missing")
            row = status_row(value)
            row["payload_sha256"] = "0" * 64
            with patch.object(observer.product_outbox, "status", return_value=row):
                self.assertEqual(observer.observe_dataset(value)["status"], "outbox_divergence")

    def test_delete_verifies_absence_and_flags_presence(self):
        value, _ = event("delete")
        with patch.dict(os.environ, {"KAVEON_DATASET_POST_WRITE_VERIFY_ENABLED": "true"}), \
             patch.object(observer.product_outbox, "status", return_value=status_row(value)):
            with patch.object(observer.product_store, "read", return_value=None):
                self.assertEqual(observer.observe_dataset(value)["status"], "verified")
            with patch.object(observer.product_store, "read", return_value={"document": {"id": "7"}, "generation": 2}):
                self.assertEqual(observer.observe_dataset(value)["status"], "target_divergence")


if __name__ == "__main__":
    unittest.main()
