import unittest

from dlm import classes, coverage

SPEC = {
    "metrics": {"Users": {"additive": True, "default": True},
                "Errors": {"additive": True},
                "Locales": {"additive": False, "approximate": True},
                "Hidden": {"additive": True, "hidden": True}},
    "dimensions": {"platform": {}, "country": {}, "secret": {"hidden": True}},
    "default_metric": "Users",
}
SAMPLES = {"platform": ["Desktop", "Web"], "country": ["India", "Japan"]}
TIMED_SPEC = dict(SPEC, time={"column": "usage_date", "min": "2025-01-01", "max": "2026-08-18",
                              "grain": "day", "latest": "2026-08-18"})
DATASET = {
    "schema_name": "kaveon_product", "table_name": "usage",
    "metrics": [{"name": "Users", "expression": "COUNT(*)"},
                {"name": "Errors", "expression": "SUM(errors)"},
                {"name": "Locales", "expression": "COUNT(DISTINCT locale)"}],
}


class CorpusTests(unittest.TestCase):
    def test_the_corpus_follows_the_spec_and_skips_hidden_elements(self):
        questions = coverage.corpus(SPEC, SAMPLES, None, per_class=8)
        self.assertEqual(questions["total"], ["total users", "total errors"])
        self.assertEqual(questions["breakdown"], ["users by platform", "users by country"])
        self.assertEqual(questions["filter"], ["users in Desktop", "users in India",
                                               "users for Web", "users for Japan"])
        self.assertEqual(questions["filter_breakdown"], ["users by country in Desktop"])
        self.assertEqual(questions["two_filters"], ["users in Desktop Japan"])
        self.assertEqual(questions["top_n"], ["top 3 platform by users", "top 3 country by users",
                                              "bottom 3 platform by users",
                                              "bottom 3 country by users"])
        self.assertEqual(questions["distinct_total"], ["total locales"])
        self.assertEqual(questions["distinct_breakdown"], ["locales by platform", "locales by country"])
        self.assertNotIn("time_slice", questions)                 # no time dimension
        self.assertNotIn("comparison_period", questions)
        self.assertIn("errors per users", questions["ratio"])
        self.assertEqual(questions["top_n_within"],
                         ["top 2 country by users in each platform"])
        self.assertEqual(questions["existence"][0],
                         "how many platform have more than 1000 users")
        self.assertEqual(len(questions["out_of_scope"]), 3)

    def test_the_time_classes_appear_only_with_a_time_dimension(self):
        questions = coverage.corpus(TIMED_SPEC, SAMPLES, "usage_date", per_class=3)
        self.assertEqual(questions["time_slice"][0], "users in 2026")
        self.assertEqual(questions["trend"][0], "users over time")
        self.assertEqual(questions["comparison_period"][0], "users vs last month")
        self.assertEqual(questions["year_over_year"][0], "users year over year")

    def test_the_corpus_reaches_a_hundred_and_twenty_questions_on_a_real_shape(self):
        spec = dict(TIMED_SPEC)
        spec["dimensions"] = {name: {} for name in
                              ("platform", "license", "segment", "industry", "region",
                               "country", "deployment", "acquisition_channel", "team_size")}
        spec["metrics"] = {**SPEC["metrics"],
                           **{f"Measure {i}": {"additive": True} for i in range(6)}}
        samples = {name: ["A", "B"] for name in spec["dimensions"]}
        questions = coverage.corpus(spec, samples, "usage_date", per_class=12)
        self.assertGreaterEqual(sum(len(v) for v in questions.values()), 120)

    def test_the_class_list_is_the_products_own(self):
        self.assertEqual([name for name, _ in coverage.CLASSES], list(classes.NAMES))


