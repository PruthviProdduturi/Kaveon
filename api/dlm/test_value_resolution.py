"""`exact_only` must bar the close match, not only the prefix match.

"chart" is one deleted letter from "Chat", a surface of the events table. The
entity-filter path asks for exact matching precisely so an ordinary word
cannot become a filter, but the close-match fallback ran regardless of that
flag — so almost any request to draw something quietly filtered the answer to
`surface = 'Chat'`:

    "Show me a chart of that"      -> Charts Created - Chat is 0
    "can i get a world map chart"  -> Charts Created - Chat is 0

Two different questions, the same wrong answer, and nothing said a filter had
been invented. The ask-path harness stubs `resolve_value` with a version that
never had the close match, which is why no existing test saw this — these
exercise the real function.
"""
import sys
import unittest
from types import SimpleNamespace
from unittest.mock import patch

if "pyodbc" not in sys.modules:
    sys.modules["pyodbc"] = SimpleNamespace(Error=Exception)

from dlm import engine

# One indexed value, the one that caused the trouble.
CHAT = {"element_key": "public.kaveon_events_enriched.surface", "value_text": "Chat",
        "value_norm": "chat", "key_column": "surface", "key_value": "Chat", "freq": 9001}


def _rows_for(norm_wanted):
    """Stand in for the value index: an exact lookup hits only on its own
    normalized value, and the prefix lookup is a LIKE the test never needs."""
    def query(sql, params):
        if "value_norm = @param1" in sql and params[1] == norm_wanted:
            return {"rows": [CHAT]}
        return {"rows": []}
    return query


class ExactOnlyTests(unittest.TestCase):
    def _resolve(self, term, *, exact_only, indexed=(CHAT,)):
        with patch.object(engine, "ensure_tables", lambda: None), \
             patch.object(engine.meta, "query", side_effect=_rows_for("chat")), \
             patch.object(engine, "_indexed_values", lambda dataset_id: list(indexed)):
            return engine.resolve_value("2", term, exact_only=exact_only)

    def test_a_charting_word_never_becomes_a_filter(self):
        self.assertEqual(self._resolve("chart", exact_only=True), [])

    def test_an_anaphor_never_becomes_a_filter(self):
        """"Show me a chart of that" — "that" is one substitution from "chat"."""
        self.assertEqual(self._resolve("that", exact_only=True), [])

    def test_the_value_itself_still_resolves_under_exact_only(self):
        hits = self._resolve("chat", exact_only=True)
        self.assertEqual([hit.get("value") for hit in hits], ["Chat"])

    def test_a_close_match_still_serves_the_paths_that_ask_for_one(self):
        """Only the entity-filter path sets exact_only. Everything else keeps
        the close match, so a genuine misspelling is still recognised."""
        hits = self._resolve("chatt", exact_only=False)
        self.assertEqual([hit.get("value") for hit in hits], ["Chat"])

    def test_a_term_that_resembles_nothing_resolves_to_nothing(self):
        for exact_only in (True, False):
            with self.subTest(exact_only=exact_only):
                self.assertEqual(self._resolve("zzyzx", exact_only=exact_only), [])


class EntityFilterTests(unittest.TestCase):
    """The same thing one level up: the filters a question produces."""

    def _filters(self, question):
        with patch.object(engine, "ensure_tables", lambda: None), \
             patch.object(engine, "_effective_spec", lambda dataset_id: {}), \
             patch.object(engine.meta, "query", side_effect=_rows_for("chat")), \
             patch.object(engine, "_indexed_values", lambda dataset_id: [CHAT]):
            return engine._resolve_entity_filters("2", question)

    def test_asking_for_a_chart_does_not_filter_the_answer(self):
        for question in ("Show me a chart of that",
                         "can i get a world map chart",
                         "chart the top countries"):
            with self.subTest(question=question):
                self.assertEqual(self._filters(question), [])

    def test_naming_the_surface_still_filters_on_it(self):
        filters = self._filters("how many sessions on chat")
        self.assertEqual([(f["column"], f["value"]) for f in filters], [("surface", "Chat")])


if __name__ == "__main__":
    unittest.main()
