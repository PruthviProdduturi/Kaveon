"""A dataset anyone has ever queried must still be deletable.

A query-history document carries `dataset_id`, and the Engine derives a
reference to the dataset from it, so the catalog refuses to delete a dataset
that any row still names. This is the reference nobody chooses: a favorite or a
recents entry is something a person did, while a history row appears simply
because someone once ran a query — so a dataset could be undeletable with no
visible reason at all.
"""

import sys
import unittest
from types import SimpleNamespace
from unittest.mock import patch

if "pyodbc" not in sys.modules:
    sys.modules["pyodbc"] = SimpleNamespace(Error=Exception)

from services import query_history as qh


def row(record_id, owner, dataset_id, revision=1):
    return {"id": record_id, "revision": revision,
            "document": {"id": record_id, "user_email": owner,
                         "dataset_id": dataset_id, "sql_text": "SELECT 1"}}


class ThePurgeClearsOnlyWhatNamesTheDataset(unittest.TestCase):
    def _run(self, records, dataset_id="d1", limit=None):
        with patch("services.product_read_authority.enabled", return_value=True), \
             patch("services.product_store.list_records", return_value=records), \
             patch("services.product_store.transact") as transact:
            kwargs = {} if limit is None else {}
            cleared = qh.remove_dataset_all_users(dataset_id, **kwargs)
        return cleared, transact

    def test_rows_naming_the_dataset_are_cleared(self):
        cleared, transact = self._run([row("h1", "a@b.c", "d1"), row("h2", "a@b.c", "d1")])
        self.assertEqual(cleared, 2)
        ids = [m.record_id for call in transact.call_args_list for m in call.args[0]]
        self.assertEqual(sorted(ids), ["h1", "h2"])

    def test_rows_for_another_dataset_are_left_alone(self):
        cleared, transact = self._run([row("h1", "a@b.c", "other")])
        self.assertEqual(cleared, 0)
        transact.assert_not_called()

    def test_a_row_with_no_dataset_is_left_alone(self):
        """A history row for a raw SQL Lab statement has no dataset, so the
        Engine derives no reference from it and it blocks nothing."""
        bare = row("h1", "a@b.c", "")
        bare["document"]["dataset_id"] = None
        cleared, transact = self._run([bare])
        self.assertEqual(cleared, 0)
        transact.assert_not_called()

    def test_each_owner_is_written_as_itself(self):
        """A history row belongs to whoever ran the query, so it cannot be
        removed in the deleter's transaction."""
        cleared, transact = self._run([row("h1", "a@b.c", "d1"),
                                       row("h2", "x@y.z", "d1"),
                                       row("h3", "x@y.z", "d1")])
        self.assertEqual(cleared, 3)
        actors = sorted(call.args[1] for call in transact.call_args_list)
        self.assertEqual(actors, ["a@b.c", "x@y.z"])
        for call in transact.call_args_list:
            owners = {m.record_id for m in call.args[0]}
            self.assertTrue(owners <= {"h1"} or owners <= {"h2", "h3"})

    def test_the_revision_guard_is_carried(self):
        cleared, transact = self._run([row("h1", "a@b.c", "d1", revision=7)])
        mutation = transact.call_args.args[0][0]
        self.assertEqual((mutation.operation, mutation.expected_revision), ("delete", 7))

    def test_the_fanout_is_bounded(self):
        many = [row(f"h{i}", f"u{i}@x.y", "d1")
                for i in range(qh.MAX_CROSS_OWNER_DELETE + 1)]
        with patch("services.product_read_authority.enabled", return_value=True), \
             patch("services.product_store.list_records", return_value=many), \
             patch("services.product_store.transact") as transact:
            with self.assertRaisesRegex(RuntimeError, "fanout bound"):
                qh.remove_dataset_all_users("d1")
        transact.assert_not_called()

    def test_an_invalid_record_is_refused_rather_than_skipped(self):
        """A malformed record means the catalog answered something unexpected,
        and guessing past it would leave a reference behind and make the delete
        fail later with a message about a different record."""
        with patch("services.product_read_authority.enabled", return_value=True), \
             patch("services.product_store.list_records",
                   return_value=[{"id": "h1", "revision": 0, "document": {}}]), \
             patch("services.product_store.transact") as transact:
            with self.assertRaises(RuntimeError):
                qh.remove_dataset_all_users("d1")
        transact.assert_not_called()

    def test_the_postgresql_path_does_nothing(self):
        with patch("services.product_read_authority.enabled", return_value=False), \
             patch("services.product_store.list_records") as listed:
            self.assertEqual(qh.remove_dataset_all_users("d1"), 0)
        listed.assert_not_called()


class TheRetentionCountIsInvalidated(unittest.TestCase):
    def test_a_purged_owner_s_remembered_count_is_dropped(self):
        """Rows went out from under the cache. A count left too high makes the
        retention sweep trim rows it did not need to, and `_at_retention_bound`
        reads an unknown count as one worth re-establishing."""
        qh._remember_history_count("a@b.c", 800)
        with patch("services.product_read_authority.enabled", return_value=True), \
             patch("services.product_store.list_records",
                   return_value=[row("h1", "a@b.c", "d1")]), \
             patch("services.product_store.transact"):
            qh.remove_dataset_all_users("d1")
        self.assertTrue(qh._at_retention_bound("a@b.c"))

    def test_an_untouched_owner_keeps_their_count(self):
        qh._remember_history_count("keep@b.c", 1)
        with patch("services.product_read_authority.enabled", return_value=True), \
             patch("services.product_store.list_records",
                   return_value=[row("h1", "a@b.c", "d1")]), \
             patch("services.product_store.transact"):
            qh.remove_dataset_all_users("d1")
        self.assertFalse(qh._at_retention_bound("keep@b.c"))


if __name__ == "__main__":
    unittest.main()
