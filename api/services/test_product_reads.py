"""What the Library must not pay for one round trip at a time.

A product read costs about half a second inside the Engine whatever it asks
for, so the only thing these tests can assert from here is that the reads a
response needs are in flight together rather than one after another. They do
that with a barrier: a read blocks until every other read of the same response
has also started, so a path that still took them in turn deadlocks and the
barrier's timeout fails the test. Nothing here measures time, which would be a
flake on a loaded machine.
"""

import os
import threading
import unittest
from unittest.mock import patch

from services import (charts, dashboards, datasets, favorites, product_reads,
                      product_read_authority as authority, saved_queries)

ACTOR = "pruthvi@example.test"
JPEG = "data:image/jpeg;base64,/9j/4AAQSkZJRg=="


def _document(identity: str, **extra) -> dict:
    return {"id": identity, "name": "Card", "visibility": "published",
            "created_by": "owner@example.test", "updated_at": "2026-01-01T00:00:00Z",
            "query_config": {}, "viz_config": {}, **extra}


class GatherTests(unittest.TestCase):
    def test_answers_come_back_in_the_order_they_were_asked_for(self):
        self.assertEqual(
            product_reads.gather(lambda: "first", lambda: "second", lambda: "third"),
            ["first", "second", "third"],
        )

    def test_a_single_read_needs_no_thread(self):
        caller = threading.current_thread()
        self.assertEqual(product_reads.gather(lambda: threading.current_thread()),
                         [caller])

    def test_a_read_that_fails_fails_the_whole_gather(self):
        def broken():
            raise RuntimeError("catalog unavailable")

        with self.assertRaisesRegex(RuntimeError, "catalog unavailable"):
            product_reads.gather(lambda: "fine", broken)

    def test_concurrency_is_bounded(self):
        live, peak, lock = 0, 0, threading.Lock()

        def read():
            nonlocal live, peak
            with lock:
                live += 1
                peak = max(peak, live)
            try:
                return None
            finally:
                with lock:
                    live -= 1

        product_reads.gather(*[read] * 12)
        self.assertLessEqual(peak, product_reads.MAX_CONCURRENT_READS)


class LibraryListConcurrencyTests(unittest.TestCase):
    """Every Library tab's independent listings are taken at once."""

    def _assert_listed_together(self, families, operation):
        barrier = threading.Barrier(len(families), timeout=10)
        listed, lock = [], threading.Lock()

        def list_records(kind, *args, **kwargs):
            with lock:
                listed.append(kind)
            # Returns only once every other listing of this response has also
            # started; a sequential path never gets past the first.
            barrier.wait()
            return []

        with patch.dict(os.environ, {authority.ENVIRONMENT_KEY: "all"}, clear=True), \
             patch.object(authority.product_store, "list_records", side_effect=list_records):
            operation()
        self.assertEqual(sorted(listed), sorted(families))

    def test_the_dashboards_tab_reads_its_family_and_the_favorites_together(self):
        self._assert_listed_together(
            ["dashboard", "favorite"],
            lambda: dashboards.list_dashboards(ACTOR, "Admin"),
        )

    def test_the_datasets_tab_reads_its_family_and_the_favorites_together(self):
        self._assert_listed_together(
            ["dataset", "favorite"],
            lambda: datasets.list_datasets(ACTOR, "Admin"),
        )

    def test_the_saved_queries_tab_reads_its_family_and_the_favorites_together(self):
        self._assert_listed_together(
            ["saved_query", "favorite"],
            lambda: saved_queries.list_saved_queries(ACTOR),
        )

    def test_the_charts_tab_also_reads_the_dataset_names_at_the_same_time(self):
        datasets._name_index_cache = None
        self._assert_listed_together(
            ["chart", "favorite", "dataset"],
            lambda: charts.list_charts(ACTOR, "Admin"),
        )

    def test_a_chart_list_whose_dataset_names_are_unreadable_still_answers(self):
        """The index is read speculatively, so it may only break a list that needs it."""
        documents = [_document("chart-1")]
        datasets._name_index_cache = None
        with patch.dict(os.environ, {authority.ENVIRONMENT_KEY: "all"}, clear=True), \
             patch("services.product_read_authority.list_documents", return_value=documents), \
             patch.object(datasets, "_read_name_index",
                          side_effect=RuntimeError("dataset listing unavailable")):
            self.assertEqual([item["id"] for item in charts.list_charts(ACTOR, "Admin")],
                             ["chart-1"])

    def test_a_chart_that_names_a_dataset_still_fails_when_the_names_are_unreadable(self):
        documents = [_document("chart-1", dataset_id="7")]
        datasets._name_index_cache = None
        with patch.dict(os.environ, {authority.ENVIRONMENT_KEY: "all"}, clear=True), \
             patch("services.product_read_authority.list_documents", return_value=documents), \
             patch.object(datasets, "_read_name_index",
                          side_effect=RuntimeError("dataset listing unavailable")):
            with self.assertRaisesRegex(RuntimeError, "dataset listing unavailable"):
                charts.list_charts(ACTOR, "Admin")


