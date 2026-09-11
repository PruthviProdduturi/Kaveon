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

    def test_list_comparison_is_owner_scoped_and_projection_only(self):
        sources = [
            {"id": "1", "name": "one", "favorite": True},
            {"id": "2", "name": "two", "favorite": False},
        ]
        targets = [
            {"document": {"id": "1", "name": "one", "columns": ["ignored"]}, "generation": 1},
            {"document": {"id": "2", "name": "changed"}, "generation": 1},
        ]
        with patch.dict(os.environ, {"KAVEON_DATASET_SHADOW_READ_ENABLED": "true"}), \
             patch.object(product_shadow_read.product_store, "read", side_effect=targets) as read:
            report = product_shadow_read.compare_dataset_list(sources, "owner@example.test", "Viewer")
        self.assertEqual(report["status"], "mismatch")
        self.assertEqual((report["match"], report["mismatch"], report["missing"]), (1, 1, 0))
        self.assertEqual(read.call_count, 2)
        self.assertTrue(all(call.args[2:] == ("owner@example.test", "Viewer") for call in read.call_args_list))

    def test_list_bound_skips_all_target_reads(self):
        sources = [{"id": str(index)} for index in range(product_shadow_read.MAX_SHADOW_LIST_RECORDS + 1)]
        with patch.dict(os.environ, {"KAVEON_DATASET_SHADOW_READ_ENABLED": "true"}), \
             patch.object(product_shadow_read.product_store, "read") as read:
            report = product_shadow_read.compare_dataset_list(sources, "owner", "Admin")
        self.assertEqual(report["status"], "skipped_limit")
        read.assert_not_called()

    def test_disabled_list_returns_before_validation_or_target_access(self):
        with patch.dict(os.environ, {}, clear=True), \
             patch.object(product_shadow_read.product_store, "read") as read:
            report = product_shadow_read.compare_dataset_list([{}], "", "bad-role")
        self.assertEqual(report["status"], "disabled")
        read.assert_not_called()

    def test_chart_comparison_is_owner_scoped_and_excludes_decorations(self):
        source = {"id": "c1", "name": "Revenue", "dataset_id": "7", "chart_type": "bar", "query_config": {}, "viz_config": {}, "visibility": "private", "favorite": True, "thumbnail": "large-sensitive-preview"}
        target_document = {field: source.get(field) for field in product_shadow_read.CHART_SHADOW_FIELDS}
        target_document["unrelated"] = "ignored"
        with patch.dict(os.environ, {"KAVEON_CHART_SHADOW_READ_ENABLED": "true"}), \
             patch.object(product_shadow_read.product_store, "read", return_value={"document": target_document, "generation": 5}) as read:
            report = product_shadow_read.compare_chart(source, "owner@example.test", "Viewer")
        read.assert_called_once_with("chart", "c1", "owner@example.test", "Viewer")
        self.assertEqual(report["status"], "match")
        self.assertNotIn("thumbnail", str(report))

    def test_chart_missing_mismatch_and_disabled_are_distinct(self):
        source = {"id": "c1", "name": "Revenue"}
        with patch.dict(os.environ, {}, clear=True), patch.object(product_shadow_read.product_store, "read") as read:
            self.assertEqual(product_shadow_read.compare_chart(source, "a", "Admin")["status"], "disabled")
            read.assert_not_called()
        with patch.dict(os.environ, {"KAVEON_CHART_SHADOW_READ_ENABLED": "true"}):
            with patch.object(product_shadow_read.product_store, "read", return_value=None):
                self.assertEqual(product_shadow_read.compare_chart(source, "a", "Admin")["status"], "missing")
            with patch.object(product_shadow_read.product_store, "read", return_value={"document": {"id": "c1", "name": "Other"}, "generation": 1}):
                self.assertEqual(product_shadow_read.compare_chart(source, "a", "Admin")["status"], "mismatch")

    def test_chart_bounds_and_invalid_target_fail_closed(self):
        source = {"id": "c1", "query_config": {"value": "x" * product_shadow_read.MAX_SHADOW_DOCUMENT_BYTES}}
        with patch.dict(os.environ, {"KAVEON_CHART_SHADOW_READ_ENABLED": "true"}):
            with self.assertRaisesRegex(RuntimeError, "byte bound"):
                product_shadow_read.compare_chart(source, "a", "Admin")
            with patch.object(product_shadow_read.product_store, "read", return_value={"document": []}):
                with self.assertRaisesRegex(RuntimeError, "response is invalid"):
                    product_shadow_read.compare_chart({"id": "c1"}, "a", "Admin")

    def test_dashboard_shadow_is_owner_scoped_canonical_and_default_off(self):
        source = {"id":"d1","name":"Ops","layout":"[]","charts":"[\"c1\"]","filters":"[]",
                  "visibility":"private","favorite":True,"thumbnail":"preview"}
        target = {field: source.get(field) for field in product_shadow_read.DASHBOARD_SHADOW_FIELDS}
        target.update({"layout":[],"charts":["c1"],"filters":[],"chart_revisions":{"c1":2}})
        with patch.dict(os.environ, {}, clear=True), patch.object(product_shadow_read.product_store,"read") as read:
            self.assertEqual(product_shadow_read.compare_dashboard(source,"owner","Admin")["status"],"disabled")
            read.assert_not_called()
        with patch.dict(os.environ,{"KAVEON_DASHBOARD_SHADOW_READ_ENABLED":"true"}),\
             patch.object(product_shadow_read.product_store,"read",return_value={"document":target,"generation":4}) as read:
            report=product_shadow_read.compare_dashboard(source,"owner","Viewer")
        self.assertEqual(report["status"],"match"); read.assert_called_once_with("dashboard","d1","owner","Viewer")
        self.assertNotIn("preview",str(report))

    def test_dashboard_shadow_distinguishes_missing_mismatch_and_invalid_source(self):
        with patch.dict(os.environ,{"KAVEON_DASHBOARD_SHADOW_READ_ENABLED":"true"}):
            with patch.object(product_shadow_read.product_store,"read",return_value=None):
                self.assertEqual(product_shadow_read.compare_dashboard({"id":"d1","layout":"[]","charts":"[]","filters":"[]"},"a","Admin")["status"],"missing")
            with patch.object(product_shadow_read.product_store,"read",return_value={"document":{"id":"d1"},"generation":1}):
                self.assertEqual(product_shadow_read.compare_dashboard({"id":"d1","layout":"[]","charts":"[]","filters":"[]"},"a","Admin")["status"],"mismatch")
            with self.assertRaisesRegex(RuntimeError,"layout is invalid"):
                product_shadow_read.compare_dashboard({"id":"d1","layout":"bad","charts":"[]","filters":"[]"},"a","Admin")


if __name__ == "__main__":
    unittest.main()
