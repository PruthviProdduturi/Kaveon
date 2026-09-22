"""Clarify, never guess.

The rules, one test each:

1. A word that resolves to no indexed value is a question with the nearest
   indexed values as its options — never a silently different answer.
2. A word that is close to a measure or a dimension asks about that instead.
3. A word that is close to nothing refuses, says why, and offers the closest
   questions this dataset *can* answer.
4. A value that lives in two columns, two measures that read a question
   equally well, and two dimensions that read a breakdown equally well each
   ask which.
5. A breakdown by a column that is not a dimension asks which dimension.
6. Every answered number still carries its evidence.
"""
import unittest
from unittest.mock import patch

from dlm import engine
from dlm.test_ask_dialogue import ProductUsersHarness


class NeverGuessTests(unittest.TestCase):
    def test_a_near_miss_value_asks_with_the_nearest_indexed_values(self):
        with ProductUsersHarness():
            result = engine.ask("users in finance")
        self.assertEqual(result["reason"], "clarify")
        self.assertEqual(result["question_class"], "clarify_value")
        self.assertEqual(result["clarification"]["options"][0]["id"], "industry=Financial Services")
        self.assertEqual(result["clarification"]["options"][-1]["id"], "skip")

    def test_a_typo_the_lexicon_can_fix_is_fixed_before_anything_is_asked(self):
        # The cheapest clarification is the one that is not needed.
        with ProductUsersHarness():
            result = engine.ask("users by deploymnt")
        self.assertTrue(result["ok"], result)
        self.assertEqual(result["frame"]["group_col"], "deployment")

    def test_a_word_close_to_a_measure_asks_about_the_measure(self):
        # Past the lexicon's edit-distance cap, so it reaches the unresolved path.
        with ProductUsersHarness(), patch.object(engine, "_fuzzy_question", lambda q, v: q):
            result = engine.ask("users in localles")
        self.assertEqual(result["reason"], "clarify")
        self.assertEqual(result["question_class"], "clarify_metric")
        self.assertEqual([o["id"] for o in result["clarification"]["options"]], ["Locales"])
        self.assertIn("is not a measure of Product users", result["clarification"]["prompt"])

    def test_a_word_close_to_a_dimension_asks_about_the_dimension(self):
        with ProductUsersHarness(), patch.object(engine, "_fuzzy_question", lambda q, v: q):
            result = engine.ask("users in deploymnts")
        self.assertEqual(result["reason"], "clarify")
        self.assertEqual(result["question_class"], "clarify_dimension")
        self.assertEqual([o["id"] for o in result["clarification"]["options"]], ["deployment"])

    def test_a_word_close_to_nothing_refuses_and_offers_what_it_can_answer(self):
        with ProductUsersHarness():
            result = engine.ask("users in zzyzx")
        self.assertFalse(result["ok"])
        self.assertEqual(result["reason"], "unanswerable")
        self.assertEqual(result["question_class"], "unanswerable")
        self.assertIn('"zzyzx" is not a value, a measure or a dimension of Product users',
                      result["why"])
        self.assertEqual(result["closest"],
                         ["total users", "users by platform", "top 5 platform by users"])
        self.assertIn("The closest it can answer", result["answer"])

    def test_a_refusal_never_carries_a_number(self):
        with ProductUsersHarness():
            result = engine.ask("users in zzyzx")
        self.assertNotIn("rows", result)
        self.assertNotIn("sql", result)
        self.assertNotIn("evidence", result)


class AmbiguityTests(unittest.TestCase):
    def test_a_value_in_two_columns_asks_which_column(self):
        with ProductUsersHarness():
            result = engine.ask("users in Enterprise")
        self.assertEqual(result["reason"], "clarify")
        self.assertEqual(result["clarification"]["kind"], "value")
        self.assertGreater(len(result["clarification"]["options"]), 1)

    def test_two_dimensions_that_read_the_breakdown_equally_well_ask_which(self):
        with ProductUsersHarness():
            result = engine.ask("users by deployment and license and platform")
        self.assertEqual(result["reason"], "clarify")
        self.assertEqual(result["question_class"], "clarify_dimension")

    def test_a_breakdown_by_a_non_dimension_column_asks_which_dimension(self):
        with ProductUsersHarness():
            result = engine.ask("users by locale")
        self.assertEqual(result["reason"], "clarify")
        self.assertEqual(result["question_class"], "clarify_dimension")
        self.assertIn("not one of its dimensions", result["clarification"]["prompt"])

    def test_resuming_with_a_pinned_choice_answers_without_asking_again(self):
        with ProductUsersHarness():
            result = engine.ask("users in finance",
                                choices={"value_phrase": "finance",
                                         "value": "industry=Financial Services"})
        self.assertTrue(result["ok"], result)
        self.assertEqual(result["frame"]["filters"],
                         [{"column": "industry", "value": "Financial Services"}])

    def test_leaving_the_word_out_is_the_user_s_choice_not_the_dlm_s(self):
        with ProductUsersHarness():
            result = engine.ask("users in finance",
                                choices={"value_phrase": "finance", "value": "skip"})
        self.assertTrue(result["ok"], result)
        self.assertEqual(result["note"], '"finance" was left out of the answer.')


class EvidenceTests(unittest.TestCase):
    def test_every_answered_number_keeps_its_evidence(self):
        with ProductUsersHarness():
            result = engine.ask("users by platform")
        self.assertTrue(result["ok"], result)
        evidence = result["evidence"]
        for key in ("sql", "dataset", "source", "source_version", "lane", "reproduce"):
            self.assertIn(key, evidence)
        self.assertTrue(evidence["reproduce"]["sql"])

    def test_the_closest_questions_come_from_the_spec_not_a_fixed_list(self):
        spec = {"default_metric": "Revenue", "time": {"latest": "2026-08"}}
        self.assertEqual(
            engine._closest_questions(spec, ["region", "country"], []),
            ["total revenue", "revenue by region", "top 5 region by revenue"])
        self.assertEqual(engine._closest_questions({}, [], []), [])

    def test_near_names_is_bounded_not_a_free_association(self):
        self.assertEqual(engine._near_names("localez", ["Locales", "Users"], {}), ["Locales"])
        self.assertEqual(engine._near_names("sprockets", ["Locales", "Users"], {}), [])
        self.assertEqual(engine._near_names("ab", ["Locales"], {}), [])


if __name__ == "__main__":
    unittest.main()
