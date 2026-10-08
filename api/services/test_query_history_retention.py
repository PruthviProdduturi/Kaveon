"""Retention must not cost a listing on every write.

Appending one history row used to list the owner's entire history first — up
to a thousand records, paged a hundred at a time — purely to find the oldest
one to trim. Every statement waited on that, so a cube breakdown the Engine
answered in ~200ms took seconds to return.
"""
import sys
import unittest
from types import SimpleNamespace
from unittest.mock import patch

if "pyodbc" not in sys.modules:
    sys.modules["pyodbc"] = SimpleNamespace(Error=Exception)

import services.query_history as history


class RetentionListingTests(unittest.TestCase):
    def setUp(self):
        history._HISTORY_COUNTS.clear()
        self.addCleanup(history._HISTORY_COUNTS.clear)

    def _write(self, owner="owner@example.test"):
        payload = {"sql_text": "SELECT 1", "status": "success", "database_name": "OpenSource"}
        with patch("services.product_read_authority.enabled", return_value=True), \
             patch.object(history.product_store, "list_records", return_value=[]) as listed, \
             patch.object(history.product_store, "transact") as transact:
            history.create_history(payload, owner)
        return listed, transact

    def test_the_first_write_lists_once_and_the_next_does_not(self):
        listed, _ = self._write()
        self.assertEqual(listed.call_count, 1, "an unknown owner is counted once")
        listed, transact = self._write()
        self.assertEqual(listed.call_count, 0, "a counted owner does not list again")
        transact.assert_called_once()

    def test_an_owner_near_the_bound_lists_again(self):
        history._HISTORY_COUNTS["owner@example.test"] = history.MAX_HISTORY_PER_OWNER - 1
        listed, _ = self._write()
        self.assertEqual(listed.call_count, 1)

    def test_an_owner_at_the_bound_still_trims(self):
        owner = "owner@example.test"
        full = [{"revision": 1, "document": {"id": str(i), "user_email": owner,
                                             "executed_at": f"2026-01-{i % 28 + 1:02d}"}}
                for i in range(history.MAX_HISTORY_PER_OWNER)]
        payload = {"sql_text": "SELECT 1", "status": "success"}
        with patch("services.product_read_authority.enabled", return_value=True), \
             patch.object(history.product_store, "list_records", return_value=full), \
             patch.object(history.product_store, "transact") as transact:
            history.create_history(payload, owner)
        mutations = transact.call_args.args[0]
        operations = [m.operation for m in mutations]
        self.assertEqual(operations[0], "create")
        self.assertEqual(set(operations[1:]), {"delete"})
        # A batch, not one record: the listing that found them is what costs,
        # so it is amortised over the writes that follow rather than repeated.
        self.assertEqual(len(mutations), 1 + history._RETENTION_TRIM_BATCH + 1)
        self.assertLessEqual(len(mutations), 100, "a transaction carries at most 100 mutations")
        # The owner is left below the bound, so the next write skips the listing.
        self.assertLess(history._HISTORY_COUNTS[owner], history.MAX_HISTORY_PER_OWNER)
        self.assertFalse(history._at_retention_bound(owner))

    def test_the_count_tracks_writes_so_the_bound_is_still_reached(self):
        owner = "owner@example.test"
        history._HISTORY_COUNTS[owner] = 10
        self._write(owner)
        self.assertEqual(history._HISTORY_COUNTS[owner], 11)


if __name__ == "__main__":
    unittest.main()
