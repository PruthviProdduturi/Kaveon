"""A chart or dashboard delete must clear its references first.

The product catalog refuses to delete a record that another record still
references, so a recents entry or a favorite left in place makes the delete
fail outright:

    invalid transaction: product record 'user_recent/<id>'
    references missing 'chart/<id>'

Both routers used to purge after the delete, inside a try/except that only
printed, so the purge never ran and nothing recorded why: every chart or
dashboard anyone had ever opened or favorited was undeletable, and the Studio
reported it as a permission problem. These tests pin the order and the
reporting rather than the implementation.
"""

import sys
import unittest
from types import SimpleNamespace
from unittest.mock import patch

if "pyodbc" not in sys.modules:
    sys.modules["pyodbc"] = SimpleNamespace(Error=Exception)

from fastapi import HTTPException

from routers import charts as charts_router
from routers import dashboards as dashboards_router
from services import datasets, favorites


CTX = SimpleNamespace(email="owner@example.com", role="Admin")


class DeleteReferenceOrderTests(unittest.TestCase):
    def _order_for(self, module, identifier, existing_getter, delete_name, call):
        """Record the order of the three steps a delete performs."""
        order = []
        with patch.object(module.svc, existing_getter,
                          return_value={"created_by": CTX.email}), \
             patch.object(module.recents_svc, "remove_recent_all_users",
                          side_effect=lambda *a, **k: order.append("recents")), \
             patch.object(module.fav_svc, "remove_favorite_all_users",
                          side_effect=lambda *a, **k: order.append("favorites")), \
             patch.object(module.svc, delete_name,
                          side_effect=lambda *a, **k: order.append("delete")), \
             patch.object(module, "can_write", return_value=True):
            call(identifier)
        return order

    def test_chart_delete_clears_references_before_the_record(self):
        order = self._order_for(
            charts_router, "chart-1", "get_chart_by_id", "delete_chart",
            lambda i: charts_router.delete_chart(i, ctx=CTX))
        self.assertEqual(order, ["recents", "favorites", "delete"])

    def test_dashboard_delete_clears_references_before_the_record(self):
        order = self._order_for(
            dashboards_router, "dash-1", "get_dashboard_by_id", "delete_dashboard",
            lambda i: dashboards_router.delete_dashboard(i, ctx=CTX))
        self.assertEqual(order, ["recents", "favorites", "delete"])

    def test_a_failed_purge_reports_itself_and_does_not_delete(self):
        """Proceeding would only produce the catalog's rejection one line later,
        and the old code's `print` left no way to know that had happened."""
        deleted = []
        with patch.object(charts_router.svc, "get_chart_by_id",
                          return_value={"created_by": CTX.email}), \
             patch.object(charts_router.recents_svc, "remove_recent_all_users",
                          side_effect=RuntimeError("catalog refused")), \
             patch.object(charts_router.svc, "delete_chart",
                          side_effect=lambda *a, **k: deleted.append(1)), \
             patch.object(charts_router, "can_write", return_value=True):
            with self.assertRaises(HTTPException) as caught:
                charts_router.delete_chart("chart-1", ctx=CTX)
        self.assertEqual(caught.exception.status_code, 502)
        self.assertIn("catalog refused", str(caught.exception.detail))
        self.assertEqual(deleted, [], "the record must not be deleted")


