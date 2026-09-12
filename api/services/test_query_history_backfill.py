import contextlib
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

from services import query_history_backfill as backfill
from services import query_history_backfill_operation as operation


class Source:
    def __init__(self, row):
        self.row = row
        self.statements = []

    def execute(self, sql, params=None):
        self.statements.append(" ".join(sql.split()))

    def query_one(self, sql, params=None):
        return {"watermark": 17}

    def query(self, sql, params=None):
        self.statements.append(" ".join(sql.split()))
        if "FROM datasets" in sql:
            return {"rows": [{"id": "1"}]}
        return {"rows": [self.row]}


@contextlib.contextmanager
def transaction(source):
    yield source


def row(record_id="q-1", owner="owner@example.test"):
    return {
        "id": record_id,
        "sql_text": "SELECT count(*) FROM trips",
        "database_name": "kavedb",
        "executed_at": "2026-01-01T00:00:00",
        "execution_time": 42,
        "row_count": 3,
        "status": "success",
        "error_message": None,
        "user_email": owner,
        "trigger_source": "sql_lab",
        "dataset_id": None,
        "tables_used": "[\"trips\"]",
    }


def snapshot():
    document = backfill.document(row())
    record = backfill.Record(
        "q-1",
        "owner@example.test",
        document,
        backfill.canonical(document),
    )
    return backfill.Snapshot(17, (record,), backfill.digest((record,)))


class QueryHistoryBackfillTests(unittest.TestCase):
    def test_capture_is_repeatable_and_pins_source_watermark(self):
        source = Source(row())
        with patch.object(backfill.db, "transaction", return_value=transaction(source)):
            captured = backfill.capture_snapshot()
        self.assertEqual(captured, snapshot())
        self.assertIn("REPEATABLE READ, READ ONLY", source.statements[0])

    def test_missing_dataset_reference_is_cleared_without_creating_a_placeholder(self):
        source = Source(row())
        source.row["dataset_id"] = "13"
        with patch.object(backfill.db, "transaction", return_value=transaction(source)):
            captured = backfill.capture_snapshot()
        self.assertIsNone(captured.records[0].document["dataset_id"])

    def test_validation_rejects_owner_mismatch_and_retention_overflow(self):
        value = snapshot()
        invalid = backfill.Record(
            value.records[0].record_id,
            "other@example.test",
            value.records[0].document,
            value.records[0].payload_sha256,
        )
        with self.assertRaisesRegex(RuntimeError, "record is invalid"):
            backfill.validate(backfill.Snapshot(17, (invalid,), backfill.digest((invalid,))))

        records = tuple(
            backfill.Record(
                f"q-{index:04d}",
                "owner@example.test",
                {**value.records[0].document, "id": f"q-{index:04d}"},
                backfill.canonical({**value.records[0].document, "id": f"q-{index:04d}"}),
            )
            for index in range(backfill.MAX_PER_OWNER + 1)
        )
        with self.assertRaisesRegex(RuntimeError, "retention exceeds"):
            backfill.validate(backfill.Snapshot(17, records, backfill.digest(records)))

    def test_apply_reconciles_exact_document_and_rejects_divergence(self):
        value = snapshot()
        exact = {"document": value.records[0].document}
        with patch.object(backfill.product_store, "read", side_effect=[None, exact]), \
                patch.object(backfill.product_store, "transact") as transact:
            report = backfill.apply_and_reconcile(value)
        self.assertEqual(report["created"], 1)
        self.assertEqual(report["reconciled"], 1)
        transact.assert_called_once()

        with patch.object(backfill.product_store, "read", return_value={"document": {}}), \
                patch.object(backfill.product_store, "transact") as transact:
            with self.assertRaisesRegex(RuntimeError, "diverges"):
                backfill.apply_and_reconcile(value)
        transact.assert_not_called()

        with patch.object(backfill.product_store, "read", side_effect=[None, exact, exact]), \
                patch.object(
                    backfill.product_store,
                    "transact",
                    side_effect=HTTPException(409, "conflict"),
                ):
            report = backfill.apply_and_reconcile(value)
        self.assertEqual(report["reconciled"], 1)

    def test_checkpoint_is_tamper_evident_and_resume_is_deterministic(self):
        value = snapshot()
        with tempfile.TemporaryDirectory() as temporary:
            path = Path(temporary) / "query-history.json"
            with patch.object(backfill, "capture_snapshot", return_value=value):
                dry_run = operation.run(path, apply=False, resume=False)
            self.assertEqual(dry_run["snapshot_sha256"], value.snapshot_sha256)

            raw = json.loads(path.read_text())
            raw["next_index"] = 1
            path.write_text(json.dumps(raw))
            with self.assertRaisesRegex(RuntimeError, "identity"):
                operation.load(path)

            operation.save(path, value, 0)
            final = {"family": "query_history", "source_count": 1, "reconciled": 1}
            with patch.dict(os.environ, {"KAVEON_QUERY_HISTORY_MIGRATION_ENABLED": "true"}), \
                    patch.object(backfill, "apply_and_reconcile", side_effect=[{}, final]) as apply:
                result = operation.run(path, apply=True, resume=True)
            self.assertTrue(result["checkpoint_complete"])
            self.assertEqual(result["reconciled"], 1)
            self.assertEqual(apply.call_count, 2)
            self.assertEqual(operation.load(path)[1:], (1, True))


if __name__ == "__main__":
    unittest.main()
