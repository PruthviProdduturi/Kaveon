"""A delete must say what is in the way, and clear what only it can clear.

The product catalog validates typed references at commit and refuses to delete
a record another record still points at. Charts and dashboards already purge
the per-user bookkeeping that names them (api/services/test_delete_reference_order.py
pins that order); these tests cover the deletes that were still handing the
catalog's own rejection to the caller:

  * a dataset delete cleared nothing at all, so any dataset anyone had opened
    or favorited, and any dataset with charts over it, answered with a 502
    quoting `references missing 'dataset/<id>'`;
  * a chart on a dashboard could not be deleted by anyone, for the same
    reason one level up;
  * a data source delete demanded that favorites be removed first — an action
    only their owners can perform, and no route offers.
"""

import sys
import unittest
from types import SimpleNamespace
from unittest.mock import patch

if "pyodbc" not in sys.modules:
    sys.modules["pyodbc"] = SimpleNamespace(Error=Exception)

from fastapi import HTTPException

from routers import catalog_sources
from routers import charts as charts_router
from routers import data_sources as data_sources_router
from routers import datasets as datasets_router
from routers import lab
from routers import reference_guard


CTX = SimpleNamespace(email="owner@example.com", role="Admin")


def _records(documents):
    return [{"id": str(document.get("id")), "revision": 1, "document": document}
            for document in documents]


class DatasetDeleteTests(unittest.TestCase):
    def _delete(self, dependents=(0, []), purge_error=None):
        order = []
        with patch.object(datasets_router.svc, "get_dataset_by_id",
                          return_value={"created_by": CTX.email}), \
             patch.object(datasets_router.reference_guard, "dependents",
                          return_value=dependents), \
             patch.object(datasets_router.recents_svc, "remove_recent_all_users",
                          side_effect=(purge_error or (lambda *a, **k: order.append("recents")))), \
             patch.object(datasets_router.fav_svc, "remove_favorite_all_users",
                          side_effect=lambda *a, **k: order.append("favorites")), \
             patch.object(datasets_router.svc, "delete_dataset",
                          side_effect=lambda *a, **k: order.append("delete")), \
             patch.object(datasets_router, "can_write", return_value=True):
            datasets_router.delete_dataset("9", ctx=CTX)
        return order

    def test_references_are_cleared_before_the_record(self):
        self.assertEqual(self._delete(), ["recents", "favorites", "delete"])

    def test_the_recents_purge_names_the_prefixed_item_id(self):
        """A recents entry stores `dataset-<id>`; purging the bare id clears
        nothing and leaves the reference the catalog refuses."""
        with patch.object(datasets_router.svc, "get_dataset_by_id",
                          return_value={"created_by": CTX.email}), \
             patch.object(datasets_router.reference_guard, "dependents", return_value=(0, [])), \
             patch.object(datasets_router.recents_svc, "remove_recent_all_users") as recents, \
             patch.object(datasets_router.fav_svc, "remove_favorite_all_users") as favorites, \
             patch.object(datasets_router.svc, "delete_dataset"), \
             patch.object(datasets_router, "can_write", return_value=True):
            datasets_router.delete_dataset("9", ctx=CTX)
        recents.assert_called_once_with("dataset-9", "dataset")
        favorites.assert_called_once_with("dataset", "9")

    def test_dependent_charts_are_refused_with_409_and_named(self):
        with self.assertRaises(HTTPException) as caught:
            self._delete(dependents=(2, ["Trips by borough"]))
        self.assertEqual(caught.exception.status_code, 409)
        self.assertIn("Trips by borough", caught.exception.detail)
        self.assertIn("Delete those charts", caught.exception.detail)

    def test_a_failed_purge_reports_itself_and_does_not_delete(self):
        with self.assertRaises(HTTPException) as caught:
            self._delete(purge_error=RuntimeError("catalog refused"))
        self.assertEqual(caught.exception.status_code, 502)
        self.assertIn("catalog refused", str(caught.exception.detail))


