"""The chat message population bound, and why it is not the listing ceiling.

Every read of chat history lists an owner's whole message family — opening one
conversation lists them all and filters (`routers/chat_history.py`), and so
does deleting one — and `product_store.list_records` answers 503 once a
listing passes the bound it was given. The declared bound used to BE that
ceiling and nothing enforced it, so it was a number an owner discovered by
exceeding it: past 1,000 messages every conversation answered 503, deleting a
conversation answered 503, and there was no way back.

These tests pin the two properties that keep that from happening: the append
refuses at a population bound that sits below the ceiling and says what to do,
and the delete lists at the ceiling rather than at the bound so an owner who
is already over it can still get back under.
"""

import unittest
from unittest.mock import patch

from fastapi import HTTPException

from services import chat_history_store as store


OWNER = "alice@example.com"


class MessageBoundTests(unittest.TestCase):
    def setUp(self):
        store._forget_count(OWNER)
        self.addCleanup(store._forget_count, OWNER)

    def test_the_population_bound_sits_below_the_listing_ceiling(self):
        """Related numbers, stated together. Raising one without the other
        reintroduces the fault this bound exists to prevent."""
        self.assertLess(store.MAX_OWNER_MESSAGES, store.MESSAGE_LIST_CEILING)
        self.assertLess(store.MAX_OWNER_MESSAGES + store._COUNT_CHECK_MARGIN,
                        store.MESSAGE_LIST_CEILING)

    def test_an_append_at_the_bound_is_refused_with_something_to_do(self):
        store._remember_count(OWNER, store.MAX_OWNER_MESSAGES)
        with patch.object(store.product_store, "transact",
                          side_effect=AssertionError("wrote past the bound")), \
             patch.object(store, "_owned_session",
                          side_effect=AssertionError("read the session")):
            with self.assertRaises(HTTPException) as caught:
                store.add_message("11", OWNER, role="user", content="hello")
        self.assertEqual(caught.exception.status_code, 409)
        self.assertEqual(caught.exception.detail["code"], "history_full")
        self.assertIn("Delete a conversation", caught.exception.detail["message"])

    def test_an_append_below_the_bound_is_stored_and_counted(self):
        store._remember_count(OWNER, 10)
        with patch.object(store, "_owned_session", return_value=({"id": "11"}, 4)), \
             patch.object(store.product_store, "transact", return_value={}), \
             patch.object(store, "_schedule_count") as scheduled:
            store.add_message("11", OWNER, role="user", content="hello")
        self.assertEqual(store._known_count(OWNER), 11)
        scheduled.assert_called_once_with(OWNER)

    def test_an_uncounted_owner_is_answered_and_counted_behind_the_answer(self):
        """Counting is a paged read, so no append waits for one."""
        with patch.object(store, "_owned_session", return_value=({"id": "11"}, 4)), \
             patch.object(store.product_store, "transact", return_value={}), \
             patch.object(store.product_store, "count_records",
                          side_effect=AssertionError("the append counted inline")), \
             patch.object(store.threading, "Thread") as thread:
            store.add_message("11", OWNER, role="user", content="hello")
        thread.assert_called_once()
        self.assertIsNone(store._known_count(OWNER))


class CountingTests(unittest.TestCase):
    def setUp(self):
        store._forget_count(OWNER)
        self.addCleanup(store._forget_count, OWNER)

    def test_a_family_past_the_ceiling_counts_to_the_ceiling(self):
        """Counted, not listed: a listing answers 503 for exactly this owner,
        and `count_records` reports them as counted-to-the-bound instead."""
        with patch.object(store.product_store, "count_records",
                          return_value={"records": store.MESSAGE_LIST_CEILING,
                                        "truncated": True, "snapshotId": "s"}):
            self.assertEqual(store.count_owner_messages(OWNER),
                             store.MESSAGE_LIST_CEILING)
        self.assertGreaterEqual(store._known_count(OWNER), store.MAX_OWNER_MESSAGES)

    def test_an_unreachable_store_leaves_the_count_unknown(self):
        """The failure must not become a plausible count in either direction:
        an invented zero would never refuse, and an invented ceiling would
        refuse every append while the store was merely unreachable."""
        with patch.object(store.product_store, "count_records",
                          side_effect=HTTPException(503, "product store is unavailable")):
            with self.assertRaises(HTTPException):
                store.count_owner_messages(OWNER)
        self.assertIsNone(store._known_count(OWNER))

    def test_a_failed_count_never_reaches_the_message_that_triggered_it(self):
        store._COUNTING.add(OWNER)
        with patch.object(store.product_store, "count_records",
                          side_effect=HTTPException(503, "unavailable")):
            store._establish_count(OWNER)
        self.assertIsNone(store._known_count(OWNER))
        self.assertNotIn(OWNER, store._COUNTING)

    def test_an_invalid_count_is_not_remembered(self):
        with patch.object(store.product_store, "count_records",
                          return_value={"records": None, "truncated": False}):
            with self.assertRaisesRegex(RuntimeError, "invalid chat message count"):
                store.count_owner_messages(OWNER)
        self.assertIsNone(store._known_count(OWNER))


class DeleteSessionBoundTests(unittest.TestCase):
    def setUp(self):
        store._forget_count(OWNER)
        self.addCleanup(store._forget_count, OWNER)

    def _message(self, index, session_id="11"):
        return {"id": f"m{index}", "revision": 1,
                "document": {"id": f"m{index}", "session_id": session_id,
                             "user_email": OWNER}}

    def test_the_delete_lists_at_the_ceiling_not_at_the_bound(self):
        """An owner over the population bound is precisely the one who needs
        this to work, so the listing is given the transport ceiling."""
        listed = {}
        with patch.object(store.product_store, "read",
                          return_value={"revision": 2,
                                        "document": {"id": "11", "user_email": OWNER}}), \
             patch.object(store.product_store, "list_records",
                          side_effect=lambda *a, **k: listed.update(k) or [self._message(1)]), \
             patch.object(store.product_store, "transact", return_value={}):
            self.assertTrue(store.delete_session("11", OWNER))
        self.assertEqual(listed["max_records"], store.MESSAGE_LIST_CEILING)

    def test_removing_messages_forgets_the_remembered_count(self):
        """A count left high would refuse appends the owner now has room for."""
        store._remember_count(OWNER, store.MAX_OWNER_MESSAGES)
        with patch.object(store.product_store, "read",
                          return_value={"revision": 2,
                                        "document": {"id": "11", "user_email": OWNER}}), \
             patch.object(store.product_store, "list_records",
                          return_value=[self._message(1), self._message(2)]), \
             patch.object(store.product_store, "transact", return_value={}):
            store.delete_session("11", OWNER)
        self.assertIsNone(store._known_count(OWNER))


if __name__ == "__main__":
    unittest.main()
