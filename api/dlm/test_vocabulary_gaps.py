"""A refusal is the curation request, and it writes itself down.

A dataset's structure is derived: dimensions, measures, time grain and value
index all come from the Engine's statistics. Its organisation's vocabulary
cannot be derived from anything -- that `rev_net_usd` is "the number", that
"churn" here means cancelled accounts -- and it used to be invisible, because
an unmapped word was answered with whatever the default measure was.

Now it refuses by name, which turns every gap into a signal carried by a real
question someone asked. These tests cover the three things that makes useful:
the ledger that collects the words, the queue that offers what each could
mean, and the resolution that makes the answer stick.

The resolution is the part worth being careful about. A mapping is an ordinary
curated alias, written exactly as the context page writes one, which is why it
needs no new storage and why it works: an alias joins the dataset's
vocabulary, and the vocabulary is both what the router matches against and
what decides whether a word can be placed at all. One test asks a question
before and after a mapping to prove that end to end.
"""

import unittest
from unittest.mock import patch

from dlm import engine, vocabulary_gaps


class TheLedgerCollectsWords(unittest.TestCase):
    def setUp(self):
        vocabulary_gaps.clear()
        self.addCleanup(vocabulary_gaps.clear)

    def test_a_word_is_recorded_with_the_question_that_carried_it(self):
        vocabulary_gaps.record("7", "churn", "churn by region", "a@example.test")
        row = vocabulary_gaps.gaps()[0]
        self.assertEqual((row["dataset_id"], row["term"], row["count"]), ("7", "churn", 1))
        self.assertEqual(row["examples"], ["churn by region"])
        self.assertEqual(row["askers"], 1)

    def test_the_same_word_asked_again_counts_rather_than_repeats(self):
        for _ in range(3):
            vocabulary_gaps.record("7", "churn", "churn by region", "a@example.test")
        rows = vocabulary_gaps.gaps()
        self.assertEqual(len(rows), 1)
        self.assertEqual(rows[0]["count"], 3)
        self.assertEqual(rows[0]["examples"], ["churn by region"],
                         "the same question is one example, not three")

    def test_distinct_askers_are_counted_separately_from_asks(self):
        """One person asking five times is a preference; five people asking
        once is a vocabulary gap."""
        for who in ("a@example.test", "b@example.test", "a@example.test"):
            vocabulary_gaps.record("7", "churn", "churn by region", who)
        row = vocabulary_gaps.gaps()[0]
        self.assertEqual((row["count"], row["askers"]), (3, 2))

    def test_the_most_asked_word_comes_first(self):
        vocabulary_gaps.record("7", "rare", "rare by region")
        for _ in range(4):
            vocabulary_gaps.record("7", "common", "common by region")
        self.assertEqual([r["term"] for r in vocabulary_gaps.gaps()], ["common", "rare"])

    def test_the_same_word_on_two_datasets_is_two_gaps(self):
        """Each dataset needs its own mapping -- "churn" may be a measure on one
        and genuinely absent from the other."""
        vocabulary_gaps.record("7", "churn", "churn by region")
        vocabulary_gaps.record("9", "churn", "churn by region")
        self.assertEqual(len(vocabulary_gaps.gaps()), 2)

    def test_only_the_named_datasets_are_listed(self):
        """The questions people ask are as visibility-bound as the datasets."""
        vocabulary_gaps.record("7", "churn", "q")
        vocabulary_gaps.record("9", "attrition", "q")
        self.assertEqual([r["term"] for r in vocabulary_gaps.gaps(["7"])], ["churn"])

    def test_a_dismissed_word_is_filtered_on_read(self):
        """The dismissal is durable and the ledger is not, so it is applied
        when the queue is read rather than trusted to stay deleted."""
        vocabulary_gaps.record("7", "asdf", "q")
        vocabulary_gaps.record("7", "churn", "q")
        rows = vocabulary_gaps.gaps(["7"], {"7": ["asdf"]})
        self.assertEqual([r["term"] for r in rows], ["churn"])

    def test_the_ledger_is_bounded_and_evicts_the_least_recent(self):
        for i in range(vocabulary_gaps.MAX_TERMS + 10):
            vocabulary_gaps.record("7", f"word{i:04d}", "q")
        rows = vocabulary_gaps.gaps()
        self.assertEqual(len(rows), vocabulary_gaps.MAX_TERMS)
        self.assertNotIn("word0000", [r["term"] for r in rows])

    def test_examples_are_capped_and_truncated(self):
        for i in range(6):
            vocabulary_gaps.record("7", "churn", f"question number {i} " + "x" * 400)
        row = vocabulary_gaps.gaps()[0]
        self.assertEqual(len(row["examples"]), vocabulary_gaps.MAX_EXAMPLES)
        self.assertTrue(all(len(e) <= vocabulary_gaps.MAX_EXAMPLE_LENGTH for e in row["examples"]))

    def test_a_blank_term_or_dataset_is_not_recorded(self):
        vocabulary_gaps.record("", "churn", "q")
        vocabulary_gaps.record("7", "   ", "q")
        self.assertEqual(vocabulary_gaps.gaps(), [])

    def test_recording_never_raises(self):
        """This runs on the refusal path: a question that could not be answered
        must still be refused cleanly if the ledger cannot take the note."""
        with patch.object(vocabulary_gaps, "_GAPS", None):
            vocabulary_gaps.record("7", "churn", "q")   # must not raise

    def test_forgetting_a_resolved_word_removes_it(self):
        vocabulary_gaps.record("7", "churn", "q")
        self.assertTrue(vocabulary_gaps.forget("7", "CHURN"), "the term is matched case-insensitively")
        self.assertEqual(vocabulary_gaps.gaps(), [])

    def test_forgetting_a_dataset_removes_only_its_words(self):
        vocabulary_gaps.record("7", "churn", "q")
        vocabulary_gaps.record("9", "churn", "q")
        self.assertEqual(vocabulary_gaps.forget_dataset("7"), 1)
        self.assertEqual([r["dataset_id"] for r in vocabulary_gaps.gaps()], ["9"])


