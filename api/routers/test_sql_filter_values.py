"""GET /sql/distinct-filter-values — the dropdown's values, end to end.

Over an Engine catalog the statement is a grouped aggregate the Engine's cube
answers, which cannot carry an `IS NOT NULL` (any WHERE puts it back on a
scan). The NULL group and the extra row the statement asks for are therefore
resolved here, so the dropdown still gets at most `limit` selectable values
and never a blank entry."""

import sys
import unittest
from types import SimpleNamespace
from unittest.mock import patch

if "pyodbc" not in sys.modules:
    sys.modules["pyodbc"] = SimpleNamespace(Error=Exception)

from middleware.auth import UserContext
from fastapi import Response
from routers import sql
from services.query_generator import build_distinct_filter_values_query

ANALYST = UserContext("analyst@example.com", "Analyst")

DATASET = {
    "id": "144", "database_name": "OpenSource", "schema_name": "public",
    "table_name": "kaveon_events_enriched", "dimensions": [],
    "columns": [{"table_name": "kaveon_events_enriched", "column_name": "region",
                 "is_dimension": True, "is_metric": False},
                {"table_name": "kaveon_events_enriched", "column_name": "platform",
                 "is_dimension": True, "is_metric": False},
                {"table_name": "kaveon_events_enriched", "column_name": "event_date",
                 "is_dimension": False, "is_metric": False}],
}


def _engine_result(rows):
    return {"id": "query-1", "elapsed_ms": 11,
            "columns": [{"name": "key"}, {"name": "value"},
                        {"name": "__kaveon_filter_rows"}],
            "data": rows}


def call(rows, column="region", limit=100, filters=None):
    captured = {}

    def generate(params):
        captured["params"] = params
        return build_distinct_filter_values_query(params)

    with patch.object(sql.datasets_svc, "get_dataset_by_id", return_value=DATASET), \
         patch.object(sql, "_engine_source_for_catalog",
                      return_value={"id": "s1", "engine_catalog": "OpenSource"}), \
         patch.object(sql, "build_distinct_filter_values_query", side_effect=generate), \
         patch.object(sql, "_execute_engine_read_only", return_value=_engine_result(rows)), \
         patch.object(sql.history_svc, "create_history", return_value=None):
        result = sql.distinct_filter_values(
            Response(), "144", column, fact_key=None, limit=limit, source=None,
            chart_id=None, dashboard_id=None, filters=filters, ctx=ANALYST,
        )
    return result, captured["params"]


class EngineCubeFilterValuesTests(unittest.TestCase):
    def test_the_statement_sent_to_the_engine_is_the_cube_shape(self):
        _, params = call([["Europe", "Europe", 1]])
        self.assertTrue(params["engine_source"])
        self.assertEqual(params["db_type"], "kaveon")
        generated = build_distinct_filter_values_query(params)["sql"]
        self.assertIn("GROUP BY region", generated)
        self.assertNotIn("WHERE", generated)

    def test_a_null_group_is_dropped_from_the_values(self):
        result, _ = call([[None, None, 9], ["Asia", "Asia", 2], ["Europe", "Europe", 1]])
        self.assertEqual(result["values"],
                         [{"key": "Asia", "value": "Asia"},
                          {"key": "Europe", "value": "Europe"}])

    def test_the_extra_row_the_statement_asks_for_is_trimmed_to_the_limit(self):
        rows = [[f"r{i}", f"r{i}", 1] for i in range(4)]
        result, _ = call(rows, limit=3)
        self.assertEqual([v["value"] for v in result["values"]], ["r0", "r1", "r2"])

    def test_a_null_group_does_not_cost_the_caller_a_value(self):
        rows = [[None, None, 9]] + [[f"r{i}", f"r{i}", 1] for i in range(3)]
        result, _ = call(rows, limit=3)
        self.assertEqual([v["value"] for v in result["values"]], ["r0", "r1", "r2"])

    def test_the_reported_tier_and_key_column_are_unchanged(self):
        result, _ = call([["Europe", "Europe", 1]])
        self.assertIsNone(result["keyColumn"])
        self.assertEqual(result["filteringTier"], 3)
        self.assertTrue(result["success"])

    def test_a_column_with_no_declared_dimension_still_scans(self):
        _, params = call([["2026-01-01", "2026-01-01", 1]], column="event_date")
        generated = build_distinct_filter_values_query(params)["sql"]
        self.assertIn("SELECT DISTINCT", generated)
        self.assertIn("event_date IS NOT NULL", generated)

    def test_a_cascading_filter_is_passed_through_and_still_scans(self):
        _, params = call([["Europe", "Europe", 1]],
                         filters='[{"column": "platform", "value": "Web"}]')
        self.assertEqual(params["filters"], [{"column": "platform", "value": "Web"}])
        generated = build_distinct_filter_values_query(params)["sql"]
        self.assertIn("SELECT DISTINCT", generated)
        self.assertIn("platform = 'Web'", generated)

    def test_rows_already_free_of_nulls_are_returned_whole(self):
        """Every non-Engine path excludes nulls and limits in SQL, so the
        trimming here must be a no-op for them."""
        rows = [["Asia", "Asia", 1], ["Europe", "Europe", 2]]
        result, _ = call(rows)
        self.assertEqual(len(result["values"]), 2)


if __name__ == "__main__":
    unittest.main()
