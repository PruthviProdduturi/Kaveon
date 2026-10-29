"""A map result is bounded by its map, and a cached result still says it was cut.

Two things are pinned here.

The generic ceiling (MAX_RESULT_ROWS, 5,000) is what a table can scroll, not
what a map can draw. The largest map Kaveon ships has 215 features, so up to
4,785 of those rows were transferred and parsed only to be collapsed onto
regions that already had a value. A map now gets a map-sized ceiling, and that
ceiling only ever lowers a bound -- a chart that asked for fewer rows keeps its
own number.

And `truncated` used to be dropped on the way into the cache and on the way
out, so a dashboard tile -- the only surface that reads the cache, and the one
most likely to be showing a cut result -- was the one surface that never said
so.
"""

import sys
import unittest
from types import SimpleNamespace
from unittest.mock import patch

if "pyodbc" not in sys.modules:
    sys.modules["pyodbc"] = SimpleNamespace(Error=Exception)

from fastapi import Response

from middleware.auth import UserContext
from models.sql import SqlExecuteBody
from routers import sql


class GeographicExecutionBoundTests(unittest.TestCase):
    def setUp(self):
        sql._QUERY_CACHE.clear()
        self.ctx = UserContext("analyst@example.com", "Analyst")

    def _run(self, body, result):
        """Call the route with everything outside it held still."""
        # Not an Engine catalog, so the statement goes through the pool --
        # the path these ceilings apply to. There is no retirement flag now.
        with patch.object(sql, "_engine_source_for_catalog", return_value=None), \
             patch.object(sql, "assert_no_platform_tables"), \
             patch.object(sql.sql_execute_limiter, "check"), \
             patch.object(sql.history_svc, "create_history"), \
             patch.object(sql.pool, "execute_query", return_value=result) as execute:
            answer = sql.execute_sql(body, Response(), self.ctx)
        return answer, execute

    def _body(self, **over):
        fields = {
            "sql_text": "SELECT country, SUM(revenue) AS revenue FROM sales GROUP BY country",
            "database": "warehouse",
            "source": "dashboard-chart",
        }
        fields.update(over)
        return SqlExecuteBody(**fields)

    def test_a_map_asks_the_source_for_a_map_sized_result(self):
        _, execute = self._run(
            self._body(chart_type="world_map"),
            {"columns": ["country", "revenue"], "rows": [], "row_count": 0},
        )
        self.assertEqual(execute.call_args.kwargs["max_rows"], sql.MAX_GEOGRAPHIC_ROWS)

    def test_a_table_still_asks_for_the_generic_ceiling(self):
        _, execute = self._run(
            self._body(chart_type="table"),
            {"columns": ["country"], "rows": [], "row_count": 0},
        )
        self.assertEqual(execute.call_args.kwargs["max_rows"], sql.MAX_RESULT_ROWS)

    def test_a_statement_with_no_chart_type_is_unchanged(self):
        """The Lab and every other caller keep the ceiling they had."""
        _, execute = self._run(
            self._body(),
            {"columns": ["country"], "rows": [], "row_count": 0},
        )
        self.assertEqual(execute.call_args.kwargs["max_rows"], sql.MAX_RESULT_ROWS)

    def test_the_map_ceiling_only_ever_lowers_a_bound(self):
        """A chart that wants 10 rows gets 10, not 1,000."""
        _, execute = self._run(
            self._body(chart_type="world_map", row_limit=10),
            {"columns": ["country"], "rows": [], "row_count": 0},
        )
        self.assertEqual(execute.call_args.kwargs["max_rows"], 10)

    def test_a_chart_cannot_raise_the_map_ceiling(self):
        # 5,000 is the most the request model accepts -- it validates row_limit
        # at MAX_RESULT_ROWS -- and it is still five times the map ceiling, so
        # it is the real test of which bound wins.
        _, execute = self._run(
            self._body(chart_type="world_map", row_limit=sql.MAX_RESULT_ROWS),
            {"columns": ["country"], "rows": [], "row_count": 0},
        )
        self.assertEqual(execute.call_args.kwargs["max_rows"], sql.MAX_GEOGRAPHIC_ROWS)

    def test_the_map_ceiling_cannot_truncate_a_map_that_was_drawable(self):
        self.assertGreater(sql.MAX_GEOGRAPHIC_ROWS, 215, "world.json has 215 features")
        self.assertLess(sql.MAX_GEOGRAPHIC_ROWS, sql.MAX_RESULT_ROWS)

    def test_every_spelling_of_a_map_gets_the_map_ceiling(self):
        for chart_type in sorted(sql.GEOGRAPHIC_CHART_TYPES):
            _, execute = self._run(
                self._body(chart_type=chart_type),
                {"columns": ["country"], "rows": [], "row_count": 0},
            )
            self.assertEqual(execute.call_args.kwargs["max_rows"], sql.MAX_GEOGRAPHIC_ROWS,
                             f"{chart_type} must get the map ceiling")

    def test_a_truncated_result_says_so_and_still_says_so_from_the_cache(self):
        body = self._body(chart_type="world_map", use_cache=True)
        live, _ = self._run(body, {
            "columns": ["country", "revenue"],
            "rows": [["Kenya", 1]],
            "row_count": 1,
            "truncated": True,
        })
        self.assertTrue(live["truncated"])
        self.assertFalse(live["from_cache"])
        self.assertIn("first", live["message"])

        # The second call must not reach the source at all, and must carry the
        # same truncation fact the first one did.
        cached, execute = self._run(body, {"columns": [], "rows": [], "row_count": 0})
        execute.assert_not_called()
        self.assertTrue(cached["from_cache"])
        self.assertTrue(cached["truncated"],
                        "a cached result dropped the truncation flag entirely")
        self.assertIn("first", cached["message"])
        self.assertIn("cached", cached["message"])

    def test_an_untruncated_cached_result_does_not_claim_truncation(self):
        body = self._body(use_cache=True)
        self._run(body, {
            "columns": ["country"], "rows": [["Kenya"]], "row_count": 1, "truncated": False,
        })
        cached, execute = self._run(body, {"columns": [], "rows": [], "row_count": 0})
        execute.assert_not_called()
        self.assertFalse(cached["truncated"])
        self.assertIn("Returned", cached["message"])