SALES = {
    "id": "2", "dataset_name": "Sales", "database_name": "OpenSource",
    "schema_name": "kaveon_product", "table_name": "sales", "date_column": None,
    "columns": [
        {"column_name": "region", "is_dimension": True, "is_metric": False},
        {"column_name": "revenue", "is_dimension": False, "is_metric": True},
    ],
    "metrics": [{"name": "Revenue", "expression": "SUM(revenue)"}],
}


def _artifact(curation=None):
    spec = engine._suggest_spec(SALES["columns"], SALES["metrics"])
    return {"dataset_id": "2", "status": "ready", "built_at": "2026-11-27T00:00:00",
            "manifest": {"name": "Sales", "context_spec": spec},
            "compiled_context": {"curation": curation or {}}}


class TheQueueOffersWhatAWordCouldMean(unittest.TestCase):
    """A word the router could not place is exactly the word no automatic
    suggestion should be trusted on, so the queue offers the dataset's
    measures and dimensions in full and lets a person choose."""

    def setUp(self):
        vocabulary_gaps.clear()
        self.addCleanup(vocabulary_gaps.clear)

    def _queue(self, artifact=None):
        with patch.object(engine, "_serving_artifacts", return_value=[artifact or _artifact()]):
            return engine.vocabulary_queue("admin@example.test", "Admin")

    def test_a_recorded_word_appears_with_its_dataset_name_and_targets(self):
        vocabulary_gaps.record("2", "churn", "churn by region", "a@example.test")
        queue = self._queue()
        self.assertEqual(queue["terms"], 1)
        row = queue["gaps"][0]
        self.assertEqual(row["dataset_name"], "Sales")
        self.assertEqual(row["measures"], ["Revenue"])
        self.assertEqual(row["dimensions"], ["region"])

    def test_a_near_miss_is_offered_as_the_closest_match(self):
        """A typo is a different answer from a missing word, so the queue says
        which it looks like."""
        vocabulary_gaps.record("2", "revenu", "revenu by region")
        self.assertEqual(self._queue()["gaps"][0]["closest"], ["Revenue"])

    def test_a_dismissed_word_is_not_in_the_queue(self):
        vocabulary_gaps.record("2", "asdf", "asdf by region")
        queue = self._queue(_artifact({"dismissed_terms": ["asdf"]}))
        self.assertEqual(queue["terms"], 0)

    def test_the_queue_says_its_observations_are_process_local(self):
        """The limitation is stated by the API rather than left to be
        discovered: a reader of this queue must know a restart empties it."""
        self.assertIn("restart", self._queue()["note"])

    def test_a_read_without_an_identity_reports_that_rather_than_an_empty_queue(self):
        result = engine.vocabulary_queue()
        self.assertEqual(result["terms"], 0)
        self.assertIn("identity", result["skipped"])


