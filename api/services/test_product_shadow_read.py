import os
import unittest
from unittest.mock import patch

from services import product_shadow_read


class ProductShadowReadTests(unittest.TestCase):
    def test_disabled_returns_before_target_access(self):
        with patch.dict(os.environ, {}, clear=True), \
             patch.object(product_shadow_read.product_store, "read") as read:
            report = product_shadow_read.compare_dataset({"id": "7"}, "owner@example.test", "Viewer")
        self.assertEqual(report, {"family": "datasets", "enabled": False, "status": "disabled"})
        read.assert_not_called()

    def test_exact_match_uses_requesting_actor_and_role(self):
        document = {"id": "7", "name": "Orders", "visibility": "private"}
        with patch.dict(os.environ, {"KAVEON_DATASET_SHADOW_READ_ENABLED": "true"}), \
             patch.object(product_shadow_read.product_store, "read", return_value={"document": document, "generation": 4}) as read:
            report = product_shadow_read.compare_dataset(document, "owner@example.test", "Viewer")
        read.assert_called_once_with("dataset", "7", "owner@example.test", "Viewer")
        self.assertEqual(report["status"], "match")
        self.assertEqual(report["source_sha256"], report["target_sha256"])
        self.assertNotIn("document", report)

    def test_missing_and_mismatch_are_telemetry_only(self):
        source = {"id": "7", "name": "source"}
        with patch.dict(os.environ, {"KAVEON_DATASET_SHADOW_READ_ENABLED": "true"}):
            with patch.object(product_shadow_read.product_store, "read", return_value=None):
                self.assertEqual(product_shadow_read.compare_dataset(source, "a", "Admin")["status"], "missing")
            with patch.object(product_shadow_read.product_store, "read", return_value={"document": {"id": "7", "name": "target"}, "generation": 2}):
                report = product_shadow_read.compare_dataset(source, "a", "Admin")
                self.assertEqual(report["status"], "mismatch")
                self.assertNotEqual(report["source_sha256"], report["target_sha256"])

    def test_bounds_and_response_shape_fail_closed(self):
        with patch.dict(os.environ, {"KAVEON_DATASET_SHADOW_READ_ENABLED": "true"}):
            oversized = {"id": "7", "value": "x" * product_shadow_read.MAX_SHADOW_DOCUMENT_BYTES}
            with self.assertRaisesRegex(RuntimeError, "byte bound"):
                product_shadow_read.compare_dataset(oversized, "a", "Admin")
            with patch.object(product_shadow_read.product_store, "read", return_value={"document": "invalid"}):
                with self.assertRaisesRegex(RuntimeError, "response is invalid"):
                    product_shadow_read.compare_dataset({"id": "7"}, "a", "Admin")

    def test_enabled_comparison_requires_actor_and_record(self):
        with patch.dict(os.environ, {"KAVEON_DATASET_SHADOW_READ_ENABLED": "true"}):
            for document, actor in (({}, "a"), ({"id": "7"}, "")):
                with self.assertRaisesRegex(RuntimeError, "record and actor identity"):
                    product_shadow_read.compare_dataset(document, actor, "Admin")


if __name__ == "__main__":
    unittest.main()
