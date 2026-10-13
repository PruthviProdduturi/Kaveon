"""The retention bound and the store's listing ceiling are related numbers.

Retention was set to exactly the ceiling `product_store.list_records`
enforces, so an owner the sweep had filled could not be listed at all: every
read of their history answered 503 and the page reported no history for the
person who had the most of it. Rows written between sweeps pushed the count
over with nothing to pull it back until the next sweep ran.

These cases pin the relationship rather than the values, so raising one
number without the other fails here instead of in production.
"""

import unittest
from unittest.mock import patch

from services import product_read_authority, query_history


class RetentionStaysBelowTheListingCeiling(unittest.TestCase):
    def test_the_bound_leaves_room_under_the_ceiling(self):
        self.assertLess(query_history.MAX_HISTORY_PER_OWNER,
                        query_history.HISTORY_LIST_CEILING)
        # Enough room for the rows that arrive between sweeps, not a token gap.
        self.assertGreaterEqual(
            query_history.HISTORY_LIST_CEILING - query_history.MAX_HISTORY_PER_OWNER,
            100,
        )

    def test_the_ceiling_is_one_the_store_will_accept(self):
        # product_store refuses a max_records above 1000, so a ceiling above it
        # would turn every read into a 422 rather than a 503.
        self.assertLessEqual(query_history.HISTORY_LIST_CEILING, 1000)
        self.assertGreaterEqual(query_history.HISTORY_LIST_CEILING, 1)

    def test_a_history_read_states_the_bound_it_expects(self):
        with patch.object(product_read_authority, "enabled", return_value=True):
            with patch.object(product_read_authority, "list_documents",
                              return_value=[]) as listed:
                query_history.list_history("owner@example.test", limit=50)
        _args, kwargs = listed.call_args
        # Inheriting the default is what let a read discover the bound by
        # exceeding it.
        self.assertEqual(kwargs.get("max_records"), query_history.HISTORY_LIST_CEILING)

    def test_an_owner_holding_the_retention_maximum_is_still_listable(self):
        full = [{"id": str(n)} for n in range(query_history.MAX_HISTORY_PER_OWNER)]
        with patch.object(product_read_authority, "enabled", return_value=True):
            with patch.object(product_read_authority, "list_documents",
                              return_value=full):
                rows = query_history.list_history("owner@example.test", limit=50)
        self.assertEqual(len(rows), 50)
        self.assertLessEqual(len(full), query_history.HISTORY_LIST_CEILING)


if __name__ == "__main__":
    unittest.main()
