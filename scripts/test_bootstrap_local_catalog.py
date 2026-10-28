"""Regression tests for the PostgreSQL-free local catalog bootstrap."""
from __future__ import annotations

import importlib.util
import tempfile
import unittest
from pathlib import Path


ROOT = Path(__file__).resolve().parents[1]
SPEC = importlib.util.spec_from_file_location(
    "bootstrap_local_catalog", ROOT / "scripts" / "bootstrap-local-catalog.py"
)
assert SPEC and SPEC.loader
bootstrap = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(bootstrap)


SHOWCASE = ROOT / "data" / "adls-mirror" / "opensource" / "showcase"


class BootstrapTests(unittest.TestCase):
    # `data/adls-mirror/` is gitignored (.gitignore:152), so the mirror is not
    # checked in despite this test's original name: it is produced by the local
    # Docker stack. Erroring when it is absent made the whole file fail on a
    # fresh clone and in CI, which runs `pytest scripts`. Skipped instead, so
    # the assertion still holds wherever the local lake exists -- which is
    # where the plan's qualification gate runs it.
    @unittest.skipUnless(SHOWCASE.is_dir(),
                         "local ADLS mirror absent; run the local Docker stack first")
    def test_the_local_mirror_discovers_all_showcase_tables(self):
        tables = bootstrap.discover(SHOWCASE)
        self.assertEqual(len(tables), 25)
        self.assertIn(("public", "kaveon_events_dashboard", "parquet"), tables)

    def test_discovery_ignores_non_table_directories(self):
        with tempfile.TemporaryDirectory() as raw:
            root = Path(raw)
            schema = root / "public"
            (schema / "events").mkdir(parents=True)
            (schema / "events" / "part-000.parquet").write_bytes(b"not a real parquet")
            (schema / "empty").mkdir()
            (root / "not-a-schema!").mkdir()
            self.assertEqual(bootstrap.discover(root), [("public", "events", "parquet")])

    def test_identifier_guard(self):
        with self.assertRaises(ValueError):
            bootstrap.sql_ident("public;DROP", "schema")


if __name__ == "__main__":
    unittest.main()
