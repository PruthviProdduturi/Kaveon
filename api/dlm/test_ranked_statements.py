"""A ranking excludes groups with no value for the measure.

"Top 10 countries by energy consumption" answered with the ten entities that
have no energy figures at all: their `SUM` is NULL, and `ORDER BY … DESC` sorts
NULL first in standard SQL. The statement was correct and the answer was
useless, and the UI rendered each NULL as a 0 — so it read as ten countries
consuming nothing.
"""

import sys
import unittest
from types import SimpleNamespace

if "pyodbc" not in sys.modules:
    sys.modules["pyodbc"] = SimpleNamespace(Error=Exception)

from dlm import engine_dialect as d


COLUMNS = [{"column_name": "country"}, {"column_name": "primary_energy_consumption"}]


def assemble(dialect, **kwargs):
    base = dict(schema="climate_energy", fact="energy",
                metric_expr="SUM(primary_energy_consumption)", metric_name="TWh",
                group_cols=["country"], time_group=None, filters=[], columns=COLUMNS)
    base.update(kwargs)
    return d.assemble(dialect, **base)


class RankedStatementsExcludeNullMeasures(unittest.TestCase):
    def test_a_descending_ranking_excludes_null_measures(self):
        sql = assemble(d.ENGINE, limit_n=10, sort_asc=False)
        self.assertIn("IS NOT NULL", sql)
        # Clause order matters: HAVING must precede ORDER BY or it is a syntax
        # error in every dialect.
        self.assertLess(sql.index("HAVING"), sql.index("ORDER BY"))
        self.assertIn("DESC", sql)

    def test_an_ascending_ranking_excludes_them_too(self):
        """A "lowest" question is the same trap from the other end: a NULL
        sorts first in ASC in several dialects, so the bottom ten would be
        ten groups with no measurement."""
        sql = assemble(d.ENGINE, limit_n=10, sort_asc=True)
        self.assertIn("IS NOT NULL", sql)
        self.assertLess(sql.index("HAVING"), sql.index("ORDER BY"))

    def test_the_exclusion_names_the_aggregate_not_the_alias(self):
        """`HAVING TWh IS NOT NULL` is invalid in standard SQL — the alias is
        not in scope there — so the aggregate expression has to be repeated."""
        sql = assemble(d.ENGINE, limit_n=10)
        having = sql[sql.index("HAVING"):sql.index("ORDER BY")]
        self.assertIn("SUM(", having)
        self.assertNotIn("TWh IS NOT NULL", having)

    def test_an_unranked_statement_is_untouched(self):
        """`ranked=False` is the shape the Engine's cube answers; adding a
        HAVING to it would stop the cube matching the plan."""
        sql = assemble(d.ENGINE, limit_n=10, ranked=False)
        self.assertNotIn("HAVING", sql)
        self.assertNotIn("ORDER BY", sql)
        self.assertNotIn("LIMIT", sql)

    def test_a_time_series_is_untouched(self):
        """Ordered by time, not by the measure: a period with no value is a
        gap in the series and belongs in it."""
        sql = assemble(d.ENGINE, time_group="year", limit_n=10)
        self.assertNotIn("HAVING", sql)
        self.assertIn("ORDER BY", sql)

    def test_every_dialect_gets_the_exclusion(self):
        """The portability reason for HAVING over NULLS LAST: SQL Server and
        Fabric have no NULLS LAST, so a dialect-specific ordering clause would
        have fixed one backend and broken others."""
        for name in dir(d):
            value = getattr(d, name)
            if not isinstance(value, d.Dialect) or name.startswith("_"):
                continue
            sql = assemble(value, limit_n=10)
            self.assertIn("IS NOT NULL", sql, f"{name} lost the exclusion")
            self.assertNotIn("NULLS LAST", sql, f"{name} used a non-portable clause")


if __name__ == "__main__":
    unittest.main()
