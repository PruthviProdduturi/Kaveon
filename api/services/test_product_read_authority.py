import os
import unittest
from unittest.mock import patch

from services import charts, dashboards, datasets, product_read_authority as authority


class ProductReadAuthorityTests(unittest.TestCase):
    def test_default_keeps_postgresql_authority(self):
        with patch.dict(os.environ, {}, clear=True):
            self.assertFalse(authority.enabled("datasets"))

    def test_configuration_is_family_scoped_and_unknown_values_fail_closed(self):
        with patch.dict(os.environ, {authority.ENVIRONMENT_KEY: "datasets,charts"}, clear=True):
            self.assertTrue(authority.enabled("datasets"))
            self.assertTrue(authority.enabled("charts"))
            self.assertFalse(authority.enabled("dashboards"))
        with patch.dict(os.environ, {authority.ENVIRONMENT_KEY: "datasets,typo"}, clear=True):
            with self.assertRaisesRegex(authority.ProductReadAuthorityError, "unknown"):
                authority.enabled("datasets")

    def test_target_error_has_no_postgresql_fallback(self):
        with patch.dict(os.environ, {authority.ENVIRONMENT_KEY: "datasets"}, clear=True), \
             patch.object(authority.product_store, "read", side_effect=RuntimeError("target unavailable")), \
             patch.object(datasets.db, "query_one") as postgres:
            with self.assertRaisesRegex(RuntimeError, "target unavailable"):
                datasets.get_dataset_by_id("7", "alice@example.com", "Admin")
            postgres.assert_not_called()

    def test_cutover_point_reads_never_touch_postgresql(self):
        cases = (
            ("datasets", datasets.get_dataset_by_id, datasets.db, "7"),
            ("charts", charts.get_chart_by_id, charts.db, "chart-7"),
            ("dashboards", dashboards.get_dashboard_by_id, dashboards.db, "dash-7"),
        )
        for family, operation, database, record_id in cases:
            target = {"document": {
                "id": record_id, "name": "Visible", "visibility": "published",
                "created_by": "owner@example.com",
            }}
            with self.subTest(family=family), \
                 patch.dict(os.environ, {authority.ENVIRONMENT_KEY: family}, clear=True), \
                 patch.object(authority.product_store, "read", return_value=target), \
                 patch.object(database, "query_one") as postgres:
                result = operation(record_id, "alice@example.com", "Viewer")
                self.assertEqual(result["name"], "Visible")
                self.assertFalse(result["favorite"])
                postgres.assert_not_called()

    def test_visibility_is_enforced_after_privileged_bridge_read(self):
        target = {"document": {
            "id": "7", "visibility": "private", "created_by": "owner@example.com",
        }}
        with patch.dict(os.environ, {authority.ENVIRONMENT_KEY: "datasets"}, clear=True), \
             patch.object(authority.product_store, "read", return_value=target):
            self.assertIsNone(authority.read_document(
                "datasets", "7", "someone@example.com", "Viewer",
            ))
            self.assertEqual(authority.read_document(
                "datasets", "7", "owner@example.com", "Viewer",
            )["id"], "7")

    def test_invalid_target_document_fails_closed(self):
        with patch.dict(os.environ, {authority.ENVIRONMENT_KEY: "dashboards"}, clear=True), \
             patch.object(authority.product_store, "read", return_value={"document": "bad"}):
            with self.assertRaisesRegex(authority.ProductReadAuthorityError, "invalid"):
                authority.read_document("dashboards", "7", "owner@example.com", "Admin")


if __name__ == "__main__":
    unittest.main()
