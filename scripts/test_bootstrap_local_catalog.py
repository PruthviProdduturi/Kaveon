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


class BootstrapTests(unittest.TestCase):
    def test_checked_in_mirror_discovers_all_showcase_tables(self):
        tables = bootstrap.discover(ROOT / "data" / "adls-mirror" / "opensource" / "showcase")
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
