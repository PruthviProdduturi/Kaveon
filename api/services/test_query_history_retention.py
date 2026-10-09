"""Retention must not cost a listing, or a trim, on the request path.

Appending one history row used to list the owner's entire history first — up
to a thousand records, paged a hundred at a time — and then commit the trim it
found as a hundred sequential statements inside the same transaction. Every
statement that hit the bound waited on all of it, so a cube breakdown the
Engine answered in ~200ms took seconds to return. The bound is a bound rather
than an invariant, so it is reasserted behind the response instead.
"""
import sys
from datetime import datetime, timedelta, timezone
import unittest
from fastapi import HTTPException
from types import SimpleNamespace
from unittest.mock import patch

if "pyodbc" not in sys.modules:
    sys.modules["pyodbc"] = SimpleNamespace(Error=Exception)

import services.query_history as history


def _records(owner, count, revision=1):
    return [{"revision": revision,
             "document": {"id": str(index), "user_email": owner,
                          "executed_at": f"2026-01-{index % 28 + 1:02d}"}}
            for index in range(count)]


class WritePathTests(unittest.TestCase):
    """What a query's own response waits for."""

    def setUp(self):
        history._HISTORY_COUNTS.clear()
        history._RETENTION_SWEEPS.clear()
        self.addCleanup(history._HISTORY_COUNTS.clear)
        self.addCleanup(history._RETENTION_SWEEPS.clear)

    def _write(self, owner="owner@example.test"):
        payload = {"sql_text": "SELECT 1", "status": "success", "database_name": "OpenSource"}
        with patch("services.product_read_authority.enabled", return_value=True), \
             patch.object(history.product_store, "list_records",
                          side_effect=AssertionError("the write path listed history")), \
             patch.object(history, "_schedule_retention") as scheduled, \
             patch.object(history.product_store, "transact") as transact:
            history.create_history(payload, owner)
        return scheduled, transact

    def test_a_write_commits_one_mutation_and_never_lists(self):
        scheduled, transact = self._write()
        transact.assert_called_once()
        self.assertEqual(len(transact.call_args.args[0]), 1)
        self.assertEqual(transact.call_args.args[0][0].operation, "create")
        scheduled.assert_called_once()

    def test_every_write_offers_the_sweep_the_chance_to_run(self):
        """The decision to sweep belongs to the scheduler, which knows the
        count and whether one is already in flight — not to the write."""
        for _ in range(3):
            scheduled, _ = self._write()
            scheduled.assert_called_once()

    def test_the_count_tracks_writes_so_the_bound_is_still_reached(self):
        owner = "owner@example.test"
        history._HISTORY_COUNTS[owner] = 10
        self._write(owner)
        self.assertEqual(history._HISTORY_COUNTS[owner], 11)

    def test_an_owner_no_sweep_has_counted_yet_stays_uncounted(self):
        """Only a sweep's listing establishes a count. Incrementing from an
        assumed zero would hide a full history behind a small number."""
        self._write("stranger@example.test")
        self.assertNotIn("stranger@example.test", history._HISTORY_COUNTS)
        self.assertTrue(history._at_retention_bound("stranger@example.test"))


class SchedulingTests(unittest.TestCase):
    def setUp(self):
        history._HISTORY_COUNTS.clear()
        history._RETENTION_SWEEPS.clear()
        self.addCleanup(history._HISTORY_COUNTS.clear)
        self.addCleanup(history._RETENTION_SWEEPS.clear)

    def test_an_owner_below_the_bound_schedules_nothing(self):
        history._HISTORY_COUNTS["owner@example.test"] = 100
        with patch.object(history.threading, "Thread") as thread:
            history._schedule_retention("owner@example.test")
        thread.assert_not_called()

    def test_an_owner_near_the_bound_schedules_a_sweep(self):
        history._HISTORY_COUNTS["owner@example.test"] = history.MAX_HISTORY_PER_OWNER - 1
        with patch.object(history.threading, "Thread") as thread:
            history._schedule_retention("owner@example.test")
        thread.assert_called_once()
        thread.return_value.start.assert_called_once()

    def test_a_sweep_already_in_flight_is_not_started_twice(self):
        """A dashboard writes a dozen rows at once. Each would see the same
        over-bound count, and two sweeps would race for the same revisions."""
        history._HISTORY_COUNTS["owner@example.test"] = history.MAX_HISTORY_PER_OWNER
        with patch.object(history.threading, "Thread") as thread:
            for _ in range(12):
                history._schedule_retention("owner@example.test")
        thread.assert_called_once()

    def test_a_finished_sweep_releases_its_claim(self):
        owner = "owner@example.test"
        self.assertTrue(history._claim_retention_sweep(owner))
        with patch.object(history, "run_retention_sweep", return_value=0):
            history._sweep_retention_quietly(owner)
        self.assertNotIn(owner, history._RETENTION_SWEEPS)

    def test_a_failed_sweep_releases_its_claim_and_raises_nothing(self):
        """The query that triggered the sweep has already been answered, so a
        failure here can only mean the owner stays over the bound until the
        next write schedules another one."""
        owner = "owner@example.test"
        self.assertTrue(history._claim_retention_sweep(owner))
        with patch.object(history, "run_retention_sweep",
                          side_effect=RuntimeError("KaveonDB unreachable")):
            history._sweep_retention_quietly(owner)
        self.assertNotIn(owner, history._RETENTION_SWEEPS)


