"""Clearing a catalog source's credential must work on the path that runs.

"Host identity" is the option that removes a credential altogether, so the
editor sends ``credential_kind: null`` for it. The KaveonDB branch of the PATCH
read that null as an invalid value and answered 400, while the PostgreSQL
branch had always accepted it -- and since every family reads from KaveonDB,
the one path that refused the clear was the only path that runs.
"""

import sys
import unittest
from types import SimpleNamespace
from unittest.mock import patch

if "pyodbc" not in sys.modules:
    sys.modules["pyodbc"] = SimpleNamespace(Error=Exception)

from fastapi import HTTPException

from routers import catalog_sources

STORED = {"id": "catalog-7", "source_kind": "catalog", "name": "lake",
          "catalog_identity": "lake", "source_type": "adls_gen2",
          "data_format": "parquet", "credential_kind": "secret_store",
          "credential_ref": "kv://kaveon-kv/lake", "adapter_type": "native",
          "storage_config": '{"account":"a","container":"c","root_path":""}',
          "lifecycle": "active"}


class TheCutoverPathAcceptsAClear(unittest.TestCase):
    def _patch(self, body):
        ctx = SimpleNamespace(email="ada@example.com", role="Admin")
        with patch("services.source_cutover_mutations.enabled", return_value=True), \
             patch("services.product_read_authority.read_document", return_value=dict(STORED)), \
             patch("services.source_cutover_mutations.update",
                   side_effect=lambda rid, changes, actor, **kw: {**STORED, **changes}) as update, \
             patch("routers.catalog_sources._validate_storage_config"), \
             patch("routers.catalog_sources._catalog_cutover_row", side_effect=lambda d: dict(d)):
            answer = catalog_sources.update_catalog_source("7", body, ctx)
        return answer, update

    def test_an_explicit_null_credential_kind_clears_it(self):
        answer, update = self._patch({"credential_kind": None})
        self.assertIsNone(update.call_args.args[1]["credential_kind"])
        self.assertIsNone(answer["catalogSource"]["credential_kind"])

    def test_an_empty_string_clears_rather_than_storing_empty(self):
        """Whichever spelling the client sends, the record holds null -- an
        empty string would be a credential kind that matches nothing."""
        _, update = self._patch({"credential_kind": ""})
        self.assertIsNone(update.call_args.args[1]["credential_kind"])

    def test_the_credential_reference_clears_too(self):
        _, update = self._patch({"credential_ref": None})
        self.assertIsNone(update.call_args.args[1]["credential_ref"])

    def test_a_real_credential_kind_still_applies(self):
        _, update = self._patch({"credential_kind": "managed_identity"})
        self.assertEqual(update.call_args.args[1]["credential_kind"], "managed_identity")

    def test_an_unknown_credential_kind_is_still_refused(self):
        with self.assertRaises(HTTPException) as raised:
            self._patch({"credential_kind": "sticky_note"})
        self.assertEqual(raised.exception.status_code, 400)

    def test_a_storage_type_may_not_be_cleared(self):
        """A source always has a storage type, so a null there is a mistake,
        not an instruction -- which is the asymmetry the legacy branch
        encodes and this one now matches."""
        for field in ("storage_type", "data_format", "adapter_type"):
            with self.subTest(field=field):
                with self.assertRaises(HTTPException) as raised:
                    self._patch({field: None})
                self.assertEqual(raised.exception.status_code, 400)

    def test_a_cleared_description_is_null(self):
        _, update = self._patch({"description": "  "})
        self.assertIsNone(update.call_args.args[1]["description"])

    def test_clearing_a_credential_does_not_validate_a_reference(self):
        """A managed identity has no secret to point at, and a cleared
        credential has nothing at all; reaching the secret store here is what
        used to fail a legal edit."""
        with patch("services.source_secret_store.validate_reference") as validate:
            self._patch({"credential_kind": None, "credential_ref": None})
        validate.assert_not_called()


if __name__ == "__main__":
    unittest.main()
