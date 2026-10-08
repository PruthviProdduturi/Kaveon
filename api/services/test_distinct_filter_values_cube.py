"""Filter-dropdown distinct values over an Engine catalog.

The Engine's cube over a declared table holds every declared dimension's
values as cells, but it answers a grouped aggregate, not a `SELECT DISTINCT`.
A dashboard filter dropdown over `public.kaveon_events_enriched`
(504,600,000 rows) therefore scanned. These tests pin the exact shape the
cube answers — and, just as importantly, every case that must keep the
scanning statement rather than guess.

Shapes measured against the live Engine, one run each as a shape check and
not as a benchmark claim: the `COUNT(*)` grouped form answered from the cube
(`execution.mode` "context") in well under a second, while the same statement
with no aggregate, with `WHERE <col> IS NOT NULL`, with a cascading `WHERE`,
over a column the table declares no dimension for, or grouping two
dimensions, each came back "distributed" and took tens of seconds."""

import unittest

from services.query_generator import build_distinct_filter_values_query

FACT = "public.kaveon_events_enriched"
DIMENSIONS = ("surface", "platform", "license", "segment", "industry",
              "region", "country", "deployment", "acquisition_channel", "team_size")
COLUMNS = (
    [{"table_name": "kaveon_events_enriched", "column_name": name,
      "is_dimension": True, "is_metric": False} for name in DIMENSIONS]
    + [{"table_name": "kaveon_events_enriched", "column_name": "event_date",
        "is_dimension": False, "is_metric": False},
       {"table_name": "kaveon_events_enriched", "column_name": "user_id",
        "is_dimension": False, "is_metric": False},
       {"table_name": "kaveon_events_enriched", "column_name": "actions",
        "is_dimension": False, "is_metric": True}]
)


def generate(**overrides):
    params = {"datasource": FACT, "column": "region", "columns": COLUMNS,
              "dimensions": [], "limit": 100, "filters": [],
              "db_type": "kaveon", "engine_source": True}
    params.update(overrides)
    return build_distinct_filter_values_query(params)


class EngineCubeShapeTests(unittest.TestCase):
    def test_a_declared_dimension_is_grouped_rather_than_distinct(self):
        result = generate()
        self.assertEqual(
            result["sql"],
            'SELECT region AS key, region AS value, COUNT(*) AS "__kaveon_filter_rows" '
            "FROM public.kaveon_events_enriched GROUP BY region"
        )

    def test_the_aggregate_is_present_because_a_bare_group_by_scans(self):
        self.assertIn("COUNT(*)", generate()["sql"])

    def test_no_where_clause_is_emitted_because_any_where_scans(self):
        self.assertNotIn("WHERE", generate()["sql"])

    def test_one_dimension_only_is_grouped(self):
        sql = generate()["sql"]
        self.assertEqual(sql.count("GROUP BY"), 1)
        self.assertTrue(sql.split("GROUP BY")[1].strip() == "region", sql)

    def test_ordering_is_by_the_display_value_as_before(self):
        # Deliberately absent: an ORDER BY disqualifies the cube match, and
        # this statement exists only to reach the cube. The caller orders the
        # handful of values it gets back.
        self.assertNotIn("ORDER BY", generate()["sql"])

    def test_one_row_over_the_limit_is_requested_to_absorb_a_null_group(self):
        """The cube path cannot exclude NULLs with a WHERE, so the route drops
        a NULL group from the result. Asking for one extra row keeps a full
        page of selectable values after that drop."""
        # A LIMIT disqualifies the cube too, so the statement carries none
        # and the caller trims. Asking for more rows cannot change the SQL.
        self.assertNotIn("LIMIT", generate(limit=25)["sql"])
        self.assertEqual(generate(limit=25)["sql"], generate(limit=500)["sql"])

    def test_every_declared_dimension_takes_the_cube_path(self):
        for column in DIMENSIONS:
            sql = generate(column=column)["sql"]
            self.assertIn(f"GROUP BY {column}", sql, column)

    def test_a_fully_qualified_column_name_still_resolves_to_the_cube(self):
        self.assertIn("GROUP BY region", generate(column=f"{FACT}.region")["sql"])

    def test_the_reported_tier_and_key_column_are_unchanged(self):
        """A fact-table column is always tier 3 with no key column, which is
        what the scanning statement reported for the same case."""
        cube = generate()
        self.assertEqual((cube["keyColumn"], cube["filteringTier"]), (None, 3))


