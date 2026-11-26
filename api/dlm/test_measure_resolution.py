"""A measure the dataset declares is a measure the router can reach.

A dataset declares its measures twice: as columns carrying `is_metric`, and
optionally again as named metrics with expressions. Only the named ones
reached the router, so a question naming a real measure column matched nothing
and fell through to whichever metric the fallback preferred. Over a COVID
dataset declaring `total_cases, new_cases, total_deaths, new_deaths,
population` but naming only "Total cases" and "Countries", asking for "total
deaths by country" answered:

    SELECT country, COUNT(DISTINCT country) AS Countries ... ORDER BY Countries

A wrong answer, confidently, with no clarification -- which is the one failure
this product is sold on not making.

The second half is the ranking. With the measures reachable, "total deaths"
tied `total_deaths` against `new_deaths`: the scorer drops quantifier words
before matching, so both saw only "deaths" and the router asked which was
meant. Asking was honest but unnecessary -- the discarded word was the one
that chose.
"""

import unittest
from unittest.mock import patch

from dlm import engine
from dlm.test_question_classes import DATASET, Harness


COLUMNS = [
    {"column_name": "country", "is_dimension": True, "is_metric": False},
    {"column_name": "continent", "is_dimension": True, "is_metric": False},
    {"column_name": "total_cases", "is_dimension": False, "is_metric": True},
    {"column_name": "new_cases", "is_dimension": False, "is_metric": True},
    {"column_name": "total_deaths", "is_dimension": False, "is_metric": True},
    {"column_name": "new_deaths", "is_dimension": False, "is_metric": True},
    {"column_name": "population", "is_dimension": False, "is_metric": True},
]
NAMED = [
    {"name": "Total cases", "expression": "SUM(total_cases)", "metric_type": "sum"},
    {"name": "Countries", "expression": "COUNT(DISTINCT country)", "metric_type": "count_distinct"},
]


def _names(metrics):
    return [m.get("name") or m.get("metric_name") for m in metrics]


class DeclaredMeasuresBecomeReachable(unittest.TestCase):
    def test_a_measure_column_with_no_named_metric_is_offered(self):
        implicit = engine._implicit_measures(COLUMNS, NAMED)
        self.assertEqual(sorted(_names(implicit)),
                         ["new_cases", "new_deaths", "population", "total_deaths"])

    def test_a_column_a_named_metric_already_covers_is_not_duplicated(self):
        """`Total cases` is SUM(total_cases), so the column is already spoken
        for. Offering it twice would make every question about it a tie."""
        self.assertNotIn("total_cases", _names(engine._implicit_measures(COLUMNS, NAMED)))

    def test_a_dimension_is_never_offered_as_a_measure(self):
        offered = _names(engine._implicit_measures(COLUMNS, NAMED))
        self.assertNotIn("country", offered)
        self.assertNotIn("continent", offered)

    def test_each_offered_measure_sums_its_own_column(self):
        for metric in engine._implicit_measures(COLUMNS, NAMED):
            self.assertEqual(metric["expression"], f"SUM({metric['name']})")
            self.assertTrue(metric["implicit"])

    def test_a_distinct_count_also_covers_its_column(self):
        """`COUNT(DISTINCT country)` claims `country` as thoroughly as a SUM
        would; the extractor reads the argument, not the function."""
        columns = [{"column_name": "country", "is_dimension": False, "is_metric": True}]
        self.assertEqual(engine._implicit_measures(columns, NAMED), [])

    def test_no_measure_columns_offers_nothing(self):
        plain = [{"column_name": "country", "is_dimension": True, "is_metric": False}]
        self.assertEqual(engine._implicit_measures(plain, NAMED), [])


class TheQuantifierBreaksATieItCannotWin(unittest.TestCase):
    """"total" says nothing about which measure a question wants, so it is
    dropped before matching -- until two measures tie without it."""

    def _rank(self, question, metrics):
        qset = set(engine._tokenize(question))
        return engine._rank_metrics(qset, metrics)

    def test_total_deaths_chooses_the_total_not_the_new(self):
        metrics = engine._implicit_measures(COLUMNS, NAMED)
        ranked = self._rank("total deaths by country", metrics)
        self.assertEqual(_names([ranked[0][0]])[0], "total_deaths")
        self.assertGreater(ranked[0][1], ranked[1][1],
                           "the quantifier must separate them, not leave a tie")

    def test_new_deaths_chooses_the_new_not_the_total(self):
        metrics = engine._implicit_measures(COLUMNS, NAMED)
        ranked = self._rank("new deaths by continent", metrics)
        self.assertEqual(_names([ranked[0][0]])[0], "new_deaths")

    def test_a_quantifier_alone_never_outranks_a_distinctive_word(self):
        """"total cases" must not drift to `total_deaths` because both say
        "total": one distinctive word beats any number of quantifiers."""
        ranked = self._rank("total cases", NAMED + engine._implicit_measures(COLUMNS, NAMED))
        self.assertEqual(_names([ranked[0][0]])[0], "Total cases")

    def test_a_bare_quantifier_matches_nothing(self):
        """"total" on its own names no measure, so the caller falls through to
        its default rather than picking whatever is called "Total something"."""
        ranked = self._rank("what is the total", NAMED)
        self.assertEqual(ranked[0][1], 0)

    def test_a_genuine_ambiguity_still_ties(self):
        """Nothing in the question separates these, so the router must still
        ask rather than invent a preference."""
        metrics = [{"name": "deaths_reported", "expression": "SUM(deaths_reported)"},
                   {"name": "deaths_confirmed", "expression": "SUM(deaths_confirmed)"}]
        ranked = self._rank("deaths by country", metrics)
        self.assertEqual(ranked[0][1], ranked[1][1])


class TheAskPathReachesAnUnnamedMeasure(unittest.TestCase):
    """The end the user sees. The two cases above test the pieces; this one
    asks a question and reads the statement that came back, so the wiring
    between them cannot quietly come undone."""

    # The shipped fixture declares every measure it has, which is why removing
    # the wiring broke none of its tests. This one keeps `errors` a measure
    # column with no named metric over it -- the shape the bug needs.
    UNNAMED = {
        **DATASET,
        "columns": [
            {"table_name": "usage", "column_name": "region", "is_dimension": True},
            {"table_name": "usage", "column_name": "usage_date", "is_dimension": False},
            {"table_name": "usage", "column_name": "queries_run", "is_dimension": False,
             "is_metric": True},
            {"table_name": "usage", "column_name": "errors", "is_dimension": False,
             "is_metric": True},
        ],
        "metrics": [{"name": "Queries run", "expression": "SUM(queries_run)"}],
    }

    def _ask(self, question):
        with Harness() as harness:
            with patch.object(engine.datasets_svc, "get_dataset_by_id", lambda i: self.UNNAMED):
                engine.ask(question)
            return harness.statements[0] if harness.statements else ""

    def test_a_question_about_an_unnamed_measure_sums_that_column(self):
        statement = self._ask("errors by region")
        self.assertIn("SUM(errors)", statement)
        self.assertNotIn("queries_run", statement,
                         "the named metric must not stand in for the one asked about")

    def test_the_named_metric_still_answers_its_own_question(self):
        self.assertIn("SUM(queries_run)", self._ask("queries run by region"))


if __name__ == "__main__":
    unittest.main()
