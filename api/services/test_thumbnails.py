"""Library preview images: the stored-value guard and the serving contract.

The regression these cover: the KaveonDB product-record cutover projected a
chart or dashboard document without its `thumbnail`, so a captured preview was
accepted by the API, written away and read back as null. Every Library card
drew a placeholder because no record could hold a preview.
"""

import sys
import unittest
from types import SimpleNamespace
from unittest.mock import patch

if "pyodbc" not in sys.modules:
    sys.modules["pyodbc"] = SimpleNamespace(Error=Exception)

from services import charts, dashboards, thumbnails

JPEG = "data:image/jpeg;base64,/9j/4AAQSkZJRg=="
PNG = "data:image/png;base64,iVBORw0KGgo="


class ThumbnailGuardTests(unittest.TestCase):
    def test_supported_image_data_uris_are_stored_unchanged(self):
        for value in (JPEG, PNG, "data:image/webp;base64,UklGRg=="):
            with self.subTest(value=value[:24]):
                self.assertEqual(thumbnails.normalise(value, 1000), value)

    def test_nothing_to_store_is_not_an_error(self):
        self.assertIsNone(thumbnails.normalise(None, 1000))
        self.assertIsNone(thumbnails.normalise("", 1000))

    def test_anything_but_a_bounded_base64_image_is_refused(self):
        cases = {
            "https://example.test/preview.jpg": "data URI",
            "data:image/jpeg,raw": "base64 encoded",
            "data:text/html;base64,PHNjcmlwdD4=": "JPEG, PNG or WebP",
            "data:image/jpeg;base64,": "JPEG, PNG or WebP",
            JPEG + "A" * 1000: "size limit",
        }
        for value, message in cases.items():
            with self.subTest(value=value[:32]):
                with self.assertRaisesRegex(ValueError, message):
                    thumbnails.normalise(value, len(JPEG) + 1)

    def test_decode_returns_image_bytes_and_media_type(self):
        content, media_type = thumbnails.decode(JPEG)
        self.assertEqual(media_type, "image/jpeg")
        self.assertTrue(content.startswith(b"\xff\xd8\xff"))

    def test_decode_refuses_a_corrupt_stored_value(self):
        with self.assertRaisesRegex(ValueError, "valid base64"):
            thumbnails.decode("data:image/jpeg;base64,not-base64!!")

    def test_the_dashboard_budget_is_larger_than_the_chart_budget(self):
        # A dashboard preview is a whole canvas rather than one plot, and is
        # captured twice; both budgets stay far below an exportable image.
        self.assertLess(thumbnails.CHART_MAX_CHARS, thumbnails.DASHBOARD_MAX_CHARS)
        self.assertLess(thumbnails.DASHBOARD_MAX_CHARS, 1024 * 1024)