class ChartOnDashboardTests(unittest.TestCase):
    def test_a_chart_on_a_dashboard_is_refused_with_409_not_the_catalog_error(self):
        with patch.object(charts_router.svc, "get_chart_by_id",
                          return_value={"created_by": CTX.email}), \
             patch.object(charts_router.reference_guard, "dependents",
                          return_value=(1, ["Fleet overview"])), \
             patch.object(charts_router.svc, "delete_chart") as delete, \
             patch.object(charts_router, "can_write", return_value=True):
            with self.assertRaises(HTTPException) as caught:
                charts_router.delete_chart("chart-1", ctx=CTX)
        self.assertEqual(caught.exception.status_code, 409)
        self.assertIn("Fleet overview", caught.exception.detail)
        delete.assert_not_called()

    def test_chart_references_come_from_the_field_the_engine_reads(self):
        """`chart_revisions` is what the Engine derives the dashboard's
        references from; `charts` is read too so an older document still
        accounts for its tiles."""
        self.assertEqual(
            charts_router._chart_references({"chart_revisions": {"c1": 3, "c2": 1}}),
            {"c1", "c2"})
        self.assertEqual(
            charts_router._chart_references({"charts": ["c7", 8]}), {"c7", "8"})
        self.assertEqual(charts_router._chart_references({}), set())


class DataSourceFavoriteTests(unittest.TestCase):
    def test_another_users_favorite_is_cleared_rather_than_demanded(self):
        cleared = []
        with patch.object(data_sources_router.product_read_authority, "read_document",
                          return_value={"secret_ref": "kv://ref"}), \
             patch.object(data_sources_router.source_cutover_mutations, "enabled",
                          return_value=True), \
             patch.object(data_sources_router.source_cutover_mutations, "delete") as delete, \
             patch("services.favorites.remove_favorite_all_users",
                   side_effect=lambda *args: cleared.append(args)), \
             patch.object(data_sources_router.source_secret_store, "SourceSecretStore"):
            result = data_sources_router.delete_data_source("7", CTX)
        self.assertTrue(result["success"])
        self.assertEqual(cleared, [("source", "data-7")])
        delete.assert_called_once_with("data-7", CTX.email)

    def test_a_failed_purge_reports_itself_and_does_not_delete(self):
        with patch.object(data_sources_router.product_read_authority, "read_document",
                          return_value={"secret_ref": "kv://ref"}), \
             patch.object(data_sources_router.source_cutover_mutations, "enabled",
                          return_value=True), \
             patch.object(data_sources_router.source_cutover_mutations, "delete") as delete, \
             patch("services.favorites.remove_favorite_all_users",
                   side_effect=RuntimeError("fanout bound")):
            with self.assertRaises(HTTPException) as caught:
                data_sources_router.delete_data_source("7", CTX)
        self.assertEqual(caught.exception.status_code, 502)
        self.assertIn("fanout bound", caught.exception.detail)
        delete.assert_not_called()


class SavedQueryAndCatalogSourceTests(unittest.TestCase):
    """A favorite can name a saved query or a catalog source too, and the
    Library writes saved-query favorites, so both deletes need the same purge."""

    def test_saved_query_favorites_are_cleared_before_the_record(self):
        order = []
        with patch("services.favorites.remove_favorite_all_users",
                   side_effect=lambda *args: order.append(args)), \
             patch.object(lab.saved_q_svc, "delete_saved_query",
                          side_effect=lambda *a, **k: order.append("delete") or True):
            lab.delete_saved_query("q1", CTX.email)
        self.assertEqual(order, [("saved_query", "q1"), "delete"])

    def test_a_failed_saved_query_purge_does_not_delete(self):
        with patch("services.favorites.remove_favorite_all_users",
                   side_effect=RuntimeError("catalog refused")), \
             patch.object(lab.saved_q_svc, "delete_saved_query") as delete:
            with self.assertRaises(HTTPException) as caught:
                lab.delete_saved_query("q1", CTX.email)
        self.assertEqual(caught.exception.status_code, 502)
        delete.assert_not_called()

    def test_catalog_source_favorites_are_cleared_before_the_record(self):
        order = []
        with patch.object(catalog_sources.source_cutover_mutations, "enabled",
                          return_value=True), \
             patch.object(catalog_sources.product_read_authority, "read_document",
                          return_value={"lifecycle": "draft", "catalog_identity": "Lake"}), \
             patch.object(catalog_sources.product_read_authority, "list_documents",
                          return_value=[]), \
             patch("services.favorites.remove_favorite_all_users",
                   side_effect=lambda *args: order.append(args)), \
             patch.object(catalog_sources.source_cutover_mutations, "delete",
                          side_effect=lambda *a, **k: order.append("delete")):
            catalog_sources.delete_catalog_source("c7", CTX)
        self.assertEqual(order, [("source", "catalog-c7"), "delete"])


