"""What the DLM does where there is no value, and where there is no data.

The live path already excludes a group with no measurement from a ranking —
`engine_dialect.assemble` writes `HAVING <agg> IS NOT NULL`, because without
it "top 10 countries by energy consumption" answered with the ten entities
that have no energy figures at all: their SUM is NULL and a NULL sorts first
in `ORDER BY … DESC`. These are the same question's other paths, each of which
read an absence as a reading:

* **A ranking served from the cube was not ranked.** The precomputed cells come
  back in the order the build scan produced them — by the cuboid's keys, never
  by the measure — and the filtered and multi-dimension branches sliced that to
  `top_n` without ordering it first. "Top 3 countries by energy in Europe"
  returned Albania, a country with no energy figures, and Austria, in that
  order, and `sort_asc` was ignored outright, so "bottom 3" returned the same
  three rows as "top 3".

* **A window the data does not reach answered zero.** The day cells are summed
  inside `[lo, hi)`; a window outside them sums nothing, and nothing was
  rendered as 0. "Energy in the last 7 days" answered "Energy is 0" against a
  dataset whose last day is years back, labelled "from context · no DB scan".
  A named year is caught upstream and shifted into range with a note; a
  relative window cannot be shifted, so the answer has to say what the data
  holds.

* **A missing number was printed as the word "None".** `fmt(None)` fell through
  to `str(value)`, so answer sentences read "Energy by country: Albania 10,
  Algeria None" and "Energy consumption is — of Energy consumption (None of
  None)".

* **A NULL cell was taken into arithmetic.** The proportional fallback scales a
  breakdown by each filter's weight; a cell with no value is None, and
  multiplying one raised a TypeError out of a question that should have been
  answered.
"""

import sys
import unittest
from unittest.mock import patch

if "pyodbc" not in sys.modules:
    from types import SimpleNamespace
    sys.modules["pyodbc"] = SimpleNamespace(Error=Exception)

from dlm import classes, engine

DS = {"dataset_name": "Climate energy",
      "metrics": [{"name": "Energy", "expression": "SUM(e)", "metric_type": "sum"}]}

# Cells in the order a build scan produces them — by the cuboid's keys. Two
# countries have no energy figures at all.
CELLS = {
    ("Energy", ""): {"columns": ["Energy"], "rows": [[6000]]},
    ("Energy", "country"): {"columns": ["country", "Energy"],
                            "rows": [["Albania", 10], ["Algeria", None], ["Austria", 55],
                                     ["Brazil", 900], ["China", 3000], ["Denmark", None],
                                     ["India", 2000], ["Egypt", 35]]},
    ("Energy", "country|region"): {"columns": ["country", "region", "Energy"],
                                   "rows": [["Albania", "Europe", 10],
                                            ["Algeria", "Africa", None],
                                            ["Austria", "Europe", 55],
                                            ["Brazil", "Americas", 900],
                                            ["China", "Asia", 3000],
                                            ["Denmark", "Europe", None],
                                            ["India", "Asia", 2000],
                                            ["Egypt", "Africa", 35]]},
    ("Energy", "platform"): {"columns": ["platform", "Energy"],
                             "rows": [["web", 1200], ["ios", None], ["android", 4800]]},
    ("Energy", "tier"): {"columns": ["tier", "Energy"],
                         "rows": [["free", 3000], ["paid", 3000]]},
    ("Energy", "year_date"): {"columns": ["year_date", "Energy"],
                              "rows": [["2020-01-01", 100], ["2021-01-01", 200],
                                       ["2022-01-01", 300]]},
}


def cells(dataset_id, metric, group):
    return CELLS.get((metric, group))


class ARankingFromTheCubeIsRanked(unittest.TestCase):
    def setUp(self):
        patcher = patch.object(engine, "_context_answer", cells)
        patcher.start()
        self.addCleanup(patcher.stop)

    def serve(self, **kw):
        args = dict(dataset_id="1", ds=DS, metric_name="Energy", group_col=None,
                    top_n=None, filters=[], routed={"score": 1.0})
        args.update(kw)
        return engine._serve_from_context(**args)

    def test_rank_cells_orders_and_drops_what_has_no_value(self):
        rows = [["a", 10], ["b", None], ["c", 55], ["d", 3]]
        self.assertEqual(engine._rank_cells(rows, top_n=2, sort_asc=False), [["c", 55], ["a", 10]])
        self.assertEqual(engine._rank_cells(rows, top_n=2, sort_asc=True), [["d", 3], ["a", 10]])
        self.assertEqual(engine._rank_cells(rows, top_n=None, sort_asc=False),
                         [["c", 55], ["a", 10], ["d", 3]])
        self.assertEqual(engine._rank_cells([], top_n=3, sort_asc=False), [])
        self.assertEqual(engine._rank_cells([["a", None]], top_n=3, sort_asc=False), [])

    def test_a_top_n_inside_a_filter_is_the_top_n(self):
        served = self.serve(group_col="country", top_n=3,
                            filters=[{"column": "region", "value": "Europe"}])
        self.assertEqual(served["rows"], [["Austria", 55], ["Albania", 10]])

    def test_a_bottom_n_inside_a_filter_is_the_bottom_n(self):
        served = self.serve(group_col="country", top_n=3, sort_asc=True,
                            filters=[{"column": "region", "value": "Europe"}])
        self.assertEqual(served["rows"], [["Albania", 10], ["Austria", 55]])

    def test_a_top_n_with_no_filter_is_the_top_n(self):
        served = self.serve(group_col="country", top_n=3)
        self.assertEqual(served["rows"], [["China", 3000], ["India", 2000], ["Brazil", 900]])

    def test_a_plain_breakdown_keeps_the_order_the_cube_stored(self):
        """The negative. A breakdown is not a ranking: `serve_chart` reads these
        same cells for a dashboard, where a series grouped by a date column must
        stay in the cube's order rather than being sorted by value."""
        served = self.serve(group_col="country")
        self.assertEqual([row[0] for row in served["rows"]],
                         ["Albania", "Algeria", "Austria", "Brazil", "China", "Denmark",
                          "India", "Egypt"])

    def test_the_proportional_fallback_does_not_take_a_null_into_arithmetic(self):
        served = self.serve(group_col="country", top_n=2,
                            filters=[{"column": "platform", "value": "web"},
                                     {"column": "tier", "value": "free"}])
        self.assertTrue(served["approx"])
        self.assertEqual([row[0] for row in served["rows"]], ["China", "India"])
        for _label, value in served["rows"]:
            self.assertIsNotNone(value)


