"""A map result must be bounded, and the bound must keep the largest regions.

Before this, the aggregate path took a row cap only from the chart's own
`row_limit`, which a map rarely sets -- so a map grouped by a high-cardinality
geography column (a free-text place name, a mis-picked column) produced an
unbounded grouped result that crossed the wire in full. A choropleth can draw
at most one region per map feature, and the largest map Kaveon ships has 215,
so those rows were transferred and parsed to be collapsed onto regions that
already had a value.

The ordering matters as much as the cap. The fallback ordered by the grouped
dimension, so capping an unordered-by-metric map would have dropped regions
alphabetically -- Zimbabwe every time, however large its value. These tests pin
that a capped map is ranked by its metric instead, and that nothing about a
non-map chart changed.
"""

import unittest

from services import query_generator


def _params(**over):
    params = {
        "datasource": "public.sales",
        "groupby": ["country"],
        "metrics": [{"column": "revenue", "aggregate": "SUM", "label": "Revenue"}],
        "db_type": "postgresql",
    }
    params.update(over)
    return params


class GeographicBoundTests(unittest.TestCase):
    def test_a_map_is_capped_even_with_no_row_limit_of_its_own(self):
        sql = query_generator.build_chart_preview_query(_params(chart_type="world_map"))
        self.assertIsNotNone(sql)
        self.assertIn(f"LIMIT {query_generator.MAX_GEOGRAPHIC_ROWS}", sql)

    def test_the_cap_keeps_the_largest_regions_rather_than_the_first(self):
        """An alphabetical cap would drop the same regions on every render."""
        sql = query_generator.build_chart_preview_query(_params(chart_type="world_map"))
        self.assertIn("ORDER BY", sql)
        order = sql[sql.index("ORDER BY"):]
        self.assertIn("Revenue", order,
                      f"a capped map must rank by its metric, got: {order}")
        self.assertIn("DESC", order)
        self.assertNotIn("country", order.lower().replace("revenue", ""),
                         f"the dimension must not be the ranking key, got: {order}")

    def test_a_chart_that_sets_its_own_limit_keeps_it(self):
        """The cap lowers an unbounded ceiling; it does not overrule a choice."""
        sql = query_generator.build_chart_preview_query(
            _params(chart_type="world_map", row_limit=25))
        self.assertIn("LIMIT 25", sql)
        self.assertNotIn(f"LIMIT {query_generator.MAX_GEOGRAPHIC_ROWS}", sql)

    def test_a_chart_that_asks_for_more_than_the_cap_keeps_its_own_limit(self):
        """The generated statement honours the author; the execution path is
        where a ceiling is enforced, because that one also binds saved SQL."""
        sql = query_generator.build_chart_preview_query(
            _params(chart_type="world_map", row_limit=20000))
        self.assertIn("LIMIT 20000", sql)

    def test_an_explicit_sort_is_not_overridden_by_the_cap(self):
        sql = query_generator.build_chart_preview_query(_params(
            chart_type="world_map",
            sort_by={"column": "country", "direction": "asc"},
        ))
        order = sql[sql.index("ORDER BY"):]
        self.assertIn("country", order.lower())
        self.assertIn("ASC", order)

    def test_a_non_map_chart_is_unchanged(self):
        """No cap and no metric ranking appear on a chart that did not ask."""
        sql = query_generator.build_chart_preview_query(_params(chart_type="bar_chart"))
        self.assertNotIn("LIMIT", sql)
        order = sql[sql.index("ORDER BY"):]
        self.assertIn("country", order.lower(),
                      "a bar chart still orders by its dimension")

    def test_a_chart_with_no_type_at_all_is_unchanged(self):
        """Callers that predate `chart_type` must not acquire a cap."""
        sql = query_generator.build_chart_preview_query(_params())
        self.assertNotIn("LIMIT", sql)

    def test_the_cap_applies_on_a_t_sql_source_as_TOP(self):
        sql = query_generator.build_chart_preview_query(
            _params(chart_type="world_map", db_type="fabric_sql"))
        self.assertIn(f"TOP {query_generator.MAX_GEOGRAPHIC_ROWS}", sql)
        self.assertNotIn("LIMIT", sql)

    def test_every_spelling_of_a_map_is_capped(self):
        for chart_type in sorted(query_generator.GEOGRAPHIC_CHART_TYPES):
            sql = query_generator.build_chart_preview_query(_params(chart_type=chart_type))
            self.assertIn(f"LIMIT {query_generator.MAX_GEOGRAPHIC_ROWS}", sql,
                          f"{chart_type} must be capped")

    def test_a_capped_map_with_no_metric_still_has_a_bound(self):
        """A map with no metric cannot be ranked, but must still be bounded --
        an unbounded result is the thing being prevented."""
        sql = query_generator.build_chart_preview_query(
            _params(chart_type="world_map", metrics=[]))
        self.assertIn(f"LIMIT {query_generator.MAX_GEOGRAPHIC_ROWS}", sql)

    def test_the_cap_cannot_truncate_a_map_that_was_drawable(self):
        """The ceiling has to stay above the largest bundled map, or it would
        be cutting regions that could have been drawn."""
        self.assertGreater(query_generator.MAX_GEOGRAPHIC_ROWS, 215,
                           "world.json has 215 features")


if __name__ == "__main__":
    unittest.main()