class EngineRouteGeographicBoundTests(unittest.TestCase):
    """The two execution routes must agree about how large a map result is.

    A KaveonDB catalog goes through `execute_engine_sql`, not `execute_sql`.
    It already had the generic 5,000-row ceiling; without the map ceiling here
    too, the same chart would be bounded differently depending only on which
    kind of source it was pointed at.
    """

    def setUp(self):
        sql._QUERY_CACHE.clear()
        self.ctx = UserContext("analyst@example.com", "Analyst")

    def _run(self, chart_type, rows, row_limit=None):
        body = SqlExecuteBody(
            sql_text="SELECT country, SUM(revenue) AS revenue FROM sales GROUP BY country",
            database="OpenSource", dataset_id=7, source="dashboard-chart",
            chart_type=chart_type, row_limit=row_limit,
        )
        source = {"id": "src-1", "engine_catalog": "OpenSource"}
        with patch.object(sql, "_assert_engine_execute_permission"),              patch.object(sql, "assert_read_only"),              patch.object(sql, "assert_no_platform_tables"),              patch.object(sql.sql_execute_limiter, "check"),              patch.object(sql, "_engine_source_for_catalog", return_value=source),              patch.object(sql.datasets_svc, "get_dataset_by_id",
                          return_value={"database_name": "OpenSource", "schema_name": "public"}),              patch.object(sql.history_svc, "create_history"),              patch.object(sql, "_execute_engine_chart_statement",
                          return_value=({"id": "q1", "elapsed_ms": 5}, (["country"], rows))):
            return sql.execute_engine_sql(body, Response(), self.ctx)

    def test_a_map_over_an_engine_catalog_gets_the_map_ceiling(self):
        rows = [[f"c{i}"] for i in range(sql.MAX_GEOGRAPHIC_ROWS + 50)]
        answer = self._run("world_map", rows)
        self.assertEqual(len(answer["rows"]), sql.MAX_GEOGRAPHIC_ROWS)
        self.assertTrue(answer["truncated"])

    def test_a_table_over_an_engine_catalog_keeps_the_generic_ceiling(self):
        rows = [[f"c{i}"] for i in range(sql.MAX_GEOGRAPHIC_ROWS + 50)]
        answer = self._run("table", rows)
        self.assertEqual(len(answer["rows"]), sql.MAX_GEOGRAPHIC_ROWS + 50)
        self.assertFalse(answer["truncated"])

    def test_a_map_result_inside_the_ceiling_is_not_called_truncated(self):
        answer = self._run("world_map", [["Kenya"], ["Chad"]])
        self.assertEqual(len(answer["rows"]), 2)
        self.assertFalse(answer["truncated"])


if __name__ == "__main__":
    unittest.main()
