"""Two words that carried two meanings each.

Both defects came from the same shape: a token that means one thing in one
question and the opposite in another, read only one way.

* "across" is a grouping cue in "deaths across countries" and a SCOPE cue in
  "deaths across the world". Read as grouping, "total number of covid deaths
  across the world" answered with fifty rows titled "Deaths by country" and
  offered to narrow down to a top 10. The question asked for one number.

* "chart" is a presentation word in "give me a chart" and a measured subject in
  "how many charts were created" -- the telemetry dataset's metrics are
  `charts_created`, `queries_run`, `dashboards`. So a request to draw the
  current answer scored against that dataset at the x3 metric weight, and after
  two COVID questions "can you make a world map chart" answered
  `SELECT SUM(charts_created) FROM events.daily_user_activity` and timed out
  against a billion rows.

The second is why neither is fixed by adding words to a noise list: that
dataset's subject genuinely is charts. What separates the readings is the
request form and what follows the cue, which is what these pin.
"""

import sys
import unittest
from types import SimpleNamespace

if "pyodbc" not in sys.modules:
    sys.modules["pyodbc"] = SimpleNamespace(Error=Exception)

from dlm import engine


class WholeScopeCancelsTheGroupingCue(unittest.TestCase):
    def test_the_world_is_a_scope(self):
        for question in ("total number of covid deaths across the world",
                         "deaths around the world",
                         "cases over the whole world",
                         "deaths worldwide",
                         "deaths world wide",
                         "total deaths globally",
                         "deaths overall",
                         "deaths in total",
                         "deaths altogether"):
            self.assertTrue(engine._asks_for_the_whole(question), question)

    def test_a_dimension_after_the_cue_is_still_a_grouping(self):
        """The distinction the regex has to keep: "across all countries" wants a
        row per country, "across the world" wants one number."""
        for question in ("deaths across countries",
                         "deaths across all countries",
                         "deaths across each region",
                         "deaths by country",
                         "cases per region"):
            self.assertFalse(engine._asks_for_the_whole(question), question)


class RedrawRequestsKeepTheDataset(unittest.TestCase):
    def test_a_request_to_draw_is_a_redraw(self):
        for question in ("can you make a world map chart",
                         "give me a chart",
                         "show that as a bar chart",
                         "plot it as a line graph",
                         "can you draw a pie chart",
                         "make a world map",
                         "as a table",
                         "turn it into a scatter plot",
                         "display as a donut"):
            self.assertTrue(engine._asks_only_to_redraw(question), question)

    def test_a_question_about_charts_is_not_a_redraw(self):
        """The negative half is the one that matters. The telemetry dataset
        measures charts, so these must keep routing on their own merits — a
        noise-list fix would have made them unanswerable."""
        for question in ("how many charts were created last week",
                         "charts created by platform",
                         "total queries run this month",
                         "which dashboard has the most views",
                         "dashboards by region",
                         "chart creation trend over time"):
            self.assertFalse(engine._asks_only_to_redraw(question), question)

    def test_an_ordinary_question_is_not_a_redraw(self):
        for question in ("total number of covid deaths across the world",
                         "top 10 countries by energy consumption",
                         "deaths by country"):
            self.assertFalse(engine._asks_only_to_redraw(question), question)


if __name__ == "__main__":
    unittest.main()
