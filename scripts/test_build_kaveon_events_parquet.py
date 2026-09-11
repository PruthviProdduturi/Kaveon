"""The 504M-row build, rehearsed at 1,000 users: deterministic users from the
generate_usage.py pools, the same event columns and seeds as build_504m.py,
one row group per (day, surface) with exact statistics, and a manifest
register-curated-catalog.py can consume."""
import importlib.util
import json
import sys
import tempfile
import unittest
from pathlib import Path
from unittest.mock import patch

import numpy as np
import pyarrow.parquet as pq

SPEC = importlib.util.spec_from_file_location(
    "build_kaveon_events_parquet", Path(__file__).with_name("build-kaveon-events-parquet.py"))
mod = importlib.util.module_from_spec(SPEC)
sys.modules[SPEC.name] = mod
SPEC.loader.exec_module(mod)

N = 1000


def small(fn):
    def run(self):
        with tempfile.TemporaryDirectory() as tmp, \
             patch.object(mod, "N_USERS", N), \
             patch.object(mod, "UIDS", np.arange(1, N + 1, dtype=np.int64)), \
             patch.object(mod, "DAYS", 2):
            fn(self, Path(tmp))
    return run


class UsersTests(unittest.TestCase):
    @small
    def test_users_are_deterministic_and_geographically_consistent(self, out):
        first = mod.build_users(out).to_pydict()
        second = mod.build_users(out).to_pydict()
        self.assertEqual(first, second)
        self.assertEqual(first["user_id"][:3], [1, 2, 3])
        for country, region in zip(first["country"], first["region"]):
            self.assertEqual(mod.GEO[next(k for k, v in mod.GEO.items() if v[0] == country)][1], region)
        self.assertTrue(set(first["license"]) <= set(mod.POOLS["license"]))
        self.assertTrue(set(first["platform"]) <= {"Desktop", "Mobile", "Web"})
        # Weighted pools: the most-repeated values dominate.
        self.assertEqual(max(set(first["deployment"]), key=first["deployment"].count), "Cloud")


class BuildTests(unittest.TestCase):
    @small
    def test_small_build_matches_the_contract(self, out):
        mod.build(out)

        with pq.ParquetFile(out / "kaveon_product" / "kaveon_events_enriched" / "combined-v1.parquet") as events:
            self.check_events(events)
        with pq.ParquetFile(out / "kaveon_product" / "kaveon_events_users" / "combined-v1.parquet") as users:
            self.assertEqual(users.metadata.num_rows, N)
            self.assertEqual(users.schema_arrow.names[:2], ["user_id", "platform"])

        manifest = json.loads((out / "kaveon-events-singlefile-manifest.json").read_text())
        self.assertTrue(manifest["synthetic"])
        tables = {t["name"]: t for t in manifest["tables"]}
        self.assertEqual(tables["kaveon_events_enriched"]["row_count"], N * 12)
        self.assertEqual(tables["kaveon_events_users"]["row_count"], N)
        self.assertEqual(tables["kaveon_events_enriched"]["location"],
                         "kaveon_product/kaveon_events_enriched/combined-v1.parquet")
        types = {c["name"]: c["data_type"] for c in tables["kaveon_events_enriched"]["columns"]}
        self.assertEqual(types["event_date"], "Utf8")
        self.assertEqual(types["rows_scanned"], "Int64")
        self.assertEqual(types["country"], "Utf8")
        user_types = {c["name"]: c["data_type"] for c in tables["kaveon_events_users"]["columns"]}
        self.assertEqual(user_types["user_id"], "Int64")
        self.assertEqual(user_types["locale"], "Utf8")

    def check_events(self, events):
        self.assertEqual(events.metadata.num_rows, N * 2 * 6)
        self.assertEqual(events.metadata.num_row_groups, 12)
        names = events.schema_arrow.names
        self.assertEqual(names[:3], ["event_date", "user_id", "surface"])
        # No stored Arrow schema: readers get Utf8, not the writer's dictionary type.
        self.assertEqual(str(events.schema_arrow.field("event_date").type), "string")
        self.assertEqual(str(events.schema_arrow.field("country").type), "string")
        self.assertEqual(names[3:12], list(mod.METRICS))
        self.assertEqual(names[12:], list(mod.USER_DIMS))

        first = events.metadata.row_group(0)
        date_stats = first.column(0).statistics
        self.assertEqual((date_stats.min, date_stats.max), ("2026-07-04", "2026-07-04"))
        self.assertEqual(first.column(2).statistics.min, "Chat")
        last = events.metadata.row_group(11)
        self.assertEqual(last.column(0).statistics.max, "2026-07-05")
        self.assertEqual(last.column(2).statistics.max, "Export")

        rows = events.read_row_group(0).to_pydict()
        # Same formula as build_504m.py for day 0 / Chat / actions in (3, 15).
        expected = 3 + np.abs((np.arange(1, N + 1, dtype=np.int64) * (486187 + 31 + 1)) % 13)
        self.assertEqual(rows["actions"], expected.tolist())
        # Dimensions are the user's own, joined by user_id.
        users = mod.build_users(Path(tempfile.mkdtemp())).to_pydict()
        self.assertEqual(rows["country"][:5], users["country"][:5])
        self.assertEqual(rows["license"][:5], users["license"][:5])


if __name__ == "__main__":
    unittest.main()