class ResolvingAWordIsDurable(unittest.TestCase):
    def setUp(self):
        vocabulary_gaps.clear()
        self.addCleanup(vocabulary_gaps.clear)
        self.saved = []

    def _capture(self, dataset_id, curation, actor, role="Viewer"):
        """Stand in for the publish, recording what would have been written."""
        self.saved.append((curation, role))
        return {"ok": True, "needs_regenerate": False}

    def curation(self, index):
        """The curation document of the n-th save."""
        return self.saved[index][0]

    def _resolve(self, curation=None, **kwargs):
        artifact = _artifact(curation)
        spec = engine.get_context_spec
        with patch.object(engine, "get_dlm", lambda i, a, r="Viewer": artifact), \
             patch.object(engine, "save_curation", self._capture):
            with patch.object(engine, "get_context_spec", spec):
                return engine.resolve_vocabulary_gap(
                    "2", actor="admin@example.test", **kwargs)

    def test_mapping_a_word_writes_it_as_an_alias_of_that_measure(self):
        result = self._resolve(term="churn", kind="metric", name="Revenue")
        self.assertTrue(result["ok"], result)
        self.assertIn("churn", self.curation(0)["metrics"]["Revenue"]["aliases"])

    def test_the_callers_role_reaches_the_publish(self):
        """A seeded dataset is owned by `system`, which nobody can sign in as,
        so only an Admin may publish for it. Curation published as a Viewer
        regardless of the caller, and every save on such a dataset failed --
        the context editor included."""
        self._resolve(term="churn", kind="metric", name="Revenue", role="Admin")
        self.assertEqual(self.saved[0][1], "Admin")

    def test_mapping_keeps_the_aliases_already_in_force(self):
        """Curation replaces rather than extends, so writing back only the new
        word would silently drop the derived synonyms."""
        self._resolve(term="churn", kind="metric", name="Revenue")
        aliases = self.curation(0)["metrics"]["Revenue"]["aliases"]
        self.assertIn("sales", aliases, "a derived synonym was dropped")

    def test_mapping_a_word_to_a_dimension_writes_it_there(self):
        self._resolve(term="territory", kind="dimension", name="region")
        self.assertIn("territory", self.curation(0)["dimensions"]["region"]["aliases"])

    def test_dismissing_records_the_judgement_and_maps_nothing(self):
        result = self._resolve(term="asdf", dismiss=True)
        self.assertTrue(result["ok"], result)
        self.assertEqual(self.curation(0)["dismissed_terms"], ["asdf"])
        self.assertNotIn("metrics", self.curation(0))

    def test_dismissing_adds_to_the_words_already_dismissed(self):
        self._resolve({"dismissed_terms": ["zzz"]}, term="asdf", dismiss=True)
        self.assertEqual(self.curation(0)["dismissed_terms"], ["asdf", "zzz"])

    def test_a_resolved_word_leaves_the_queue(self):
        vocabulary_gaps.record("2", "churn", "churn by region")
        self._resolve(term="churn", kind="metric", name="Revenue")
        self.assertEqual(vocabulary_gaps.gaps(), [])

    def test_a_target_the_dataset_does_not_have_is_refused(self):
        result = self._resolve(term="churn", kind="metric", name="Imaginary")
        self.assertFalse(result["ok"])
        self.assertEqual(result["reason"], "unknown_target")
        self.assertEqual(self.saved, [], "nothing is written for a bad target")

    def test_a_mapping_with_no_target_is_refused(self):
        self.assertEqual(self._resolve(term="churn")["reason"], "no_target")

    def test_a_blank_word_is_refused(self):
        self.assertEqual(self._resolve(term="   ", dismiss=True)["reason"], "no_term")


