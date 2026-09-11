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

if "pyodbc" not in sys.modules: sys.modules["pyodbc"] = SimpleNamespace(Error=Exception)
from services import chart_backfill as backfill
from services import chart_backfill_operation as operation


class Source:
    def __init__(self, row, modern=True): self.row, self.modern, self.statements = row, modern, []
    def execute(self, sql, params=None): self.statements.append(" ".join(sql.split()))
    def query_one(self, sql, params=None): return {"watermark": 12}
    def query(self, sql, params=None):
        self.statements.append(" ".join(sql.split()))
        if "information_schema" in sql:
            names = ["dataset_id", "config", "modified_at", "modified_by"] if self.modern else \
                    ["query_config", "viz_config", "updated_at", "updated_by"]
            return {"rows": [{"column_name": name} for name in names]}
        return {"rows": [self.row]}


@contextlib.contextmanager
def transaction(source): yield source


def row():
    return {"id": "c-1", "name": "Trips", "description": None, "dataset_id": 7,
            "chart_type": "bar", "config": json.dumps({"query_config": {"x": "month"},
            "viz_config": {"color": "blue"}}), "visibility": "private",
            "created_by": "owner@example.test", "modified_by": "editor@example.test",
            "created_at": "2026-01-01", "modified_at": "2026-01-02"}


def snapshot():
    document = {"id": "c-1", "name": "Trips", "description": None, "dataset_id": "7",
                "dataset_revision": 3, "chart_type": "bar", "query_config": {"x": "month"},
                "viz_config": {"color": "blue"}, "visibility": "private", "created_at": "2026-01-01",
                "updated_at": "2026-01-02", "created_by": "owner@example.test",
                "modified_by": "editor@example.test"}
    record = backfill.ChartRecord("c-1", "owner@example.test", document,
                                  hashlib.sha256(backfill._canonical(document)[0]).hexdigest())
    return backfill.ChartSnapshot(12, "snap-1", (record,), backfill.snapshot_digest((record,), "snap-1"))


class ChartBackfillTests(unittest.TestCase):
    def test_capture_is_repeatable_owner_scoped_and_revision_bound(self):
        source = Source(row())
        with patch.object(backfill.db, "transaction", return_value=transaction(source)), \
             patch.object(backfill.product_store, "read", return_value={"revision": 3, "snapshot_id": "snap-1"}) as read:
            result = backfill.capture_snapshot()
        self.assertIn("REPEATABLE READ, READ ONLY", source.statements[0])
        self.assertEqual(result.records[0].document, snapshot().records[0].document)
        self.assertEqual(read.call_args.args, ("dataset", "7", "owner@example.test", "Admin"))

    def test_capture_supports_legacy_layout_deterministically(self):
        legacy = row()
        legacy.pop("dataset_id"); legacy.pop("config"); legacy.pop("modified_at"); legacy.pop("modified_by")
        legacy.update({"query_config": json.dumps({"dataset_id": 7, "x": "month"}),
                       "viz_config": json.dumps({"color": "blue"}),
                       "updated_at": "2026-01-02", "updated_by": "editor@example.test"})
        with patch.object(backfill.db, "transaction", return_value=transaction(Source(legacy, modern=False))), \
             patch.object(backfill.product_store, "read", return_value={"revision": 3, "snapshot_id": "snap-1"}):
            result = backfill.capture_snapshot()
        self.assertEqual(result.records[0].document["dataset_id"], "7")
        self.assertEqual(result.records[0].document["query_config"]["x"], "month")

    def test_capture_rejects_bad_config_missing_dataset_and_mixed_snapshot(self):
        bad = row(); bad["config"] = "bad"
        missing = row(); missing["dataset_id"] = None; missing["config"] = "{}"
        for source_row, targets, message in ((bad, [], "config"), (missing, [], "dataset_id"),
                (row(), [None], "missing for chart")):
            with self.subTest(message=message), patch.object(backfill.db, "transaction",
                 return_value=transaction(Source(source_row))), \
                 patch.object(backfill.product_store, "read", side_effect=targets):
                with self.assertRaisesRegex(RuntimeError, message): backfill.capture_snapshot()

    def test_apply_creates_exactly_as_owner_and_resolves_only_exact_conflict(self):
        value, exact = snapshot(), {"document": snapshot().records[0].document}
        with patch.object(backfill.product_store, "read", side_effect=[None, exact]), \
             patch.object(backfill.product_store, "transact") as transact:
            self.assertEqual(backfill.apply_and_reconcile(value)["created"], 1)
        self.assertEqual(transact.call_args.args[1:], ("owner@example.test", "Admin"))
        with patch.object(backfill.product_store, "read", side_effect=[None, exact, exact]), \
             patch.object(backfill.product_store, "transact", side_effect=HTTPException(409, "conflict")):
            self.assertEqual(backfill.apply_and_reconcile(value)["reconciled"], 1)
        with patch.object(backfill.product_store, "read", return_value={"document": {}}), \
             patch.object(backfill.product_store, "transact") as transact:
            with self.assertRaisesRegex(RuntimeError, "diverges"): backfill.apply_and_reconcile(value)
        transact.assert_not_called()

    def test_checkpoint_dry_guard_tamper_and_resume_after_failure(self):
        value = snapshot()
        with tempfile.TemporaryDirectory() as temporary:
            path = Path(temporary) / "checkpoint.json"
            with patch.object(backfill, "capture_snapshot", return_value=value):
                self.assertEqual(operation.run(path, apply=False, resume=False)["mode"], "dry-run")
            with patch.dict(os.environ, {}, clear=True), self.assertRaisesRegex(RuntimeError, "requires"):
                operation.run(path, apply=True, resume=True)
            raw = json.loads(path.read_text()); raw["next_index"] = 1; path.write_text(json.dumps(raw))
            with self.assertRaisesRegex(RuntimeError, "identity"): operation.load(path)
            operation.save(path, value, 0); original = operation.save
            with patch.dict(os.environ, {"KAVEON_CHART_MIGRATION_ENABLED": "true"}), \
                 patch.object(backfill, "apply_and_reconcile", return_value={}), \
                 patch.object(operation, "save", side_effect=RuntimeError("checkpoint failure")):
                with self.assertRaisesRegex(RuntimeError, "checkpoint failure"):
                    operation.run(path, apply=True, resume=True)
            self.assertEqual(operation.load(path)[1], 0)
            report = {"family": "charts", "source_count": 1, "reconciled": 1}
            with patch.dict(os.environ, {"KAVEON_CHART_MIGRATION_ENABLED": "true"}), \
                 patch.object(backfill, "apply_and_reconcile", side_effect=[{}, report]) as apply, \
                 patch.object(operation, "save", wraps=original):
                self.assertTrue(operation.run(path, apply=True, resume=True)["checkpoint_complete"])
            self.assertEqual(apply.call_count, 2)


if __name__ == "__main__": unittest.main()