class ChartThumbnailTests(unittest.TestCase):
    def _document(self, data, prior=None):
        with patch.object(charts.product_store, "read", return_value={"revision": 3}):
            return charts._product_document(
                {"dataset_id": "7", **data}, "chart-1", "owner@example.test", prior,
            )

    def _preview_write(self, thumbnail, prior):
        """A write carrying nothing but the preview, as the capture sends it."""
        with patch.object(charts.product_store, "read", return_value={"revision": 3}):
            return charts._product_document(
                {"thumbnail": thumbnail}, "chart-1", "owner@example.test", prior,
            )

    def test_a_captured_preview_reaches_the_kaveondb_document(self):
        self.assertEqual(self._document({"thumbnail": JPEG})["thumbnail"], JPEG)

    def test_a_stored_preview_survives_an_update_that_does_not_mention_it(self):
        prior = {"created_by": "owner@example.test", "thumbnail": JPEG, "dataset_id": "7"}
        self.assertEqual(self._document({"name": "Renamed"}, prior)["thumbnail"], JPEG)

    def test_a_chart_with_no_preview_carries_no_key(self):
        self.assertNotIn("thumbnail", self._document({"name": "Fresh"}))

    def test_an_explicit_clear_removes_the_preview(self):
        prior = {"created_by": "owner@example.test", "thumbnail": JPEG, "dataset_id": "7"}
        self.assertNotIn("thumbnail", self._document({"thumbnail": None}, prior))

    def test_an_oversized_preview_is_refused_rather_than_written(self):
        oversized = JPEG + "A" * thumbnails.CHART_MAX_CHARS
        with self.assertRaisesRegex(ValueError, "size limit"):
            self._document({"thumbnail": oversized})

    def test_the_read_adapter_surfaces_the_stored_preview(self):
        adapted = charts._adapt_product({
            "id": "chart-1", "created_by": "owner@example.test",
            "query_config": {}, "viz_config": {}, "thumbnail": JPEG,
            "thumbnail_updated_at": "2026-02-02T00:00:00Z",
        }, {})
        self.assertEqual(adapted["thumbnail"], JPEG)
        self.assertTrue(adapted["has_thumbnail"])
        self.assertEqual(adapted["thumbnail_updated_at"], "2026-02-02T00:00:00Z")

    def test_a_captured_preview_records_when_it_was_captured(self):
        document = self._document({"thumbnail": JPEG})
        self.assertEqual(document["thumbnail_updated_at"], document["updated_at"])

    def test_storing_a_preview_is_not_an_edit_of_the_chart(self):
        # A preview write that moved `updated_at` would leave the record newer
        # than the preview it had just stored, so the refresh pass would call
        # that preview stale the instant it was taken and never settle.
        prior = {
            "created_by": "owner@example.test", "dataset_id": "7",
            "updated_at": "2026-01-01T00:00:00Z", "modified_by": "editor@example.test",
        }
        document = self._preview_write(JPEG, prior)
        self.assertEqual(document["updated_at"], "2026-01-01T00:00:00Z")
        self.assertEqual(document["modified_by"], "editor@example.test")
        self.assertGreater(document["thumbnail_updated_at"], document["updated_at"])

    def test_an_edit_leaves_the_record_newer_than_its_preview(self):
        prior = {
            "created_by": "owner@example.test", "dataset_id": "7", "thumbnail": JPEG,
            "thumbnail_updated_at": "2026-01-01T00:00:00Z",
            "updated_at": "2026-01-01T00:00:00Z",
        }
        document = self._document({"name": "Renamed"}, prior)
        self.assertEqual(document["thumbnail_updated_at"], "2026-01-01T00:00:00Z")
        self.assertGreater(document["updated_at"], document["thumbnail_updated_at"])

    def test_a_preview_stored_before_capture_times_existed_claims_none(self):
        # Nothing to backfill from, so the record reports no capture time and
        # the refresh pass leaves that preview alone rather than guessing.
        prior = {
            "created_by": "owner@example.test", "dataset_id": "7", "thumbnail": JPEG,
            "updated_at": "2026-01-01T00:00:00Z",
        }
        self.assertNotIn("thumbnail_updated_at", self._document({"name": "Renamed"}, prior))

    def test_a_list_entry_reports_the_preview_without_carrying_it(self):
        records = [{"document": {
            "id": "chart-1", "name": "Trips", "visibility": "published",
            "created_by": "owner@example.test", "updated_at": "2026-01-01T00:00:00Z",
            "query_config": {}, "viz_config": {}, "thumbnail": JPEG,
        }}]
        with patch.dict("os.environ", {"KAVEONDB_READ_AUTHORITY_FAMILIES": "charts"}, clear=True), \
             patch.object(charts.product_store, "list_records",
                          side_effect=lambda kind, *a, **k: records if kind == "chart" else []):
            items = charts.list_charts("owner@example.test", "Admin")
        self.assertEqual(len(items), 1)
        self.assertIsNone(items[0]["thumbnail"])
        self.assertTrue(items[0]["has_thumbnail"])

    def test_a_point_read_withholds_the_image_until_it_is_asked_for(self):
        document = {
            "id": "chart-1", "name": "Trips", "visibility": "published",
            "created_by": "owner@example.test", "query_config": {}, "viz_config": {},
            "thumbnail": JPEG,
        }
        with patch.dict("os.environ", {"KAVEONDB_READ_AUTHORITY_FAMILIES": "charts"}, clear=True), \
             patch.object(charts.product_store, "read",
                          side_effect=lambda kind, *a, **k: {"document": dict(document), "revision": 1}):
            withheld = charts.get_chart_by_id("chart-1", "owner@example.test", "Admin")
            served = charts.get_chart_thumbnail("chart-1", "owner@example.test", "Admin")
        self.assertIsNone(withheld["thumbnail"])
        self.assertTrue(withheld["has_thumbnail"])
        self.assertEqual(served, JPEG)


