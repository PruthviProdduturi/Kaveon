"""A soft-deleted catalog source must be reachable by the Admin who can erase it.

`DELETE /catalog-sources/{id}` accepts a source whose lifecycle is `draft` or
`deleted`, but no listing returned a `deleted` one -- so the row the route
would erase was one nobody could find the ID of, and its `catalog_identity`
stayed reserved against every new source for as long as the record lived.
"""

import sys
import unittest
from types import SimpleNamespace
from unittest.mock import patch

if "pyodbc" not in sys.modules:
    sys.modules["pyodbc"] = SimpleNamespace(Error=Exception)

from fastapi import HTTPException

from routers import catalog_sources


def source(identifier, lifecycle, kind="catalog"):
    return {"id": f"catalog-{identifier}", "source_kind": kind, "name": identifier,
            "lifecycle": lifecycle, "catalog_identity": identifier,
            "created_at": "2026-10-01T00:00:00Z"}


class TheListingReachesWhatTheDeleteAccepts(unittest.TestCase):
    def _list(self, role="Admin", **kwargs):
        ctx = SimpleNamespace(email="ada@example.com", role=role)
        documents = [source("live", "active"), source("gone", "deleted"),
                     source("new", "draft")]
        with patch("services.product_read_authority.enabled", return_value=True), \
             patch("services.product_read_authority.list_documents",
                   return_value=documents), \
             patch("routers.catalog_sources._catalog_cutover_row", side_effect=lambda d: dict(d)):
            return catalog_sources.list_catalog_sources(ctx, **kwargs)

    def test_a_deleted_source_is_hidden_by_default(self):
        """Everyone else reads this list as "the catalogs I can use"."""
        names = {row["name"] for row in self._list()["catalogSources"]}
        self.assertEqual(names, {"live", "new"})

    def test_an_admin_can_ask_for_the_deleted_source(self):
        names = {row["name"] for row in
                 self._list(include_deleted=True)["catalogSources"]}
        self.assertEqual(names, {"live", "new", "gone"})

    def test_a_non_admin_is_refused_rather_than_quietly_filtered(self):
        """Answering the default listing to a Viewer who asked for deleted rows
        would report absence as a fact about the data."""
        with self.assertRaises(HTTPException) as raised:
            self._list(role="Analyst", include_deleted=True)
        self.assertEqual(raised.exception.status_code, 403)

    def test_a_viewer_still_reads_the_ordinary_listing(self):
        names = {row["name"] for row in self._list(role="Viewer")["catalogSources"]}
        self.assertEqual(names, {"live", "new"})

    def test_a_data_source_is_never_listed_as_a_catalog(self):
        ctx = SimpleNamespace(email="ada@example.com", role="Admin")
        with patch("services.product_read_authority.enabled", return_value=True), \
             patch("services.product_read_authority.list_documents",
                   return_value=[source("warehouse", "deleted", kind="data")]), \
             patch("routers.catalog_sources._catalog_cutover_row", side_effect=lambda d: dict(d)):
            listed = catalog_sources.list_catalog_sources(ctx, include_deleted=True)
        self.assertEqual(listed["catalogSources"], [])


class ThePostgreSQLBranchAsksTheSameQuestion(unittest.TestCase):
    def _query_for(self, **kwargs):
        ctx = SimpleNamespace(email="ada@example.com", role="Admin")
        with patch("services.product_read_authority.enabled", return_value=False), \
             patch("routers.catalog_sources.db.query",
                   return_value={"rows": []}) as query, \
             patch("services.product_shadow_read.observe_source_list",
                   return_value={"enabled": False}):
            catalog_sources.list_catalog_sources(ctx, **kwargs)
        return query.call_args.args[0]

    def test_the_default_statement_excludes_deleted_rows(self):
        self.assertIn("lifecycle != 'deleted'", self._query_for())

    def test_asking_for_deleted_rows_drops_the_predicate(self):
        statement = self._query_for(include_deleted=True)
        self.assertNotIn("lifecycle !=", statement)
        self.assertIn("ORDER BY created_at DESC", statement)


if __name__ == "__main__":
    unittest.main()
