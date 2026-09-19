import unittest

from dlm import coverage

SPEC = {
    "metrics": {"Users": {"additive": True, "default": True},
                "Locales": {"additive": False, "approximate": True},
                "Hidden": {"additive": True, "hidden": True}},
    "dimensions": {"platform": {}, "country": {}, "secret": {"hidden": True}},
    "default_metric": "Users",
}
SAMPLES = {"platform": ["Desktop", "Web"], "country": ["India", "Japan"]}


class CorpusTests(unittest.TestCase):
    def test_the_corpus_follows_the_spec_and_skips_hidden_elements(self):
        questions = coverage.corpus(SPEC, SAMPLES, None, per_class=8)
        self.assertEqual(questions["total"], ["total users"])
        self.assertEqual(questions["breakdown"], ["users by platform", "users by country"])
        self.assertEqual(questions["filter"], ["users in Desktop", "users in India"])
        self.assertEqual(questions["filter_breakdown"], ["users by country in Desktop"])
        self.assertEqual(questions["two_filters"], ["users in Desktop Japan"])
        self.assertEqual(questions["top_n"], ["top 3 platform by users", "top 3 country by users"])
        self.assertEqual(questions["distinct_total"], ["total locales"])
        self.assertEqual(questions["distinct_breakdown"], ["locales by platform", "locales by country"])
        self.assertNotIn("year", questions)                       # no date column
        self.assertEqual(len(questions["out_of_scope"]), 2)
        with_dates = coverage.corpus(SPEC, SAMPLES, "event_date", per_class=2)
        self.assertEqual(with_dates["year"], ["users in 2024", "users in 2025"])
        self.assertEqual(with_dates["trend"], ["users over time", "users by month"])

    def test_outcomes_are_counted_from_the_evidence_lane(self):
        outcome = coverage.Outcome()
        self.assertEqual(coverage.classify({"ok": True, "evidence": {"lane": "context"}}, outcome), "context")
        self.assertEqual(coverage.classify({"ok": True, "evidence": {"lane": "cache"}}, outcome), "cache")
        self.assertEqual(coverage.classify({"ok": True, "from_context": False}, outcome), "live")
        self.assertEqual(coverage.classify({"ok": False, "reason": "clarify"}, outcome), "clarify")
        self.assertEqual(coverage.classify({"ok": False, "reason": "out_of_scope"}, outcome), "out_of_scope")
        self.assertEqual(coverage.classify({"ok": False, "reason": "query_failed"}, outcome), "failed")
        self.assertEqual((outcome.asked, outcome.answered, outcome.context, outcome.cache, outcome.live,
                          outcome.clarified, outcome.refused, outcome.failed), (6, 3, 1, 1, 1, 1, 1, 1))

    def test_the_report_renders_a_table_and_a_summary_line(self):
        report = {"dataset_id": "2", "dataset_name": "Product users", "classes": {
            "total": {"asked": 1, "answered": 1, "clarified": 0, "refused": 0, "failed": 0,
                      "context": 1, "cache": 0, "live": 0, "questions": []},
            "out_of_scope": {"asked": 2, "answered": 0, "clarified": 0, "refused": 2, "failed": 0,
                             "context": 0, "cache": 0, "live": 0, "questions": []}}}
        text = coverage.render(report, markdown=True)
        self.assertIn("| total | 1 | 1 | 0 | 0 | 0 | 1 | 0 | 0 |", text)
        self.assertIn("| **all** | 3 | 1 | 0 | 2 | 0 | 1 | 0 | 0 |", text)
        self.assertIn("Answered 1 of 3 (33.3%); from context 1 of 1 answered (100.0%)", text)


if __name__ == "__main__":
    unittest.main()
