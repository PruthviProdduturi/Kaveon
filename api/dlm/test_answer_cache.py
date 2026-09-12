"""Precomputed answers are loaded one at a time and cached under a byte bound.
A dataset with thousands of multi-dimension cuboids must never be pulled into
memory whole on the ask path."""
import json
import sys
import unittest
from types import SimpleNamespace
from unittest.mock import patch

if "pyodbc" not in sys.modules:
    sys.modules["pyodbc"] = SimpleNamespace(Error=Exception)

from dlm import engine


def _row(metric, group, n_rows):
    rows = [[f"v{i}", i] for i in range(n_rows)]
    return {"metric_name": metric, "group_col": group, "columns": json.dumps(["g", metric]), "rows": json.dumps(rows)}


class FakeMeta:
    """Answers table with a query log, so the tests can see what was fetched."""
    def __init__(self, rows):
        self.rows = rows
        self.log = []

    def query(self, sql, params):
        self.log.append((sql, list(params)))
        did = params[0]
        if "metric_name = @param1" in sql:
            hits = [r for r in self.rows if r["metric_name"] == params[1] and r["group_col"] == params[2]]
            return {"rows_objects": [dict(r, dataset_id=did) for r in hits]}
        if "SELECT metric_name, group_col FROM dlm_answers" in sql:
            return {"rows_objects": [{"metric_name": r["metric_name"], "group_col": r["group_col"]} for r in self.rows]}
        raise AssertionError(f"unexpected query: {sql}")


class AnswerCacheTests(unittest.TestCase):
    def setUp(self):
        engine._evict_answers()

    def test_one_answer_is_fetched_by_key_not_the_whole_dataset(self):
        meta = FakeMeta([_row("Trips", "", 1), _row("Trips", "borough", 5)] + [_row("ms", f"c{i}|d{i}", 2000) for i in range(300)])
        with patch.object(engine, "meta", meta), patch.object(engine, "ensure_tables", lambda: None):
            hit = engine._context_answer("23", "Trips", "borough")
            again = engine._context_answer("23", "Trips", "borough")
        self.assertEqual(len(hit["rows"]), 5)
        self.assertIs(hit, again)
        self.assertEqual(len(meta.log), 1)
        self.assertIn("metric_name = @param1 AND group_col = @param2", meta.log[0][0])

    def test_missing_answer_is_none_and_not_retried_from_cache(self):
        meta = FakeMeta([_row("Trips", "", 1)])
        with patch.object(engine, "meta", meta), patch.object(engine, "ensure_tables", lambda: None):
            self.assertIsNone(engine._context_answer("23", "Trips", "zone"))
            self.assertIsNone(engine._context_answer("23", "Trips", "zone"))
        self.assertEqual(len(meta.log), 1)

    def test_cache_is_bounded_by_bytes(self):
        big = [_row("ms", f"c{i}", 4000) for i in range(40)]      # ~50 KB each serialized
        meta = FakeMeta(big)
        with patch.object(engine, "meta", meta), patch.object(engine, "ensure_tables", lambda: None), \
             patch.object(engine, "_ANSWER_CACHE_LIMIT_BYTES", 200_000):
            for i in range(40):
                engine._context_answer("23", "ms", f"c{i}")
            self.assertLessEqual(engine._answer_cache_bytes(), 200_000)
            self.assertLess(len(engine._ANSWER_CACHE), 40)
            # the most recent entries survive; the oldest were evicted
            self.assertIn(("23", "ms", "c39"), engine._ANSWER_CACHE)
            self.assertNotIn(("23", "ms", "c0"), engine._ANSWER_CACHE)

    def test_dataset_eviction_only_drops_that_dataset(self):
        meta = FakeMeta([_row("Trips", "", 1)])
        with patch.object(engine, "meta", meta), patch.object(engine, "ensure_tables", lambda: None):
            engine._context_answer("23", "Trips", "")
            engine._context_answer("24", "Trips", "")
            engine._evict_answers("23")
        self.assertNotIn(("23", "Trips", ""), engine._ANSWER_CACHE)
        self.assertIn(("24", "Trips", ""), engine._ANSWER_CACHE)

    def test_answer_keys_lists_without_loading_rows(self):
        meta = FakeMeta([_row("Trips", "", 1), _row("Trips", "borough", 5)])
        with patch.object(engine, "meta", meta), patch.object(engine, "ensure_tables", lambda: None):
            keys = engine._answer_keys("23")
        self.assertEqual(keys, [("Trips", ""), ("Trips", "borough")])
        self.assertNotIn("rows", meta.log[0][0].split("FROM")[0])


if __name__ == "__main__":
    unittest.main()
