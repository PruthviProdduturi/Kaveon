"""A registered data source resolves from KaveonDB, not the metadata database.

`_resolve_data_source` read the `data_sources` table out of the metadata
database pool and returned None the moment that pool was absent, so after
PostgreSQL retirement no external source could be connected to at all -- which
is why every querying endpoint was fenced with a 503. The registry itself had
already moved: `/lab/databases` and `/api/v1/data-sources` list sources from
KaveonDB. Only the resolver was still looking in the old place.

These tests pin the resolution, including the part that must fail closed: a
source whose secret reference is missing or unrecognised must not quietly fall
back to an endpoint from the environment, because that connects somewhere the
source does not name.
"""

import sys
import unittest
from types import SimpleNamespace
from unittest.mock import patch

if "pyodbc" not in sys.modules:
    sys.modules["pyodbc"] = SimpleNamespace(Error=Exception)

import database.pool as pool
from services import credentials


ACTOR, ROLE = "analyst@example.com", "Analyst"
VAULT = "https://kaveon-kv.vault.azure.net"


class FakeStore:
    """Stands in for the vault. The real class builds a DefaultAzureCredential
    in __init__, which a unit test has no business doing."""

    def __init__(self, secret, seen=None):
        self.secret, self.seen = secret, seen

    def __call__(self, *args, **kwargs):
        return self

    def get(self, reference):
        if self.seen is not None:
            self.seen["reference"] = reference
        return self.secret


def document(**over):
    base = {
        "source_kind": "data", "source_id": "data-7", "name": "Warehouse",
        "source_type": "postgresql", "database_name": "warehouse",
        "is_active": True, "secret_ref": f"{VAULT}/secrets/kaveon-source-data-abc",
    }
    base.update(over)
    return base


class KaveonDbSourceResolutionTests(unittest.TestCase):
    def _resolve(self, documents, database="warehouse", secret="postgresql://u:p@h/db"):
        """Resolve through the real credential path, with only the vault faked."""
        with patch.object(pool, "_live_meta_db", return_value="__absent__"),              patch("services.product_read_authority.enabled", return_value=True),              patch("services.product_read_authority.list_documents", return_value=documents),              patch.dict("os.environ", {"KAVEON_SOURCE_SECRET_READ_ENABLED": "true",
                                       "KAVEON_KEY_VAULT_URL": VAULT}),              patch("services.source_secret_store.SourceSecretStore", FakeStore(secret)):
            return pool._resolve_data_source(database, ACTOR, ROLE)

    def test_a_source_registered_with_a_vault_reference_resolves_its_secret(self):
        row = self._resolve([document()])
        self.assertIsNotNone(row, "the source must resolve without a metadata database")
        self.assertEqual(row["type"], "postgresql")
        self.assertEqual(row["database_name"], "warehouse")
        self.assertEqual(row["connection_string"], "postgresql://u:p@h/db")

    def test_a_backfilled_marker_derives_the_name_it_was_written_with(self):
        """`source_backfill` wrote `key-managed:data_sources/<id>`, which records
        that a secret is managed without naming it. The name is derivable."""
        from services import source_secret_store
        expected = source_secret_store.secret_name("data", "7")
        seen = {}
        with patch.object(pool, "_live_meta_db", return_value="__absent__"),              patch("services.product_read_authority.enabled", return_value=True),              patch("services.product_read_authority.list_documents",
                   return_value=[document(secret_ref="key-managed:data_sources/7")]),              patch.dict("os.environ", {"KAVEON_SOURCE_SECRET_READ_ENABLED": "true",
                                       "KAVEON_KEY_VAULT_URL": VAULT}),              patch("services.source_secret_store.SourceSecretStore", FakeStore("postgresql://u:p@h/db", seen)):
            row = pool._resolve_data_source("warehouse", ACTOR, ROLE)
        self.assertEqual(row["connection_string"], "postgresql://u:p@h/db")
        self.assertEqual(seen["reference"], f"{VAULT}/secrets/{expected}")

    def test_a_source_is_found_by_its_id_as_well_as_its_label(self):
        """Callers pass either, the same two ways the legacy lookup accepted."""
        self.assertIsNotNone(self._resolve([document()], database="7"))

    def test_an_inactive_or_catalog_source_is_not_a_data_source(self):
        self.assertIsNone(self._resolve([document(is_active=False)]))
        self.assertIsNone(self._resolve([document(source_kind="catalog")]))

    def test_an_unknown_name_resolves_to_nothing(self):
        self.assertIsNone(self._resolve([document()], database="not-registered"))

    def test_a_source_with_no_usable_reference_fails_closed(self):
        """It must not fall through to an endpoint from the environment: that
        would connect somewhere the source does not name."""
        for reference in ("", "legacy-ciphertext", "kaveon:fernet:v1:x", None):
            with self.subTest(reference=reference):
                self.assertIsNone(self._resolve([document(secret_ref=reference)]))

    def test_the_marker_payload_is_not_consulted(self):
        """`key-managed:data_sources/<id>` names nothing the resolver needs: the
        secret's name is a pure function of the source's kind and id, which is
        how the backfill wrote it. So the payload is decoration, and a marker
        without one still resolves rather than failing closed on a difference
        that cannot matter."""
        full = self._resolve([document(secret_ref="key-managed:data_sources/7")])
        bare = self._resolve([document(secret_ref="key-managed:")])
        self.assertEqual(full, bare)
        self.assertIsNotNone(full)

    def test_the_secret_never_appears_in_the_document_shape(self):
        """The row handed on carries the plaintext; the document it came from
        must not, or the registry listing would leak it."""
        source = document()
        self._resolve([source])
        self.assertNotIn("postgresql://", str(source))
        self.assertTrue(str(source["secret_ref"]).startswith("https://"))

    def test_a_disabled_vault_read_is_an_error_rather_than_a_silent_miss(self):
        """A deployment that has not enabled source-secret reads should hear
        about it, not see every source quietly vanish."""
        with patch.object(pool, "_live_meta_db", return_value="__absent__"),              patch("services.product_read_authority.enabled", return_value=True),              patch("services.product_read_authority.list_documents", return_value=[document()]),              patch.dict("os.environ", {"KAVEON_KEY_VAULT_URL": VAULT}, clear=False):
            import os
            os.environ.pop("KAVEON_SOURCE_SECRET_READ_ENABLED", None)
            with self.assertRaises(credentials.CredentialError):
                pool._resolve_data_source("warehouse", ACTOR, ROLE)

    def test_without_an_actor_the_kaveondb_path_is_not_attempted(self):
        """The legacy signature still works: no actor means no product read."""
        with patch.object(pool, "_live_meta_db", return_value="__absent__"),              patch("services.product_read_authority.list_documents") as listed:
            self.assertIsNone(pool._resolve_data_source("warehouse"))
        listed.assert_not_called()


if __name__ == "__main__":
    unittest.main()