class ReferenceGuardTests(unittest.TestCase):
    """The guard counts dependents at system scope and names only the visible
    ones, so a dependent the caller cannot see blocks the delete without being
    disclosed."""

    def _dependents(self, system, visible):
        def listed(family, actor, role, **kwargs):
            return system if actor == "kaveon-system" else visible
        with patch.object(reference_guard.product_read_authority, "enabled",
                          return_value=True), \
             patch.object(reference_guard.product_read_authority, "list_documents",
                          side_effect=listed):
            return reference_guard.dependents(
                "dashboards", lambda document: document.get("uses") is True,
                actor=CTX.email, role="Analyst")

    def test_an_invisible_dependent_is_counted_but_not_named(self):
        total, names = self._dependents(
            system=[{"id": "d1", "name": "Private board", "uses": True}], visible=[])
        self.assertEqual((total, names), (1, []))
        self.assertIn("not visible to you", reference_guard.message(
            total, names, "dashboard", "Remove it."))

    def test_visible_dependents_are_named_and_the_rest_counted(self):
        system = [{"id": f"d{i}", "name": f"Board {i}", "uses": True} for i in range(8)]
        total, names = self._dependents(system=system, visible=system[:2])
        self.assertEqual(total, 8)
        self.assertEqual(names, ["Board 0", "Board 1"])
        self.assertIn("and 6 more", reference_guard.message(
            total, names, "dashboard", "Remove it."))

    def test_naming_is_capped(self):
        system = [{"id": f"d{i}", "name": f"Board {i}", "uses": True} for i in range(20)]
        total, names = self._dependents(system=system, visible=system)
        self.assertEqual(total, 20)
        self.assertEqual(len(names), reference_guard.MAX_NAMED)

    def test_the_legacy_path_has_no_catalog_reference_to_anticipate(self):
        """Off KaveonDB read authority a dependent row is governed by its own
        foreign key, so the guard asks nothing of the control plane."""
        with patch.object(reference_guard.product_read_authority, "enabled",
                          return_value=False), \
             patch.object(reference_guard.product_read_authority, "list_documents",
                          side_effect=AssertionError("listed on the legacy path")):
            self.assertEqual(
                reference_guard.dependents("dashboards", lambda document: True,
                                           actor=CTX.email, role="Admin"),
                (0, []))

    def test_the_scan_states_its_bound(self):
        """The product store's listing ceiling is asked for rather than
        discovered by exceeding it."""
        seen = []

        def listed(family, actor, role, **kwargs):
            seen.append(kwargs.get("max_records"))
            return []

        with patch.object(reference_guard.product_read_authority, "enabled",
                          return_value=True), \
             patch.object(reference_guard.product_read_authority, "list_documents",
                          side_effect=listed):
            reference_guard.dependents("charts", lambda document: True,
                                       actor=CTX.email, role="Admin")
        self.assertEqual(seen, [reference_guard.MAX_DEPENDENT_SCAN])


if __name__ == "__main__":
    unittest.main()
