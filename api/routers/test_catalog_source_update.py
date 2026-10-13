"""Updating a catalog source must accept the shape it can be created in.

The create path exempts a managed or workload identity from secret-reference
validation, because the platform supplies the credential and the reference is
a label rather than a pointer to a secret. The update path did not, so a
workload-identity catalog could be created and then not corrected: a patch
carrying its own reference back failed with "source secret reference is
invalid" for a reference it never needed.
"""

import sys
import unittest
from types import SimpleNamespace
from unittest.mock import patch

try:
    import pyodbc  # noqa: F401
except ModuleNotFoundError:
    sys.modules["pyodbc"] = SimpleNamespace(Connection=object, connect=lambda *a, **k: None)

from middleware.auth import UserContext
from routers import catalog_sources as cs

ADMIN = UserContext(email="admin@example.test", role="Admin")
STORED = {"source_type": "adls_gen2",
          "storage_config": '{"account":"lake","container":"c","root_path":"r"}',
          "credential_kind": "workload_identity", "credential_ref": "a-reader",
          "lifecycle": "active"}


def _patch(body, stored=None):
    with patch.object(cs.source_cutover_mutations, "enabled", return_value=True), \
         patch.object(cs.product_read_authority, "read_document",
                      return_value=dict(stored or STORED)), \
         patch.object(cs.source_cutover_mutations, "update",
                      side_effect=lambda key, changes, actor, **kw: {**(stored or STORED), **changes}) as update, \
         patch.object(cs, "_catalog_cutover_row", side_effect=lambda row: row), \
         patch.object(cs, "_validate_storage_config"), \
         patch.object(cs.source_secret_store, "validate_reference",
                      side_effect=cs.source_secret_store.SourceSecretError(
                          "source secret reference is invalid")) as validate:
        result = cs.update_catalog_source("abc", body, ctx=ADMIN)
    return result, update, validate


class APlatformSuppliedIdentityNeedsNoSecret(unittest.TestCase):
    def test_a_workload_identity_reference_is_not_validated_as_a_secret(self):
        result, update, validate = _patch({"credential_ref": "a-reader"})
        validate.assert_not_called()
        self.assertEqual(update.call_args.args[1]["credential_ref"], "a-reader")
        self.assertTrue(result["success"])

    def test_the_kind_being_set_in_the_same_patch_decides(self):
        # Switching to workload identity and setting its label at once must
        # not be judged by the kind that was stored before.
        _result, _update, validate = _patch(
            {"credential_kind": "workload_identity", "credential_ref": "a-reader"},
            stored={**STORED, "credential_kind": "secret_store"})
        validate.assert_not_called()

    def test_a_managed_identity_reference_is_not_validated_either(self):
        _result, _update, validate = _patch(
            {"credential_ref": "label"},
            stored={**STORED, "credential_kind": "managed_identity"})
        validate.assert_not_called()


class ASecretStoreReferenceIsStillChecked(unittest.TestCase):
    def test_a_key_vault_reference_must_still_be_valid(self):
        from fastapi import HTTPException
        with self.assertRaises(HTTPException) as raised:
            _patch({"credential_ref": "not-a-uri"},
                   stored={**STORED, "credential_kind": "secret_store"})
        self.assertEqual(raised.exception.status_code, 400)

    def test_switching_to_a_secret_store_in_the_patch_is_checked(self):
        from fastapi import HTTPException
        with self.assertRaises(HTTPException):
            _patch({"credential_kind": "secret_store", "credential_ref": "not-a-uri"})


if __name__ == "__main__":
    unittest.main()
