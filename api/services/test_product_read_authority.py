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

    def test_all_explicitly_cuts_over_every_supported_product_family(self):
        with patch.dict(os.environ, {authority.ENVIRONMENT_KEY: "all"}, clear=True):
            for family in authority.SUPPORTED_FAMILIES:
                with self.subTest(family=family):
                    self.assertTrue(authority.enabled(family))

    def test_all_does_not_silently_enable_unimplemented_control_plane_families(self):
        with patch.dict(os.environ, {authority.ENVIRONMENT_KEY: "all"}, clear=True):
            with self.assertRaises(authority.ProductReadAuthorityError):
                authority.enabled("context_cache")

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
                 patch.object(authority.product_store, "read",
                              side_effect=lambda kind, *args, **kwargs: target if kind != "favorite" else None), \
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

    def test_cutover_lists_filter_visibility_order_and_never_read_postgresql(self):
        records = [
            {"document": {"id": "old", "name": "Old", "visibility": "published",
                          "created_by": "other@example.com", "updated_at": "2026-01-01T00:00:00Z"}},
            {"document": {"id": "private", "name": "Hidden", "visibility": "private",
                          "created_by": "other@example.com", "updated_at": "2026-03-01T00:00:00Z"}},
            {"document": {"id": "new", "name": "New", "visibility": "published",
                          "created_by": "other@example.com", "updated_at": "2026-02-01T00:00:00Z"}},
        ]
        cases = (("datasets", datasets.list_datasets, datasets.db),
                 ("charts", charts.list_charts, charts.db),
                 ("dashboards", dashboards.list_dashboards, dashboards.db))
        for family, operation, database in cases:
            with self.subTest(family=family), patch.dict(os.environ, {authority.ENVIRONMENT_KEY: family}, clear=True), \
                 patch.object(authority.product_store, "list_records", side_effect=lambda kind, *args, **kwargs: records if kind != "favorite" else []), \
                 patch.object(database, "query") as postgres:
                result = operation("viewer@example.com", "Viewer")
                self.assertEqual([item["id"] for item in result], ["new", "old"])
                self.assertTrue(all(item["favorite"] is False for item in result))
                postgres.assert_not_called()

    def test_list_target_failure_has_no_postgresql_fallback(self):
        with patch.dict(os.environ, {authority.ENVIRONMENT_KEY: "dashboards"}, clear=True), \
             patch.object(authority.product_store, "list_records", side_effect=RuntimeError("unavailable")), \
             patch.object(dashboards.db, "query") as postgres:
            with self.assertRaisesRegex(RuntimeError, "unavailable"):
                dashboards.list_dashboards("owner@example.com", "Admin")
            postgres.assert_not_called()

    def test_list_favorites_are_joined_from_kaveondb(self):
        records = [{"document": {"id": "7", "visibility": "published",
                                  "created_by": "owner", "updated_at": "2026-01-01"}}]
        favorites = [{"document": {"user_email": "alice", "object_type": "dataset", "object_id": "7"}}]
        with patch.dict(os.environ, {authority.ENVIRONMENT_KEY: "datasets"}, clear=True), \
             patch.object(authority.product_store, "list_records",
                          side_effect=lambda kind, *args, **kwargs: favorites if kind == "favorite" else records):
            self.assertTrue(authority.list_documents("datasets", "alice", "Viewer")[0]["favorite"])


if __name__ == "__main__":
    unittest.main()
