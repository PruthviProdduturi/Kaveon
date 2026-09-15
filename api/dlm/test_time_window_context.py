"""Year and relative-time windows and trends are answered from the day cells an
additive metric stores at build time; one filter or one breakdown inside a
window uses the date|dimension pair the cuboid cover stored. Non-additive
metrics still run live."""
import sys
import unittest
from datetime import date
from types import SimpleNamespace
from unittest.mock import patch

if "pyodbc" not in sys.modules:
    sys.modules["pyodbc"] = SimpleNamespace(Error=Exception)

from dlm import engine

DS = {"dataset_name": "Telemetry"}
DAYS = {
    ("Total actions", "event_date"): {"columns": ["event_date", "Total actions"],
        "rows": [["2025-12-30", 5], ["2025-12-31", 7], ["2026-01-01", 10], ["2026-01-15", 20], ["2026-02-01", 30], ["2026-02-02", 40]]},
    ("Total actions", "country|event_date"): {"columns": ["country", "event_date", "Total actions"],
        "rows": [["India", "2025-12-31", 4], ["India", "2026-01-01", 6], ["Germany", "2026-01-01", 4],
                 ["India", "2026-02-01", 25], ["Germany", "2026-02-01", 5], ["Germany", "2026-02-02", 40]]},
}


def cells(dataset_id, metric, group):
    return DAYS.get((metric, group))


class TimeWindowTests(unittest.TestCase):
    def setUp(self):
        self.enter = self.enterContext if hasattr(self, "enterContext") else None
        self._p = patch.object(engine, "_context_answer", cells)
        self._p.start()
        self.addCleanup(self._p.stop)

    def serve(self, **kw):
        args = dict(dataset_id="1", ds=DS, metric_name="Total actions", group_col=None, filters=[],
                    date_column="event_date", year=None, relative_time=None, time_group=None,
                    question="", top_n=None, sort_asc=False, conf=0.9)
        args.update(kw)
        return engine._serve_time_window(**args)

    def test_a_year_window_sums_the_days_inside_it(self):
        r = self.serve(year=2026, question="actions in 2026")
        self.assertEqual(r["rows"], [[100]])
        self.assertTrue(r["from_context"])
        self.assertIn("2026", r["title"])

    def test_a_named_month_narrows_the_year(self):
        r = self.serve(year=2026, month=2, question="actions in February 2026")
        self.assertEqual(r["rows"], [[70]])
        self.assertIn("February 2026", r["title"])
        self.assertEqual(engine._extract_month("actions in Jul 2026"), 7)
        self.assertEqual(engine._extract_month("actions in 2026 july"), 7)
        self.assertEqual(engine._extract_month("actions during september, 2026"), 9)
        self.assertIsNone(engine._extract_month("actions in 2026"))
        self.assertIsNone(engine._extract_month("what may the actions be in 2026"))   # the verb, not the month
        self.assertEqual(engine._extract_month("actions in May 2026"), 5)

    def test_a_relative_window_is_a_lower_bound(self):
        r = self.serve(relative_time="'2026-02-01'", question="actions in the last 7 days")
        self.assertEqual(r["rows"], [[70]])

    def test_a_trend_rolls_days_up_to_months(self):
        r = self.serve(time_group="event_date", question="trend of actions by month")
        self.assertEqual(r["rows"], [["2025-12", 12], ["2026-01", 30], ["2026-02", 70]])
        self.assertEqual(r["chartType"], "line")
        self.assertEqual(r["xAxis"], "event_date")

    def test_a_trend_inside_a_year_keeps_days_by_default(self):
        r = self.serve(time_group="event_date", year=2026, question="actions over time in 2026")
        self.assertEqual([row[0] for row in r["rows"]], ["2026-01-01", "2026-01-15", "2026-02-01", "2026-02-02"])

    def test_a_breakdown_inside_a_window_uses_the_pair_cells(self):
        r = self.serve(group_col="country", year=2026, question="actions in 2026 by country")
        self.assertEqual(r["rows"], [["Germany", 49], ["India", 31]])
        self.assertEqual(r["chartType"], "bar")

    def test_a_filter_inside_a_window_uses_the_pair_cells(self):
        r = self.serve(filters=[{"column": "country", "value": "India"}], year=2026, question="India actions in 2026")
        self.assertEqual(r["rows"], [[31]])
        self.assertIn("India", r["title"])

    def test_missing_cells_fall_through_to_live(self):
        self.assertIsNone(self.serve(group_col="platform", year=2026, question="by platform in 2026"))
        self.assertIsNone(self.serve(filters=[{"column": "a", "value": 1}], group_col="country", year=2026))

    def test_year_bounds_come_from_the_cells(self):
        self.assertEqual(engine._cell_year_bounds("1", "Total actions", "event_date", []), (2025, 2026))
        self.assertEqual(engine._cell_year_bounds("1", "Total actions", "event_date",
                                                  [{"column": "country", "value": "Germany"}]), (2026, 2026))
        self.assertEqual(engine._cell_year_bounds("1", "Users", "event_date", []), (None, None))


class WindowParsingTests(unittest.TestCase):
    def test_year_and_iso_lower_bound_and_today(self):
        self.assertEqual(engine._time_window(2026, None), ("2026-01-01", "2027-01-01"))
        self.assertEqual(engine._time_window(None, "'2026-09-07'"), ("2026-09-07", None))
        today = date.today().isoformat()
        self.assertEqual(engine._time_window(None, "CURRENT_DATE")[0], today)
        self.assertIsNone(engine._time_window(None, None))


if __name__ == "__main__":
    unittest.main()
