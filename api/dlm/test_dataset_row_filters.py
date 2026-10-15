"""A dataset can say which rows belong to it.

`climate_energy.energy` holds `World`, `OECD (EI)` and `High-income countries`
in the same `country` column as the countries, as the upstream data does, so
"top 10 countries by energy consumption" answered with aggregates. `iso_code`
separates them — null for every aggregate, set for every country — and there
was nowhere to say that: a dataset's filters were equality pairs, so "every row
with an ISO code" was inexpressible.

Dropping the aggregate rows would have been the wrong fix: `World` is a
legitimate answer to a question about the world.
"""

import sys
import unittest
from types import SimpleNamespace

if "pyodbc" not in sys.modules:
    sys.modules["pyodbc"] = SimpleNamespace(Error=Exception)

from dlm import engine_dialect as d


COLUMNS = [{"column_name": "country"}, {"column_name": "iso_code"}]
NOT_NULL = {"column": "iso_code", "op": "IS NOT NULL"}


def assemble(**kwargs):
    base = dict(schema="climate_energy", fact="energy",
                metric_expr="SUM(primary_energy_consumption)", metric_name="TWh",
                group_cols=["country"], time_group=None, filters=[], columns=COLUMNS)
    base.update(kwargs)
    return d.assemble(d.ENGINE, **base)


class PredicateRendering(unittest.TestCase):
    def test_a_null_test_renders_without_a_value(self):
        self.assertEqual(d.predicate(d.ENGINE, NOT_NULL), "iso_code IS NOT NULL")
        self.assertEqual(d.predicate(d.ENGINE, {"column": "iso_code", "op": "IS NULL"}),
                         "iso_code IS NULL")

    def test_a_comparison_renders_its_value_through_the_dialect(self):
        out = d.predicate(d.ENGINE, {"column": "year", "op": ">=", "value": 2000})
        self.assertIn(">=", out)
        self.assertIn("2000", out)

    def test_an_unknown_operator_degrades_to_equality_rather_than_being_interpolated(self):
        """This is what keeps a stored predicate from becoming an injection
        point: the operator is never passed through, only selected."""
        out = d.predicate(d.ENGINE, {"column": "country", "op": "; DROP TABLE x --",
                                     "value": "Chad"})
        self.assertNotIn("DROP", out)
        self.assertIn("=", out)

    def test_a_clause_naming_no_column_says_nothing(self):
        self.assertIsNone(d.predicate(d.ENGINE, {"op": "IS NOT NULL"}))
        self.assertIsNone(d.predicate(d.ENGINE, {"column": "", "op": "IS NOT NULL"}))


class DatasetPredicatesHoldForEveryQuestion(unittest.TestCase):
    def test_the_predicate_reaches_the_where_clause(self):
        sql = assemble(row_filters=[NOT_NULL], limit_n=10)
        self.assertIn("WHERE iso_code IS NOT NULL", sql)

    def test_it_combines_with_the_question_s_own_filters(self):
        sql = assemble(row_filters=[NOT_NULL],
                       filters=[{"column": "year", "value": 2020}], limit_n=10)
        where = sql[sql.index("WHERE"):sql.index("GROUP BY")]
        self.assertIn("iso_code IS NOT NULL", where)
        self.assertIn("year", where)
        self.assertIn(" AND ", where)

    def test_the_dataset_s_predicate_comes_first(self):
        """It says which rows are part of the dataset at all, so it reads ahead
        of whatever one question narrows to."""
        sql = assemble(row_filters=[NOT_NULL],
                       filters=[{"column": "year", "value": 2020}], limit_n=10)
        self.assertLess(sql.index("iso_code"), sql.index("year"))

    def test_it_holds_on_the_unranked_cube_shape_too(self):
        """The cube answers the unranked shape. A predicate applied only when
        ranked would mean a cube-answered question and a scanned one disagreed
        about which rows the dataset contains."""
        sql = assemble(row_filters=[NOT_NULL], ranked=False)
        self.assertIn("iso_code IS NOT NULL", sql)

    def test_no_predicate_leaves_the_statement_as_it_was(self):
        self.assertNotIn("WHERE", assemble(limit_n=10))

    def test_a_malformed_clause_is_dropped_rather_than_rendered(self):
        sql = assemble(row_filters=[{"op": "IS NOT NULL"}, "not a dict", NOT_NULL],
                       limit_n=10)
        # Scoped to WHERE: a ranked statement also carries the D-45 null
        # exclusion in its HAVING, which is a different clause saying a
        # different thing.
        where = sql[sql.index("WHERE"):sql.index("GROUP BY")]
        self.assertEqual(where.count("IS NOT NULL"), 1)
        self.assertIn("iso_code IS NOT NULL", where)


if __name__ == "__main__":
    unittest.main()