class SweepTests(unittest.TestCase):
    def setUp(self):
        history._HISTORY_COUNTS.clear()
        self.addCleanup(history._HISTORY_COUNTS.clear)

    def test_a_sweep_below_the_bound_counts_and_deletes_nothing(self):
        owner = "owner@example.test"
        with patch.object(history.product_store, "list_records",
                          return_value=_records(owner, 400)), \
             patch.object(history.product_store, "transact") as transact:
            self.assertEqual(history.run_retention_sweep(owner), 0)
        transact.assert_not_called()
        self.assertEqual(history._HISTORY_COUNTS[owner], 400)

    def test_a_sweep_at_the_bound_trims_a_batch_and_clears_the_bound(self):
        owner = "owner@example.test"
        with patch.object(history.product_store, "list_records",
                          return_value=_records(owner, history.MAX_HISTORY_PER_OWNER)), \
             patch.object(history.product_store, "transact") as transact:
            trimmed = history.run_retention_sweep(owner)
        mutations = transact.call_args.args[0]
        self.assertEqual(trimmed, len(mutations))
        self.assertEqual({item.operation for item in mutations}, {"delete"})
        # A batch, not one record: the listing that found them is what costs,
        # so it is amortised over the writes that follow rather than repeated.
        self.assertEqual(len(mutations), history._RETENTION_TRIM_BATCH)
        self.assertLessEqual(len(mutations), 100,
                             "a transaction carries at most 100 mutations")
        # The owner is left below the bound, so the writes that follow
        # schedule nothing.
        self.assertLess(history._HISTORY_COUNTS[owner], history.MAX_HISTORY_PER_OWNER)
        self.assertFalse(history._at_retention_bound(owner))

    def test_a_sweep_trims_the_oldest_records(self):
        owner = "owner@example.test"
        start = datetime(2026, 1, 1, tzinfo=timezone.utc)
        records = [{"revision": 1, "document": {
            "id": f"q{index}", "user_email": owner,
            "executed_at": (start + timedelta(minutes=index)).isoformat()}}
            for index in range(history.MAX_HISTORY_PER_OWNER)]
        with patch.object(history.product_store, "list_records",
                          return_value=list(reversed(records))), \
             patch.object(history.product_store, "transact") as transact:
            history.run_retention_sweep(owner)
        removed = [item.record_id for item in transact.call_args.args[0]]
        self.assertEqual(removed,
                         [f"q{index}" for index in range(history._RETENTION_TRIM_BATCH)])

    def test_a_record_belonging_to_someone_else_stops_the_sweep(self):
        owner = "owner@example.test"
        records = _records(owner, history.MAX_HISTORY_PER_OWNER)
        records[0]["document"]["user_email"] = "someone@else.test"
        with patch.object(history.product_store, "list_records", return_value=records), \
             patch.object(history.product_store, "transact") as transact:
            with self.assertRaisesRegex(RuntimeError, "retention state is invalid"):
                history.run_retention_sweep(owner)
        transact.assert_not_called()

    def test_a_listing_that_loses_its_snapshot_is_re_read(self):
        """Moving the listing off the request path put it in a race it never
        had before: a dashboard opening twelve tiles writes twelve history
        records while the sweep is still paging, the listing refuses to span a
        snapshot that moved, and every sweep failed. A burst is short, so the
        listing is simply re-read."""
        owner = "owner@example.test"
        moved = HTTPException(409, "KaveonDB product list changed during pagination")
        with patch.object(history.product_store, "list_records",
                          side_effect=[moved, moved, _records(owner, 400)]) as listed, \
             patch.object(history.time, "sleep"), \
             patch.object(history.product_store, "transact"):
            self.assertEqual(history.run_retention_sweep(owner), 0)
        self.assertEqual(listed.call_count, 3)
        self.assertEqual(history._HISTORY_COUNTS[owner], 400)

    def test_a_listing_that_never_settles_gives_up_rather_than_spinning(self):
        owner = "owner@example.test"
        moved = HTTPException(409, "KaveonDB product list changed during pagination")
        with patch.object(history.product_store, "list_records",
                          side_effect=moved) as listed, \
             patch.object(history.time, "sleep"):
            with self.assertRaises(HTTPException):
                history.run_retention_sweep(owner)
        self.assertEqual(listed.call_count, history._RETENTION_LIST_ATTEMPTS)

    def test_a_failure_that_is_not_a_lost_snapshot_is_not_retried(self):
        owner = "owner@example.test"
        with patch.object(history.product_store, "list_records",
                          side_effect=HTTPException(503, "bound exceeded")) as listed:
            with self.assertRaises(HTTPException):
                history.run_retention_sweep(owner)
        self.assertEqual(listed.call_count, 1)

    def test_a_record_with_no_revision_stops_the_sweep(self):
        owner = "owner@example.test"
        records = _records(owner, history.MAX_HISTORY_PER_OWNER)
        records[0]["revision"] = None
        with patch.object(history.product_store, "list_records", return_value=records), \
             patch.object(history.product_store, "transact") as transact:
            with self.assertRaisesRegex(RuntimeError, "retention state is invalid"):
                history.run_retention_sweep(owner)
        transact.assert_not_called()


if __name__ == "__main__":
    unittest.main()