class DashboardThumbnailTests(unittest.TestCase):
    def _document(self, data, prior=None):
        return dashboards._product_document(data, "dash-1", "owner@example.test", prior)

    def test_each_theme_slot_survives_an_update_naming_only_the_other(self):
        prior = {"created_by": "owner@example.test", "thumbnail": JPEG, "charts": []}
        document = self._document({"thumbnail_dark": PNG}, prior)
        self.assertEqual(document["thumbnail"], JPEG)
        self.assertEqual(document["thumbnail_dark"], PNG)

    def test_a_dashboard_with_no_preview_carries_neither_key(self):
        document = self._document({"name": "Ops", "charts": []})
        self.assertNotIn("thumbnail", document)
        self.assertNotIn("thumbnail_dark", document)
        self.assertNotIn("thumbnail_updated_at", document)
        self.assertNotIn("thumbnail_dark_updated_at", document)

    def test_both_themes_can_be_current_at_once(self):
        # Each theme is captured in its own pass, and a capture that moved
        # `updated_at` pushed the record past the other theme's capture time —
        # so whichever was captured first was stale again by the time the
        # second finished and the pass could never report the pair as done.
        prior = {
            "created_by": "owner@example.test", "charts": [],
            "updated_at": "2026-01-01T00:00:00Z",
        }
        after_light = self._document({"thumbnail": JPEG}, prior)
        self.assertEqual(after_light["updated_at"], "2026-01-01T00:00:00Z")
        after_dark = self._document({"thumbnail_dark": PNG}, after_light)
        self.assertEqual(after_dark["updated_at"], "2026-01-01T00:00:00Z")
        self.assertEqual(after_dark["thumbnail_updated_at"], after_light["thumbnail_updated_at"])
        self.assertGreater(after_dark["thumbnail_updated_at"], after_dark["updated_at"])
        self.assertGreater(after_dark["thumbnail_dark_updated_at"], after_dark["updated_at"])

    def test_an_edit_leaves_the_record_newer_than_both_previews(self):
        prior = {
            "created_by": "owner@example.test", "charts": [],
            "thumbnail": JPEG, "thumbnail_dark": PNG,
            "thumbnail_updated_at": "2026-01-01T00:00:00Z",
            "thumbnail_dark_updated_at": "2026-01-01T00:00:01Z",
            "updated_at": "2026-01-01T00:00:01Z",
        }
        document = self._document({"name": "Renamed"}, prior)
        self.assertEqual(document["thumbnail_updated_at"], "2026-01-01T00:00:00Z")
        self.assertEqual(document["thumbnail_dark_updated_at"], "2026-01-01T00:00:01Z")
        self.assertGreater(document["updated_at"], document["thumbnail_dark_updated_at"])

    def test_a_list_entry_reports_both_slots_without_carrying_either(self):
        records = [{"document": {
            "id": "dash-1", "name": "Ops", "visibility": "published",
            "created_by": "owner@example.test", "updated_at": "2026-01-01T00:00:00Z",
            "layout": [], "charts": [], "filters": [], "thumbnail": JPEG,
            "thumbnail_updated_at": "2026-02-02T00:00:00Z",
        }}]
        with patch.dict("os.environ", {"KAVEONDB_READ_AUTHORITY_FAMILIES": "dashboards"}, clear=True), \
             patch.object(dashboards.product_store, "list_records",
                          side_effect=lambda kind, *a, **k: records if kind == "dashboard" else []):
            items = dashboards.list_dashboards("owner@example.test", "Admin")
        self.assertIsNone(items[0]["thumbnail"])
        self.assertTrue(items[0]["has_thumbnail"])
        self.assertFalse(items[0]["has_thumbnail_dark"])
        # The refresh pass decides what is outstanding from these, so a list
        # entry has to carry each slot's capture time even though it withholds
        # the image: without them the pass can only see what is missing.
        self.assertEqual(items[0]["thumbnail_updated_at"], "2026-02-02T00:00:00Z")
        self.assertIsNone(items[0]["thumbnail_dark_updated_at"])

    def test_one_capture_is_served_for_both_themes(self):
        document = {
            "id": "dash-1", "name": "Ops", "visibility": "published",
            "created_by": "owner@example.test", "layout": [], "charts": [],
            "filters": [], "thumbnail": JPEG,
        }
        with patch.dict("os.environ", {"KAVEONDB_READ_AUTHORITY_FAMILIES": "dashboards"}, clear=True), \
             patch.object(dashboards.product_store, "read",
                          side_effect=lambda kind, *a, **k: {"document": dict(document), "revision": 1}):
            light = dashboards.get_dashboard_thumbnail("dash-1", "owner@example.test", "Admin", False)
            dark = dashboards.get_dashboard_thumbnail("dash-1", "owner@example.test", "Admin", True)
        self.assertEqual(light, JPEG)
        self.assertEqual(dark, JPEG)


