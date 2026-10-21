"""Retention has to delete the right records, and only when it should.

A sweep deletes. That is enough reason to test it harder than the thing it
protects: a bound that trims too eagerly loses records nobody asked it to
lose, and one that never runs leaves the family unbounded while looking
handled.

The cases here are the ones where getting it wrong is silent: an owner under
the bound must be left alone, the oldest must go rather than whichever the
listing happened to return first, a record belonging to someone else must stop
the sweep rather than be deleted, and two writes from one owner must not start
two sweeps that race for the same revisions.
"""
import threading
import unittest
from unittest import mock

from services import product_retention


def _record(index: int, owner: str = "person@example.test", *, revision: int = 1) -> dict:
    return {
        "revision": revision,
        "document": {
            "id": f"event-{index:04}",
            "user_email": owner,
            # Ordered oldest first by index, so the expected victims are known.
            "timestamp": f"2026-01-01T00:{index:02}:00",
        },
    }


def _policy(cap: int = 10, limit: int = 100) -> product_retention.RetentionPolicy:
    return product_retention.RetentionPolicy(
        kind="activity", cap=cap, list_limit=limit,
        ordered_by="timestamp", owner_field="user_email",
    )


class Sweeping(unittest.TestCase):
    def setUp(self) -> None:
        self.owner = "person@example.test"
        product_retention._IN_FLIGHT.clear()
        self.addCleanup(product_retention._IN_FLIGHT.clear)

    def test_an_owner_under_the_bound_is_left_alone(self) -> None:
        records = [_record(index) for index in range(5)]
        with mock.patch.object(product_retention.product_store, "list_records_capped",
                               return_value=(records, False)), \
             mock.patch.object(product_retention.product_store, "transact",
                               side_effect=AssertionError("nothing should be deleted")):
            self.assertEqual(product_retention.sweep(_policy(cap=10), self.owner), 0)

    def test_the_oldest_go_first(self) -> None:
        """Not whichever the listing returned first."""
        records = [_record(index) for index in range(15)]
        # Returned newest-first, so an implementation trusting listing order
        # would delete the newest five -- the opposite of retention.
        records.reverse()
        captured: list = []
        with mock.patch.object(product_retention.product_store, "list_records_capped",
                               return_value=(records, False)), \
             mock.patch.object(product_retention.product_store, "transact",
                               side_effect=lambda mutations, *a, **k: captured.extend(mutations)):
            deleted = product_retention.sweep(_policy(cap=10), self.owner)

        self.assertEqual(deleted, 5)
        self.assertEqual(
            [mutation.record_id for mutation in captured],
            [f"event-{index:04}" for index in range(5)],
            "the five oldest must go, in order",
        )
        self.assertTrue(all(m.operation == "delete" for m in captured))
        self.assertTrue(all(m.expected_revision == 1 for m in captured),
                        "a delete must name the revision it expects")

    def test_a_record_belonging_to_someone_else_stops_the_sweep(self) -> None:
        """Deleting on a mismatched listing would delete another person's record."""
        records = [_record(index) for index in range(15)]
        records[0]["document"]["user_email"] = "someone.else@example.test"
        with mock.patch.object(product_retention.product_store, "list_records_capped",
                               return_value=(records, False)), \
             mock.patch.object(product_retention.product_store, "transact",
                               side_effect=AssertionError("must not delete")):
            with self.assertRaises(RuntimeError):
                product_retention.sweep(_policy(cap=10), self.owner)

    def test_a_missing_revision_stops_the_sweep(self) -> None:
        records = [_record(index) for index in range(15)]
        records[0]["revision"] = None
        with mock.patch.object(product_retention.product_store, "list_records_capped",
                               return_value=(records, False)), \
             mock.patch.object(product_retention.product_store, "transact",
                               side_effect=AssertionError("must not delete")):
            with self.assertRaises(RuntimeError):
                product_retention.sweep(_policy(cap=10), self.owner)

    def test_a_truncated_listing_still_trims(self) -> None:
        """The population is unknown but known to be over, so clear a batch."""
        records = [_record(index) for index in range(100)]
        with mock.patch.object(product_retention.product_store, "list_records_capped",
                               return_value=(records, True)), \
             mock.patch.object(product_retention.product_store, "transact"):
            deleted = product_retention.sweep(_policy(cap=10, limit=100), self.owner)
        self.assertEqual(deleted, product_retention.MAX_DELETE_FANOUT)

    def test_one_transaction_is_never_exceeded(self) -> None:
        records = [_record(index) for index in range(500)]
        with mock.patch.object(product_retention.product_store, "list_records_capped",
                               return_value=(records, False)), \
             mock.patch.object(product_retention.product_store, "transact"):
            deleted = product_retention.sweep(_policy(cap=10, limit=1000), self.owner)
        self.assertLessEqual(deleted, product_retention.MAX_DELETE_FANOUT)


class Scheduling(unittest.TestCase):
    def setUp(self) -> None:
        self.owner = "person@example.test"
        product_retention._IN_FLIGHT.clear()
        self.addCleanup(product_retention._IN_FLIGHT.clear)

    def test_a_burst_from_one_owner_starts_one_sweep(self) -> None:
        """Otherwise each sweep races the others for the same revisions."""
        started: list = []

        class Recording:
            def __init__(self, target, name=None, daemon=None):
                self.target = target
                started.append(name)

            def start(self):
                pass  # never run, so the claim stays held

        for _ in range(5):
            product_retention.schedule(_policy(), self.owner, spawn=Recording)
        self.assertEqual(len(started), 1, "a held claim must suppress the rest")

    def test_a_failing_sweep_releases_its_claim(self) -> None:
        """A claim held forever would stop this owner ever being swept again."""
        policy = _policy()

        class Immediate:
            def __init__(self, target, name=None, daemon=None):
                self.target = target

            def start(self):
                self.target()

        with mock.patch.object(product_retention, "sweep",
                               side_effect=RuntimeError("storage is unavailable")):
            product_retention.schedule(policy, self.owner, spawn=Immediate)

        self.assertNotIn((policy.kind, self.owner), product_retention._IN_FLIGHT)
        # And a later write may try again.
        with mock.patch.object(product_retention, "sweep", return_value=0) as swept:
            product_retention.schedule(policy, self.owner, spawn=Immediate)
        swept.assert_called_once()

    def test_different_owners_do_not_block_each_other(self) -> None:
        started: list = []

        class Recording:
            def __init__(self, target, name=None, daemon=None):
                started.append(name)

            def start(self):
                pass

        product_retention.schedule(_policy(), "alice@example.test", spawn=Recording)
        product_retention.schedule(_policy(), "bob@example.test", spawn=Recording)
        self.assertEqual(len(started), 2)


class PolicyShape(unittest.TestCase):
    def test_a_cap_at_or_above_the_listing_limit_is_refused(self) -> None:
        """The fault this guards against has already happened twice.

        An owner at the listing ceiling could not be listed, so every read of
        their history answered 503 and the sweep that would have brought them
        back under it read the same listing and failed the same way.
        """
        with self.assertRaises(ValueError):
            product_retention.RetentionPolicy(
                kind="activity", cap=1_000, list_limit=1_000,
                ordered_by="timestamp", owner_field="user_email")

    def test_the_activity_policy_is_shaped_correctly(self) -> None:
        from services import source_cutover_mutations as mutations

        policy = mutations.ACTIVITY_RETENTION
        self.assertEqual(policy.kind, "activity")
        self.assertLess(policy.cap, policy.list_limit)
        self.assertEqual(policy.owner_field, "user_email")


if __name__ == "__main__":
    unittest.main()
