"""Pair breakdowns come from packed cuboid scans: one GROUP BY over a few
low-cardinality dimensions, rolled up exactly per pair, so the scan budget
covers many more two-way questions than one scan per pair. Non-additive
metrics keep their direct per-pair scans."""
import re
import sys
import unittest
from types import SimpleNamespace
from unittest.mock import patch

if "pyodbc" not in sys.modules:
    sys.modules["pyodbc"] = SimpleNamespace(Error=Exception)

from dlm import engine

# A tiny fact table: (deployment, platform, country, actions, user_id)
FACTS = [
    ("Cloud", "Web", "India", 10, 1),
    ("Cloud", "Web", "India", 5, 2),
    ("Cloud", "Mobile", "India", 7, 1),
    ("OnPrem", "Web", "Germany", 3, 3),
    ("OnPrem", "Mobile", "Germany", 8, 3),
    ("OnPrem", "Mobile", "India", 1, 4),
]
DIMS = {"deployment": 0, "platform": 1, "country": 2}


def fake_engine(sql, database, timeout_seconds=None):
    """Evaluate the DLM's generated SQL against FACTS: totals, GROUP BY on one
    or more dims, SUM(actions) and COUNT(DISTINCT user_id)."""
    grouped = re.search(r"GROUP BY (.*?)(?: ORDER BY| LIMIT|$)", sql)
    keys = [k.strip().strip('"') for k in grouped.group(1).split(",")] if grouped else []
    groups = {}
    for row in FACTS:
        key = tuple(row[DIMS[k]] for k in keys)
        g = groups.setdefault(key, {"sum": 0, "users": set()})
        g["sum"] += row[3]
        g["users"].add(row[4])
    metrics = []
    if "SUM(actions)" in sql:
        metrics.append("sum")
    if "COUNT(DISTINCT user_id)" in sql:
        metrics.append("users")
    rows = []
    for key, g in groups.items():
        rows.append(list(key) + [g["sum"] if m == "sum" else len(g["users"]) for m in metrics])
    if "ORDER BY" in sql:
        rows.sort(key=lambda r: -r[len(keys)])
    limit = re.search(r"LIMIT (\d+)", sql)
    return {"rows": rows[: int(limit.group(1))] if limit else rows}


class CuboidPackingTests(unittest.TestCase):
    def test_the_cover_reaches_every_storable_pair_within_the_key_bound(self):
        card = {"a": 3, "b": 4, "c": 5, "d": 6, "e": 7, "f": 2000}
        with patch.object(engine, "_CUBOID_MAX_KEYS", 3):
            packed = engine._pack_cuboids(list(card), card, pair_cap=5000, budget=12)
        covered = {frozenset(p) for c in packed for p in engine._combinations(c, 2)}
        wanted = {frozenset(p) for p in engine._combinations(list("abcde"), 2)}
        self.assertEqual(covered, wanted)                            # all ten pairs among a..e
        self.assertTrue(all(len(c) <= 3 for c in packed))
        self.assertLessEqual(len(packed), 4)                         # ten pairs, three per scan
        self.assertFalse(any("f" in c for c in packed))              # f pairs with nothing under the cap

    def test_the_budget_bounds_the_scans_and_the_cell_cap_bounds_each_cuboid(self):
        card = {"a": 300, "b": 300, "c": 2}
        with patch.object(engine, "_CUBOID_CELL_CAP", 1000):
            packed = engine._pack_cuboids(list(card), card, pair_cap=100_000, budget=12)
            limited = engine._pack_cuboids(list(card), card, pair_cap=100_000, budget=1)
        self.assertEqual(packed, [["c", "a"], ["c", "b"]])          # a×b would be 90,000 cells: left to a direct scan
        self.assertEqual(limited, [["c", "a"]])


class CuboidRollupTests(unittest.TestCase):
    def _build(self, metrics):
        columns = [{"column_name": d, "is_dimension": True} for d in DIMS]
        stored = {}

        def store(dataset_id, metric, group, cols, rows, now):
            stored[(metric, group)] = rows

        with patch.object(engine.meta, "execute", lambda *a, **k: None), \
             patch.object(engine, "_execute_dataset_query", fake_engine), \
             patch.object(engine, "_store_answer", store), \
             patch.object(engine, "_build_sketch_cuboids", lambda *a, **k: 0):
            engine._precompute_answers("1", "OpenSource", "s", "events", columns, [], metrics, {}, report={})
        return stored

    def test_every_pair_is_exact_and_shaped_like_a_direct_scan(self):
        stored = self._build([{"name": "Total actions", "expression": "SUM(actions)"}])
        self.assertEqual(stored[("Total actions", "")], [[34]])
        pair = stored[("Total actions", "country|deployment")]
        self.assertEqual(pair[0], ["India", "Cloud", 22])                # ordered by the metric, descending
        self.assertEqual(sorted(pair), [["Germany", "OnPrem", 11], ["India", "Cloud", 22], ["India", "OnPrem", 1]])
        self.assertEqual(sorted(stored[("Total actions", "deployment|platform")]),
                         [["Cloud", "Mobile", 7], ["Cloud", "Web", 15], ["OnPrem", "Mobile", 9], ["OnPrem", "Web", 3]])
        self.assertTrue(all(isinstance(r[2], int) for r in pair))          # Int64 sums stay integers

    def test_one_scan_covers_all_pairs(self):
        calls = []

        def counting(sql, database, timeout_seconds=None):
            calls.append(sql)
            return fake_engine(sql, database, timeout_seconds)

        columns = [{"column_name": d, "is_dimension": True} for d in DIMS]
        with patch.object(engine.meta, "execute", lambda *a, **k: None), \
             patch.object(engine, "_execute_dataset_query", counting), \
             patch.object(engine, "_store_answer", lambda *a, **k: None), \
             patch.object(engine, "_build_sketch_cuboids", lambda *a, **k: 0):
            engine._precompute_answers("1", "OpenSource", "s", "events", columns, [],
                                       [{"name": "Total actions", "expression": "SUM(actions)"}], {}, report={})
        grouped = [c for c in calls if "GROUP BY" in c]
        self.assertEqual(len(grouped), 4)                                   # three single dims + one cuboid, no per-pair scans
        self.assertEqual(sum(1 for c in grouped if c.count(",") >= 2 and "AS g1" not in c), 1)

    def test_non_additive_metrics_still_get_direct_pair_scans(self):
        stored = self._build([{"name": "Total actions", "expression": "SUM(actions)"},
                              {"name": "Users", "expression": "COUNT(DISTINCT user_id)"}])
        users = stored[("Users", "country|deployment")]
        self.assertEqual(sorted(users), [["Germany", "OnPrem", 1], ["India", "Cloud", 2], ["India", "OnPrem", 1]])
        self.assertNotIn(("Users", "deployment|platform|country"), stored)   # cuboids never store a distinct count


if __name__ == "__main__":
    unittest.main()
