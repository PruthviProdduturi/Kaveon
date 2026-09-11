import contextlib
import hashlib
import json
import os
import sys
import tempfile
import unittest
from pathlib import Path
from types import SimpleNamespace
from unittest.mock import patch

from fastapi import HTTPException

if "pyodbc" not in sys.modules:
    sys.modules["pyodbc"] = SimpleNamespace(Error=Exception)

from services import saved_query_backfill as backfill
from services import saved_query_backfill_operation as operation


class Source:
    def __init__(self, source_row, extended=True):
        self.row, self.extended, self.statements = source_row, extended, []

    def execute(self, sql, params=None):
        self.statements.append(" ".join(sql.split()))

    def query_one(self, sql, params=None):
        return {"watermark": 21}

    def query(self, sql, params=None):
        self.statements.append(" ".join(sql.split()))
        if "information_schema" in sql:
            names = ["id", "name", "description", "sql_text", "created_by", "created_at"]
            names += ["modified_by", "modified_at"] if self.extended else ["updated_at"]
            return {"rows": [{"column_name": name} for name in names]}
        return {"rows": [self.row]}


@contextlib.contextmanager
def transaction(source):
    yield source


def row(extended=True):
    value = {"id": "7", "name": "Trips", "description": None,
             "sql_text": "SELECT * FROM trips", "created_by": "owner@example.test",
             "created_at": "2026-01-01"}
    if extended:
        value.update(modified_by="editor@example.test", modified_at="2026-01-02")
    else:
        value["updated_at"] = "2026-01-02"
    return value


def snapshot():
    document = {"id": "7", "name": "Trips", "description": None,
                "sql": "SELECT * FROM trips", "created_at": "2026-01-01",
                "updated_at": "2026-01-02", "created_by": "owner@example.test",
                "modified_by": "editor@example.test"}
    record = backfill.SavedQueryRecord(
        "7", "owner@example.test", document,
        hashlib.sha256(backfill._canonical(document)[0]).hexdigest())
    return backfill.SavedQuerySnapshot(21, (record,), backfill.snapshot_digest((record,)))


class SavedQueryBackfillTests(unittest.TestCase):
    def test_capture_is_repeatable_read_and_supports_both_schemas(self):
        source = Source(row())
        with patch.object(backfill.db, "transaction", return_value=transaction(source)):
            result = backfill.capture_snapshot()
        self.assertIn("REPEATABLE READ, READ ONLY", source.statements[0])
        self.assertEqual(result, snapshot())

        simple_source = Source(row(False), extended=False)
        with patch.object(backfill.db, "transaction", return_value=transaction(simple_source)):
            simple = backfill.capture_snapshot()
        self.assertEqual(simple.records[0].document["modified_by"], "owner@example.test")

    def test_capture_rejects_missing_owner_and_unsupported_schema(self):
        missing = row(); missing["created_by"] = None
        with patch.object(backfill.db, "transaction", return_value=transaction(Source(missing))):
            with self.assertRaisesRegex(RuntimeError, "owner is missing"):
                backfill.capture_snapshot()
        unsupported = Source(row())
        unsupported.query = lambda sql, params=None: {"rows": []}
        with patch.object(backfill.db, "transaction", return_value=transaction(unsupported)):
            with self.assertRaisesRegex(RuntimeError, "schema is unsupported"):
                backfill.capture_snapshot()

    def test_apply_is_owner_scoped_and_accepts_only_exact_conflict(self):
        value, exact = snapshot(), {"document": snapshot().records[0].document}
        with patch.object(backfill.product_store, "read", side_effect=[None, exact]), \
             patch.object(backfill.product_store, "transact") as transact:
            self.assertEqual(backfill.apply_and_reconcile(value)["created"], 1)
        self.assertEqual(transact.call_args.args[1:], ("owner@example.test", "Admin"))

        with patch.object(backfill.product_store, "read", side_effect=[None, exact, exact]), \
             patch.object(backfill.product_store, "transact",
                          side_effect=HTTPException(409, "conflict")):
            self.assertEqual(backfill.apply_and_reconcile(value)["reconciled"], 1)
        with patch.object(backfill.product_store, "read", return_value={"document": {}}), \
             patch.object(backfill.product_store, "transact") as transact:
            with self.assertRaisesRegex(RuntimeError, "diverges"):
                backfill.apply_and_reconcile(value)
        transact.assert_not_called()

    def test_checkpoint_is_default_dry_tamper_evident_and_resumable(self):
        value = snapshot()
        with tempfile.TemporaryDirectory() as temporary:
            path = Path(temporary) / "checkpoint.json"
            with patch.object(backfill, "capture_snapshot", return_value=value):
                report = operation.run(path, apply=False, resume=False)
            self.assertEqual(report["mode"], "dry-run")
            with patch.dict(os.environ, {}, clear=True):
                with self.assertRaisesRegex(RuntimeError, "requires"):
                    operation.run(path, apply=True, resume=True)
            raw = json.loads(path.read_text()); raw["next_index"] = 1
            path.write_text(json.dumps(raw))
            with self.assertRaisesRegex(RuntimeError, "identity"):
                operation.load(path)

            operation.save(path, value, 0)
            final = {"family": "saved_queries", "source_count": 1, "reconciled": 1}
            with patch.dict(os.environ, {"KAVEON_SAVED_QUERY_MIGRATION_ENABLED": "true"}), \
                 patch.object(backfill, "apply_and_reconcile", side_effect=[{}, final]) as apply:
                result = operation.run(path, apply=True, resume=True)
            self.assertTrue(result["checkpoint_complete"])
            self.assertEqual(apply.call_count, 2)
            self.assertEqual(operation.load(path)[1:], (1, True))


if __name__ == "__main__":
    unittest.main()