class ThumbnailEndpointTests(unittest.TestCase):
    """The Library card points an <img> at these, so they must answer with image
    bytes, a cacheable validator, and a 404 that lets the card fall back."""

    def setUp(self):
        from fastapi import FastAPI
        from fastapi.testclient import TestClient
        from middleware.auth import UserContext, get_user_context
        from routers import charts as charts_router, dashboards as dashboards_router

        self.app = FastAPI()
        self.app.include_router(charts_router.router)
        self.app.include_router(dashboards_router.router)
        self.app.dependency_overrides[get_user_context] =             lambda: UserContext("owner@example.test", "Analyst")
        self.client = TestClient(self.app)
        self.charts_router = charts_router
        self.dashboards_router = dashboards_router

    def tearDown(self):
        self.client.close()

    def test_a_chart_preview_is_served_as_a_cacheable_image(self):
        with patch.object(self.charts_router.svc, "get_chart_thumbnail", return_value=JPEG):
            response = self.client.get("/charts/chart-1/thumbnail")
        self.assertEqual(response.status_code, 200, response.text)
        self.assertEqual(response.headers["content-type"], "image/jpeg")
        self.assertTrue(response.headers["etag"])
        self.assertIn("max-age", response.headers["cache-control"])
        self.assertEqual(response.content, thumbnails.decode(JPEG)[0])

    def test_a_chart_without_a_preview_answers_not_found(self):
        with patch.object(self.charts_router.svc, "get_chart_thumbnail", return_value=None):
            self.assertEqual(self.client.get("/charts/chart-1/thumbnail").status_code, 404)

    def test_the_point_read_route_is_not_shadowed_by_the_preview_route(self):
        with patch.object(self.charts_router.svc, "get_chart_by_id", return_value={"id": "chart-1"}):
            self.assertEqual(self.client.get("/charts/chart-1").json()["id"], "chart-1")

    def test_a_dashboard_preview_is_requested_for_the_viewer_theme(self):
        with patch.object(self.dashboards_router.svc, "get_dashboard_thumbnail",
                          return_value=PNG) as served:
            response = self.client.get("/dashboards/dash-1/thumbnail?theme=dark")
        self.assertEqual(response.status_code, 200, response.text)
        self.assertEqual(response.headers["content-type"], "image/png")
        self.assertIs(served.call_args.args[3], True)

    def test_a_dashboard_without_a_preview_answers_not_found(self):
        with patch.object(self.dashboards_router.svc, "get_dashboard_thumbnail", return_value=None):
            self.assertEqual(self.client.get("/dashboards/dash-1/thumbnail").status_code, 404)


if __name__ == "__main__":
    unittest.main()