class EngineCubeFallbackTests(unittest.TestCase):
    """Anything the cube is not known to answer keeps the existing statement."""

    def _is_scan(self, result, column):
        self.assertIsNotNone(result)
        self.assertNotIn("GROUP BY", result["sql"])
        self.assertIn("SELECT DISTINCT", result["sql"])
        self.assertIn(f"{column} IS NOT NULL", result["sql"])

    def test_a_column_the_dataset_declares_no_dimension_for_keeps_the_scan(self):
        for column in ("event_date", "user_id"):
            self._is_scan(generate(column=column), column)

    def test_a_measure_column_keeps_the_scan(self):
        self._is_scan(generate(column="actions"), "actions")

    def test_a_column_absent_from_the_dataset_keeps_the_scan(self):
        self._is_scan(generate(column="not_a_column"), "not_a_column")

    def test_a_dataset_with_no_column_metadata_keeps_the_scan(self):
        self._is_scan(generate(columns=[]), "region")

    def test_a_cascading_filter_keeps_the_scan(self):
        """The cascading narrow needs a WHERE, and any WHERE takes the
        statement off the cube — so the narrowed values are still correct,
        just read by scanning."""
        result = generate(filters=[{"column": "platform", "value": "Web"}])
        self.assertNotIn("GROUP BY", result["sql"])
        self.assertIn("platform = 'Web'", result["sql"])

    def test_a_self_filter_on_the_target_column_is_still_ignored(self):
        """The dropdown's own selection never narrows its own list, so a
        filter naming only the target column leaves the cube path reachable."""
        result = generate(filters=[{"column": "region", "value": "Europe"}])
        self.assertIn("GROUP BY region", result["sql"])

    def test_an_empty_cascading_value_does_not_force_the_scan(self):
        for value in (None, "", []):
            result = generate(filters=[{"column": "platform", "value": value}])
            self.assertIn("GROUP BY region", result["sql"], repr(value))

    def test_a_dataset_with_dimension_joins_keeps_the_scan(self):
        result = generate(dimensions=[{"table": "public.dim_region",
                                       "factKey": "region_id", "dimKey": "region_id"}])
        self.assertNotIn("GROUP BY", result["sql"])

    def test_a_column_that_lives_on_a_joined_dimension_table_keeps_its_own_path(self):
        result = generate(
            column="region_name",
            dimensions=[{"table": "public.dim_region", "factKey": "region_id",
                         "dimKey": "region_id"}],
            columns=COLUMNS + [{"table_name": "public.dim_region",
                                "column_name": "region_name", "is_dimension": True}],
        )
        self.assertNotIn("GROUP BY", result["sql"])
        self.assertIn("public.dim_region", result["sql"])


class NonEngineSourcesAreUntouchedTests(unittest.TestCase):
    """The cube belongs to the Engine. Every other source keeps the statement
    it had, byte for byte."""

    def test_postgresql_is_unchanged(self):
        self.assertEqual(
            generate(db_type="postgresql", engine_source=False)["sql"],
            'SELECT DISTINCT fact."region" AS "key", fact."region" AS "value" '
            'FROM "public"."kaveon_events_enriched" AS fact '
            'WHERE fact."region" IS NOT NULL ORDER BY "value" LIMIT 100',
        )

    def test_fabric_sql_is_unchanged(self):
        self.assertEqual(
            generate(db_type="fabric_sql", engine_source=False)["sql"],
            "SELECT DISTINCT TOP 100 fact.[region] AS [key], fact.[region] AS [value] "
            "FROM [public].[kaveon_events_enriched] AS fact "
            "WHERE fact.[region] IS NOT NULL ORDER BY [value]",
        )

    def test_mysql_is_unchanged(self):
        self.assertIn("SELECT DISTINCT", generate(db_type="mysql", engine_source=False)["sql"])

    def test_an_engine_dialect_without_an_engine_source_is_unchanged(self):
        """`db_type` names the dialect; `engine_source` names the catalog. Only
        the catalog decides whether a cube exists."""
        self.assertNotIn("GROUP BY", generate(db_type="kaveon", engine_source=False)["sql"])


if __name__ == "__main__":
    unittest.main()