class CardReadTests(unittest.TestCase):
    def test_a_card_read_takes_its_record_and_its_favorite_at_once(self):
        barrier = threading.Barrier(2, timeout=10)

        def read(kind, record_id, *args, **kwargs):
            barrier.wait()
            if kind == "favorite":
                self.assertEqual(record_id,
                                 favorites._record_id(ACTOR, "chart", "chart-1"))
                return {"document": {"user_email": ACTOR}}
            return {"document": _document("chart-1")}

        with patch.dict(os.environ, {authority.ENVIRONMENT_KEY: "all"}, clear=True), \
             patch.object(authority.product_store, "read", side_effect=read), \
             patch.object(datasets, "_read_name_index", return_value={}):
            chart = charts.get_chart_by_id("chart-1", ACTOR, "Admin")
        self.assertTrue(chart["favorite"])

    def test_a_favorite_is_read_against_what_the_document_calls_itself(self):
        """A document that names an identity other than its key must not be
        reported with the favorite of the key it was read under."""
        keys = []

        def read(kind, record_id, *args, **kwargs):
            if kind == "favorite":
                keys.append(record_id)
                return None
            return {"document": _document("canonical")}

        with patch.dict(os.environ, {authority.ENVIRONMENT_KEY: "all"}, clear=True), \
             patch.object(authority.product_store, "read", side_effect=read):
            authority.read_document("charts", "alias", ACTOR, "Admin")
        self.assertIn(favorites._record_id(ACTOR, "chart", "canonical"), keys)


class CardCoverTests(unittest.TestCase):
    """A stored preview is read with one round trip, not three."""

    def _assert_single_read(self, kind, document, operation):
        read = []

        def product_read(requested_kind, record_id, *args, **kwargs):
            read.append(requested_kind)
            return {"document": document} if requested_kind == kind else None

        with patch.dict(os.environ, {authority.ENVIRONMENT_KEY: "all"}, clear=True), \
             patch.object(authority.product_store, "read", side_effect=product_read), \
             patch.object(authority.product_store, "list_records",
                          side_effect=AssertionError("a cover listed a product family")):
            self.assertEqual(operation(), JPEG)
        self.assertEqual(read, [kind])

    def test_a_chart_cover_asks_for_neither_the_favorite_nor_the_dataset_name(self):
        self._assert_single_read(
            "chart", _document("chart-1", dataset_id="7", thumbnail=JPEG),
            lambda: charts.get_chart_thumbnail("chart-1", ACTOR, "Admin"),
        )

    def test_a_dashboard_cover_asks_for_nothing_but_the_dashboard(self):
        self._assert_single_read(
            "dashboard", _document("dash-1", thumbnail=JPEG, layout=[], charts=[], filters=[]),
            lambda: dashboards.get_dashboard_thumbnail("dash-1", ACTOR, "Admin", False),
        )

    def test_a_dashboard_cover_still_falls_back_to_the_other_theme(self):
        self._assert_single_read(
            "dashboard", _document("dash-1", thumbnail=JPEG, layout=[], charts=[], filters=[]),
            lambda: dashboards.get_dashboard_thumbnail("dash-1", ACTOR, "Admin", True),
        )

    def test_a_cover_is_still_refused_to_a_viewer_who_may_not_see_the_record(self):
        document = _document("chart-1", visibility="private", thumbnail=JPEG)
        with patch.dict(os.environ, {authority.ENVIRONMENT_KEY: "all"}, clear=True), \
             patch.object(authority.product_store, "read",
                          side_effect=lambda kind, *a, **k: {"document": document}):
            self.assertIsNone(charts.get_chart_thumbnail("chart-1", ACTOR, "Viewer"))

    def test_a_cover_read_never_publishes_a_favorite_it_did_not_read(self):
        document = _document("chart-1", thumbnail=JPEG)
        with patch.dict(os.environ, {authority.ENVIRONMENT_KEY: "all"}, clear=True), \
             patch.object(authority.product_store, "read",
                          side_effect=lambda kind, *a, **k:
                          {"document": document} if kind == "chart" else
                          self.fail("a cover read the favorites")):
            result = authority.read_document("charts", "chart-1", ACTOR, "Admin",
                                             with_favorite=False)
        self.assertFalse(result["favorite"])


if __name__ == "__main__":
    unittest.main()
