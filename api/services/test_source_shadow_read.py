import os
import unittest
from unittest.mock import patch

from services import product_shadow_read as shadow
from services import source_mutations


def data_source(identifier=7, owner="owner@example.test"):
    return {
        "id": identifier, "name": "Warehouse", "type": "PostgreSQL",
        "database_name": "warehouse", "region": "WW", "description": None,
        "created_by": owner, "is_active": True,
    }


def catalog_source(identifier="lake", owner="owner@example.test"):
    return {
        "id": identifier, "name": "Lake", "engine_catalog": "lake",
        "storage_type": "adls_gen2", "credential_kind": "managed_identity",
        "credential_ref": None, "lifecycle": "active", "description": None,
        "created_by": owner,
    }


class SourceShadowReadTests(unittest.TestCase):
    def test_default_off_does_not_read_target(self):
        with patch.dict(os.environ, {}, clear=True), patch.object(shadow.product_store, "read") as read:
            report = shadow.observe_source(data_source(), "data", "viewer@example.test", "Viewer")
        self.assertEqual(report["status"], "disabled")
        read.assert_not_called()

    def test_data_source_uses_public_document_and_requester_authority(self):
        source = data_source()
        expected = source_mutations.data_document(source)
        with patch.dict(os.environ, {"KAVEON_SOURCE_SHADOW_READ_ENABLED": "true"}), \
             patch.object(shadow.product_store, "read", return_value={"document": expected, "generation": 3}) as read:
            report = shadow.observe_source(source, "data", "viewer@example.test", "Viewer")
        self.assertEqual(report["status"], "match")
        self.assertNotIn("record_id", report)
        self.assertNotIn("connection_string", str(expected))
        read.assert_called_once_with("source", "data-7", "viewer@example.test", "Viewer")

    def test_catalog_source_matches_canonical_outbox_document(self):
        source = catalog_source()
        expected = source_mutations.catalog_document(source)
        with patch.dict(os.environ, {"KAVEON_SOURCE_SHADOW_READ_ENABLED": "true"}), \
             patch.object(shadow.product_store, "read", return_value={"document": expected, "generation": 2}) as read:
            report = shadow.observe_source(source, "catalog", "admin@example.test", "Admin")
        self.assertEqual(report["status"], "match")
        read.assert_called_once_with("source", "catalog-lake", "admin@example.test", "Admin")

    def test_list_is_bounded_before_any_target_read(self):
        sources = [data_source(identifier=index) for index in range(shadow.MAX_SHADOW_LIST_RECORDS + 1)]
        with patch.dict(os.environ, {"KAVEON_SOURCE_SHADOW_READ_ENABLED": "true"}), \
             patch.object(shadow.product_store, "read") as read:
            report = shadow.observe_source_list(sources, "data", "viewer@example.test", "Viewer")
        self.assertEqual(report["status"], "skipped_limit")
        self.assertEqual(report["limit"], shadow.MAX_SHADOW_LIST_RECORDS)
        read.assert_not_called()

    def test_missing_and_mismatch_are_content_free(self):
        source = data_source()
        with patch.dict(os.environ, {"KAVEON_SOURCE_SHADOW_READ_ENABLED": "true"}):
            with patch.object(shadow.product_store, "read", return_value=None):
                missing = shadow.observe_source(source, "data", "viewer@example.test", "Viewer")
            with patch.object(shadow.product_store, "read", return_value={"document": {"source_id": "wrong"}}):
                mismatch = shadow.observe_source(source, "data", "viewer@example.test", "Viewer")
        self.assertEqual(missing["status"], "missing")
        self.assertEqual(mismatch["status"], "mismatch")
        self.assertNotIn("document", missing)
        self.assertNotIn("document", mismatch)

    def test_empty_actor_and_invalid_target_fail_closed(self):
        source = data_source()
        with patch.dict(os.environ, {"KAVEON_SOURCE_SHADOW_READ_ENABLED": "true"}):
            with self.assertRaisesRegex(RuntimeError, "actor identity"):
                shadow.observe_source(source, "data", "", "Viewer")
            with patch.object(shadow.product_store, "read", return_value={"document": []}):
                with self.assertRaisesRegex(RuntimeError, "response is invalid"):
                    shadow.observe_source(source, "data", "viewer@example.test", "Viewer")


if __name__ == "__main__":
    unittest.main()