class AWindowTheDataDoesNotReach(unittest.TestCase):
    def setUp(self):
        patcher = patch.object(engine, "_context_answer", cells)
        patcher.start()
        self.addCleanup(patcher.stop)

    def serve(self, **kw):
        args = dict(dataset_id="1", ds=DS, metric_name="Energy", group_col=None, filters=[],
                    date_column="year_date", year=None, relative_time=None, time_group=None,
                    question="", top_n=None, sort_asc=False, conf=1.0)
        args.update(kw)
        return engine._serve_time_window(**args)

    def test_an_empty_window_says_what_the_data_holds(self):
        served = self.serve(relative_time="'2026-10-08'",
                            question="energy in the last 7 days")
        self.assertEqual(served["rows"], [[0]])
        self.assertIn("2020-01-01 to 2022-01-01", served["note"])
        self.assertIn("contains none of it", served["note"])

    def test_an_empty_year_window_says_the_same(self):
        served = self.serve(year=2019, question="energy in 2019")
        self.assertEqual(served["rows"], [[0]])
        self.assertIn("2020-01-01 to 2022-01-01", served["note"])

    def test_a_window_the_data_does_reach_says_nothing(self):
        """The negative: an ordinary window must not be annotated."""
        for kwargs in ({"year": 2021, "question": "energy in 2021"},
                       {"relative_time": "'2020-06-01'", "question": "energy lately"},
                       {"time_group": "year_date", "question": "energy over time"}):
            served = self.serve(**kwargs)
            self.assertIsNone(served["note"], kwargs)

    def test_a_trend_over_an_empty_window_is_annotated_too(self):
        served = self.serve(time_group="year_date", year=2019,
                            question="energy over time in 2019")
        self.assertEqual(served["rows"], [])
        self.assertIn("contains none of it", served["note"])


class AMissingNumberIsNotTheWordNone(unittest.TestCase):
    def test_fmt_says_nothing_with_a_dash(self):
        self.assertEqual(classes.fmt(None), "—")

    def test_fmt_still_renders_every_real_value(self):
        for value, text in ((0, "0"), (3, "3"), (1234, "1,234"), (1234.5, "1,234.50"),
                            (-7, "-7"), ("Europe", "Europe")):
            self.assertEqual(classes.fmt(value), text, value)

    def test_an_answer_sentence_shows_the_absence_as_one(self):
        rows = [["Albania", 10], ["Algeria", None], ["India", 2000]]
        sentence = classes.base_sentence("breakdown", metric="Energy", dimension="country",
                                         rows=rows)
        self.assertNotIn("None", sentence)
        self.assertIn("Algeria —", sentence)

    def test_a_share_of_nothing_reads_as_nothing(self):
        composed = classes.compose_share({"rows": [[None]]}, {"rows": [[None]]},
                                         metric="Energy", subject="India", group_col=None)
        self.assertNotIn("None", composed["answer"])


class TheExistenceCountAsAProportion(unittest.TestCase):
    RESULT = {"columns": ["country", "Energy"],
              "rows": [["China", 3000], ["India", 2000], ["Albania", 10], ["Egypt", 35]]}

    def test_the_proportion_is_said_when_it_was_asked_for(self):
        composed = classes.compose_existence(self.RESULT, metric="Energy", dimension="country",
                                            comparison=">", threshold=1000, as_share=True)
        self.assertIn("2 of 4 country have Energy above 1,000", composed["answer"])
        self.assertIn("That is 50.0%", composed["answer"])

    def test_the_count_alone_is_unchanged_when_it_was_not(self):
        composed = classes.compose_existence(self.RESULT, metric="Energy", dimension="country",
                                            comparison=">", threshold=1000)
        self.assertIn("2 of 4 country have Energy above 1,000", composed["answer"])
        self.assertNotIn("That is", composed["answer"])


if __name__ == "__main__":
    unittest.main()
