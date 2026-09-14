import os
import unittest
from types import SimpleNamespace
from unittest.mock import patch

from routers import catalog_sources
from services import product_read_authority as authority, source_mutations


class CatalogSourceReadAuthorityTests(unittest.TestCase):
    def document(self):
        return source_mutations.catalog_document({
            "id": "lake", "name": "Lake", "engine_catalog": "OpenSource",
            "storage_type": "adls_gen2", "storage_config": {"account": "a", "container": "c", "root_path": "r"},
            "data_format": "delta", "credential_kind": "secret_store",
            "credential_ref": "https://vault.vault.azure.net/secrets/lake",
            "adapter_type": "native", "adapter_config": {}, "lifecycle": "active",
            "description": "Demo", "created_by": "owner", "modified_by": "owner",
            "created_at": "2026-01-01", "modified_at": "2026-01-02",
        })

    def test_list_and_point_reads_never_use_postgresql(self):
        document = self.document(); ctx = SimpleNamespace(email="admin", role="Admin")
        environment = {authority.ENVIRONMENT_KEY: "sources", "KAVEON_KEY_VAULT_URL": "https://vault.vault.azure.net"}
        with patch.dict(os.environ, environment, clear=True), \
             patch.object(authority.product_store, "list_records",
                          side_effect=lambda kind, *args, **kwargs: [] if kind == "favorite" else [{"document": document}]), \
             patch.object(authority.product_store, "read", return_value={"document": document}), \
             patch.object(catalog_sources.db, "query") as query, patch.object(catalog_sources.db, "query_one") as query_one:
            listed = catalog_sources.list_catalog_sources(ctx)
            pointed = catalog_sources.get_catalog_source("lake", ctx)
            self.assertEqual(listed["catalogSources"][0]["engine_catalog"], "OpenSource")
            self.assertEqual(pointed["catalogSource"]["storage_type"], "adls_gen2")
            self.assertEqual(pointed["catalogSource"]["credential_ref"], document["credential_ref"])
            query.assert_not_called(); query_one.assert_not_called()

    def test_plaintext_secret_reference_is_rejected(self):
        row = {**self.document(), "credential_kind": "secret_store", "credential_ref": "plaintext"}
        with self.assertRaises(Exception): catalog_sources._catalog_cutover_row(row)


if __name__ == "__main__": unittest.main()