class FavoriteFanoutTests(unittest.TestCase):
    """`remove_favorite_all_users` mirrors the recents purge: it is a fanout
    across owners, because a favorite belongs to whoever made it rather than to
    the person deleting the object, and it is bounded for the same reason."""

    def _records(self, count, object_id="chart-1"):
        return [{"id": f"f{i}", "revision": 1,
                 "document": {"user_email": f"u{i}@example.com",
                              "object_type": "chart", "object_id": object_id}}
                for i in range(count)]

    def test_each_owner_is_written_as_itself(self):
        with patch.object(favorites, "product_read_authority", create=True), \
             patch("services.product_read_authority.enabled", return_value=True), \
             patch("services.product_store.list_records",
                   return_value=self._records(3)), \
             patch("services.product_store.transact") as transact:
            favorites.remove_favorite_all_users("chart", "chart-1")
        actors = sorted(call.args[1] for call in transact.call_args_list)
        self.assertEqual(actors, ["u0@example.com", "u1@example.com", "u2@example.com"])

    def test_unrelated_favorites_are_left_alone(self):
        with patch("services.product_read_authority.enabled", return_value=True), \
             patch("services.product_store.list_records",
                   return_value=self._records(2, object_id="chart-other")), \
             patch("services.product_store.transact") as transact:
            favorites.remove_favorite_all_users("chart", "chart-1")
        transact.assert_not_called()

    def test_the_fanout_is_bounded(self):
        with patch("services.product_read_authority.enabled", return_value=True), \
             patch("services.product_store.list_records",
                   return_value=self._records(favorites.MAX_CROSS_OWNER_DELETE + 1)), \
             patch("services.product_store.transact") as transact:
            with self.assertRaisesRegex(RuntimeError, "fanout bound"):
                favorites.remove_favorite_all_users("chart", "chart-1")
        transact.assert_not_called()


class CompiledDlmCascadeTests(unittest.TestCase):
    """A dataset's compiled DLM context is deleted with it, references first.

    A `dlm_definition` references its dataset and a `dlm_run` references that
    definition, so the catalog refused to delete a dataset that had ever been
    compiled. Unlike a chart, these records are derived rather than authored:
    nobody can clear them from the UI, so the delete clears them — in one
    transaction, runs before the definition before the dataset.
    """

    def _runs(self, count, definition_id="71"):
        return [{"id": f"{definition_id}-v{index + 1}", "revision": 2,
                 "document": {"definition_id": definition_id,
                              "definition_revision": 1, "status": "ready",
                              "artifact": {"path": "p", "sha256": "a" * 64}}}
                for index in range(count)]

    def _delete(self, runs, definition):
        reads = {"dataset": {"revision": 8, "document": {"id": "71"}},
                 "dlm_definition": definition}
        with patch("services.product_read_authority.enabled", return_value=True), \
             patch.object(datasets.product_store, "read",
                          side_effect=lambda kind, *a, **k: reads[kind]), \
             patch.object(datasets.product_store, "list_records", return_value=runs), \
             patch.object(datasets.product_store, "transact") as transact, \
             patch.object(datasets.db, "transaction",
                          side_effect=AssertionError("PostgreSQL reached")):
            result = datasets.delete_dataset("71", "owner@example.com")
        return result, transact

    def test_runs_then_definition_then_dataset_in_one_transaction(self):
        result, transact = self._delete(self._runs(2), {"revision": 3})
        self.assertTrue(result)
        transact.assert_called_once()
        staged = [(item.kind, item.record_id, item.expected_revision)
                  for item in transact.call_args.args[0]]
        self.assertEqual(staged, [
            ("dlm_run", "71-v1", 2), ("dlm_run", "71-v2", 2),
            ("dlm_definition", "71", 3), ("dataset", "71", 8),
        ])
        self.assertTrue(all(item.operation == "delete"
                            for item in transact.call_args.args[0]))

    def test_another_dataset_s_runs_are_left_alone(self):
        _, transact = self._delete(self._runs(2, definition_id="99"), None)
        staged = [(item.kind, item.record_id) for item in transact.call_args.args[0]]
        self.assertEqual(staged, [("dataset", "71")])

    def test_the_cascade_is_bounded(self):
        """Past the bound `transact` would refuse the whole group for its size
        and name neither the dataset nor the reason."""
        with self.assertRaisesRegex(RuntimeError, "compiled DLM runs"):
            self._delete(self._runs(datasets.MAX_DLM_RUN_CASCADE + 1), {"revision": 3})

    def test_an_invalid_run_revision_is_not_deleted_around(self):
        broken = self._runs(1)
        broken[0]["revision"] = 0
        with self.assertRaisesRegex(RuntimeError, "DLM run delete state"):
            self._delete(broken, {"revision": 3})


if __name__ == "__main__":
    unittest.main()