class DismissalsSurviveAContextEditorSave(unittest.TestCase):
    """The context page replaces the whole curation document -- that is what
    makes it WYSIWYG, since a suggested alias it did not send back is one the
    curator removed. No editor shows dismissed words, so a save carrying none
    of them means "I was not asked", not "clear them"."""

    def test_a_save_that_omits_dismissals_keeps_them(self):
        kept = engine._sanitize_curation({"metrics": {}, "dismissed_terms": ["asdf"]})
        self.assertEqual(kept["dismissed_terms"], ["asdf"])

    def test_the_sanitizer_lowercases_and_bounds_the_list(self):
        clean = engine._sanitize_curation({"dismissed_terms": [" ASDF ", "asdf", ""]})
        self.assertEqual(clean["dismissed_terms"], ["asdf"])

    def test_an_explicit_empty_list_is_how_a_caller_clears_them(self):
        self.assertNotIn("dismissed_terms", engine._sanitize_curation({"dismissed_terms": []}))


class TheRefusalRecordsItself(unittest.TestCase):
    """The ask path end to end: a refused question leaves a gap behind, and a
    mapped word stops being refused. Without this the ledger could be correct
    and never written to."""

    def setUp(self):
        vocabulary_gaps.clear()
        self.addCleanup(vocabulary_gaps.clear)

    def _ask(self, question, aliases=None):
        from dlm.test_measure_substitution import SalesHarness, SALES_DATASET
        dataset = SALES_DATASET
        if aliases:
            dataset = {**SALES_DATASET,
                       "metrics": [{"name": "Regions", "expression": "COUNT(DISTINCT region)",
                                    "metric_type": "count_distinct"}]}
        with SalesHarness() as harness:
            if aliases:
                spec = engine._suggest_spec(dataset["columns"], dataset["metrics"])
                spec["metrics"].setdefault("revenue", {"aliases": []})
                with patch.object(engine, "_effective_spec",
                                  lambda i: engine._merge_spec(spec, aliases)):
                    result = engine.ask(question)
            else:
                result = engine.ask(question)
        return result, harness.statements

    def test_a_refused_question_leaves_the_word_in_the_ledger(self):
        result, statements = self._ask("churn by region")
        self.assertFalse(result["ok"], result)
        self.assertEqual(statements, [])
        rows = vocabulary_gaps.gaps()
        self.assertEqual([r["term"] for r in rows], ["churn"])
        self.assertEqual(rows[0]["examples"], ["churn by region"])

    def test_an_answered_question_leaves_nothing_in_the_ledger(self):
        self._ask("revenue by region")
        self.assertEqual(vocabulary_gaps.gaps(), [])

    def test_the_asker_is_recorded_from_the_identity_ask_forwards(self):
        """`ask` installs the caller as serving state and forwards them as
        `principal`, leaving its own `actor` argument None on the re-entered
        call. Reading `actor` alone counted every refusal as anonymous."""
        from dlm.test_measure_substitution import SalesHarness
        with SalesHarness():
            engine.ask("churn by region", actor="reader@example.test", role="Admin")
        self.assertEqual(vocabulary_gaps.gaps()[0]["askers"], 1)

    def test_a_mapped_word_is_answered_rather_than_refused(self):
        """The whole point, and the load-bearing assumption of the feature: a
        mapping needs no new machinery because an alias is matched by the
        ranker itself. `_name_tokens` reads curated aliases, so "churn" anchors
        on `revenue` and a measure resolves -- which is why the refusal below
        it never runs, rather than running and being satisfied."""
        curation = {"metrics": {"revenue": {"aliases": ["churn"]}}}
        result, statements = self._ask("churn by region", aliases=curation)
        self.assertTrue(result["ok"], result)
        self.assertTrue(any("SUM(revenue)" in s for s in statements), statements)
        self.assertEqual(result["frame"]["metric"], "revenue",
                         "the alias must resolve the measure, not merely stop the refusal")


if __name__ == "__main__":
    unittest.main()
