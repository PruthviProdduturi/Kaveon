import contextlib
import hashlib
import json
import sys
import unittest
from datetime import datetime, timezone
from types import SimpleNamespace
from unittest.mock import patch

if "pyodbc" not in sys.modules:
    sys.modules["pyodbc"] = SimpleNamespace(Error=Exception)

from services import postgresql_live_inventory as inventory
from services.postgresql_retirement_gate import AUTHORITY_FAMILIES


class Source:
    def __init__(self, tables, counts=None):
        self.tables = sorted(tables)
        self.counts = counts or {}
        self.statements = []

    def execute(self, sql, params=None):
        self.statements.append(" ".join(sql.split()))

    def query(self, sql, params=None):
        self.statements.append(" ".join(sql.split()))
        return {"rows": [{"table_name": table} for table in self.tables]}

    def query_one(self, sql, params=None):
        self.statements.append(" ".join(sql.split()))
        if "txid_current_snapshot" in sql:
            return {"snapshot": "10:20:"}
        table = sql.split('"')[1]
        return {"count": self.counts.get(table, 0)}


class LiveInventoryTests(unittest.TestCase):
    def collect(self, source):
        with patch.object(inventory.db, "transaction",
                          return_value=contextlib.nullcontext(source)):
            return inventory.collect(now=datetime(2026, 9, 11, tzinfo=timezone.utc))

    def test_collects_all_families_in_one_read_only_snapshot(self):
        tables = {table for values in AUTHORITY_FAMILIES.values() for table in values}
        source = Source(tables | {"product_migration_outbox"}, {"datasets": 9})
        report = self.collect(source)
        self.assertIn("REPEATABLE READ, READ ONLY", source.statements[0])
        self.assertEqual(len(report["authority_tables"]), len(tables))
        datasets = next(row for row in report["authority_tables"] if row["table"] == "datasets")
        self.assertEqual(datasets["row_count"], 9)
        self.assertEqual(report["infrastructure_tables"], ["product_migration_outbox"])
        unsigned = {key: value for key, value in report.items() if key != "report_sha256"}
        self.assertEqual(report["report_sha256"], hashlib.sha256(
            json.dumps(unsigned, sort_keys=True, separators=(",", ":"),
                       ensure_ascii=False).encode()).hexdigest())

    def test_absent_known_tables_are_explicit_zero_not_unknown(self):
        report = self.collect(Source(["datasets"], {"datasets": 2}))
        chat = next(row for row in report["authority_tables"] if row["table"] == "chat_sessions")
        self.assertEqual(chat, {"table": "chat_sessions", "family": "chat_history",
                                "present": False, "row_count": 0})

    def test_unclassified_table_fails_before_any_row_count(self):
        source = Source(["datasets", "forgotten_runtime_table"])
        with self.assertRaisesRegex(RuntimeError, "unclassified.*forgotten_runtime_table"):
            self.collect(source)
        self.assertFalse(any("COUNT" in statement for statement in source.statements))

    def test_invalid_count_and_table_bound_fail_closed(self):
        with self.assertRaisesRegex(RuntimeError, "count is invalid"):
            self.collect(Source(["datasets"], {"datasets": -1}))
        source = Source(["datasets"] * (inventory.MAX_TABLES + 1))
        with self.assertRaisesRegex(RuntimeError, "table bound"):
            self.collect(source)


if __name__ == "__main__":
    unittest.main()