class OracleTests(unittest.TestCase):
    def test_expectations_write_their_own_statements(self):
        expected = coverage.expectations(SPEC, DATASET, SAMPLES)
        by_question = {e["question"]: e for e in expected}
        self.assertEqual(by_question["total users"]["sql"],
                         "SELECT COUNT(*) AS v FROM kaveon_product.usage")
        self.assertEqual(by_question["users in Desktop"]["sql"],
                         "SELECT COUNT(*) AS v FROM kaveon_product.usage WHERE platform = 'Desktop'")
        self.assertTrue(by_question["total locales"]["approximate"])
        self.assertEqual(by_question["users by platform"]["cell"], "Desktop")

    def test_an_exact_answer_must_match_and_an_approximate_one_has_a_bound(self):
        self.assertEqual(coverage._compare(100.0, 100.0, False), "right")
        self.assertEqual(coverage._compare(101.0, 100.0, False), "wrong")
        self.assertEqual(coverage._compare(103.0, 100.0, True), "right")
        self.assertEqual(coverage._compare(120.0, 100.0, True), "wrong")
        self.assertEqual(coverage._compare(None, 100.0, False), "unchecked")

    def test_a_breakdown_is_checked_at_its_named_cell(self):
        answer = {"rows": [["Web", 5], ["Desktop", 9]]}
        self.assertEqual(coverage._headline(answer, "Desktop"), 9.0)
        self.assertEqual(coverage._headline(answer, None), 5.0)
        self.assertIsNone(coverage._headline(answer, "Mobile"))


class ReportTests(unittest.TestCase):
    def _row(self, **kw):
        base = {"asked": 0, "answered": 0, "clarified": 0, "refused": 0, "failed": 0,
                "context": 0, "cache": 0, "live": 0, "checked": 0, "wrong": 0,
                "misclassified": 0, "questions": []}
        base.update(kw)
        return base

    def test_the_report_renders_a_table_and_a_summary_line(self):
        report = {"dataset_id": "2", "dataset_name": "Product users", "wrong_answers": [],
                  "classes": {
                      "total": self._row(asked=1, answered=1, context=1, checked=1),
                      "out_of_scope": self._row(asked=2, refused=2)}}
        text = coverage.render(report, markdown=True)
        self.assertIn("| total | 1 | 1 | 0 | 0 | 0 | 1 | 0 | 0 | 1 | 0 |", text)
        self.assertIn("| **all** | 3 | 1 | 0 | 2 | 0 | 1 | 0 | 0 | 1 | 0 |", text)
        self.assertIn("Answered 1 of 3 (33.3%); from context 1 of 1 answered (100.0%)", text)
        self.assertIn("checked 1; wrong 0", text)

    def test_a_class_below_ninety_percent_is_listed_as_open_not_hidden(self):
        report = {"classes": {
            "breakdown": self._row(asked=10, answered=8, context=8),
            "filter": self._row(asked=10, answered=10, context=10),
            "top_n": self._row(asked=4, answered=4, checked=2, wrong=1),
            "out_of_scope": self._row(asked=2, answered=1, refused=1)}}
        self.assertEqual(coverage.open_classes(report), [
            ("breakdown", "8 of 10 answered"),
            ("out_of_scope", "answered 1 it should have held"),
            ("top_n", "1 wrong of 2 checked"),
        ])
        text = coverage.render({**report, "wrong_answers": []}, markdown=False)
        self.assertIn("Open (below 90% answered, or with a wrong answer)", text)


class ClassifyTests(unittest.TestCase):
    def test_outcomes_are_counted_from_the_evidence_lane(self):
        outcome = coverage.Outcome()
        self.assertEqual(coverage.classify({"ok": True, "evidence": {"lane": "context"}}, outcome), "context")
        self.assertEqual(coverage.classify({"ok": True, "evidence": {"lane": "cache"}}, outcome), "cache")
        self.assertEqual(coverage.classify({"ok": True, "from_context": False}, outcome), "live")
        self.assertEqual(coverage.classify({"ok": False, "reason": "clarify"}, outcome), "clarify")
        self.assertEqual(coverage.classify({"ok": False, "reason": "unanswerable"}, outcome), "unanswerable")
        self.assertEqual(coverage.classify({"ok": False, "reason": "query_failed"}, outcome), "failed")
        self.assertEqual((outcome.asked, outcome.answered, outcome.context, outcome.cache, outcome.live,
                          outcome.clarified, outcome.refused, outcome.failed), (6, 3, 1, 1, 1, 1, 1, 1))


if __name__ == "__main__":
    unittest.main()
