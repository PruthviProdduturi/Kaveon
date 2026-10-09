"""Seeded content must stay editable, and nobody else's content must become so.

KaveonDB requires a product record's owner to be the principal writing it, and
grants no Admin exception on writes the way it does on reads. The API's own
rule (middleware.permissions.can_write) is that an Editor or an Admin may edit
anyone's content. Nothing reconciled the two, so saving one of the seeded
dashboards was authorized by the API and then refused by the Engine with
"product record owner does not match the authenticated principal" — and since
the `system` sentinel is an identity nobody holds, that content could never be
edited by anyone.
"""
import sys
import unittest
from types import SimpleNamespace
from unittest.mock import patch

if "pyodbc" not in sys.modules:
    sys.modules["pyodbc"] = SimpleNamespace(Error=Exception)

from services import product_store


class SeededWriterTests(unittest.TestCase):
    def test_an_admin_writes_seeded_content_as_the_sentinel(self):
        self.assertEqual(
            product_store.writer(product_store.SEEDED_OWNER, "pruthvi@example.test", "Admin"),
            product_store.SEEDED_OWNER)

    def test_an_editor_writes_seeded_content_as_the_sentinel(self):
        self.assertEqual(
            product_store.writer(product_store.SEEDED_OWNER, "editor@example.test", "Editor"),
            product_store.SEEDED_OWNER)

    def test_seeded_content_stays_seeded_rather_than_transferring(self):
        """The first Admin to edit a seeded dashboard must not come to own it,
        or the next Admin would be locked out exactly as before."""
        first = product_store.writer(product_store.SEEDED_OWNER, "one@example.test", "Admin")
        second = product_store.writer(product_store.SEEDED_OWNER, "two@example.test", "Admin")
        self.assertEqual(first, second)
        self.assertEqual(first, product_store.SEEDED_OWNER)

    def test_an_analyst_is_not_a_steward_of_seeded_content(self):
        """can_write already refuses an Analyst someone else's content, so the
        attribution must not quietly grant what authorization withheld."""
        self.assertEqual(
            product_store.writer(product_store.SEEDED_OWNER, "analyst@example.test", "Analyst"),
            "analyst@example.test")
        self.assertEqual(
            product_store.writer(product_store.SEEDED_OWNER, "viewer@example.test", "Viewer"),
            "viewer@example.test")

    def test_another_persons_content_is_never_written_as_them(self):
        """This is the line that keeps the rule from becoming a general
        impersonation path: only the ownerless sentinel is stewarded, so the
        Engine goes on enforcing that one user cannot write another's records.
        """
        for role in ("Admin", "Editor", "Analyst", "Viewer"):
            with self.subTest(role=role):
                self.assertEqual(
                    product_store.writer("alice@example.test", "mallory@example.test", role),
                    "mallory@example.test")

    def test_a_caller_writing_their_own_content_writes_as_themselves(self):
        self.assertEqual(
            product_store.writer("alice@example.test", "alice@example.test", "Analyst"),
            "alice@example.test")

    def test_an_unknown_owner_falls_back_to_the_caller(self):
        for owner in (None, "", "System", "SYSTEM"):
            with self.subTest(owner=owner):
                self.assertEqual(
                    product_store.writer(owner, "pruthvi@example.test", "Admin"),
                    "pruthvi@example.test")


class SeededDashboardWriteTests(unittest.TestCase):
    """The rule reaching the actual write, not just the helper."""

    SEEDED = {"id": "d1", "name": "Kaveon Events", "created_by": "system",
              "visibility": "published"}

    def _update(self, document, actor, role):
        from services import dashboards
        with patch("services.product_read_authority.enabled", return_value=True), \
             patch.object(dashboards.product_store, "read",
                          return_value={"revision": 4, "document": document}), \
             patch.object(dashboards.product_store, "transact") as transact, \
             patch.object(dashboards, "get_dashboard_by_id", return_value=document):
            dashboards.update_dashboard("d1", {"name": "Renamed"}, actor, role)
        return transact.call_args.args[1]

    def test_an_admin_saving_a_seeded_dashboard_writes_as_the_sentinel(self):
        self.assertEqual(self._update(self.SEEDED, "pruthvi@example.test", "Admin"), "system")

    def test_an_admin_saving_their_own_dashboard_writes_as_themselves(self):
        owned = {**self.SEEDED, "created_by": "pruthvi@example.test"}
        self.assertEqual(
            self._update(owned, "pruthvi@example.test", "Admin"), "pruthvi@example.test")


if __name__ == "__main__":
    unittest.main()
