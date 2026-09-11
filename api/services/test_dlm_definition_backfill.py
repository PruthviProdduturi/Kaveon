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

from services import dlm_definition_backfill as backfill
from services import dlm_definition_backfill_operation as operation


class Source:
    def __init__(self, rows):
        self.rows = rows
        self.statements = []

    def execute(self, sql, params=None): self.statements.append(" ".join(sql.split()))
    def query_one(self, sql, params=None): return {"watermark": 12}
    def query(self, sql, params=None):
        self.statements.append(" ".join(sql.split()))
        return {"rows": self.rows}


@contextlib.contextmanager
def transaction(source):
    yield source


def snapshot():
    document = {"dataset_id": "7", "dataset_revision": 3}
    encoded = json.dumps(document, sort_keys=True, separators=(",", ":"))
    record = backfill.DefinitionRecord("7", "owner@example.test", document, hashlib.sha256(encoded.encode()).hexdigest())
    return backfill.DefinitionSnapshot(12, "snap-1", (record,), backfill.snapshot_digest((record,), "snap-1"))


class DlmDefinitionBackfillTests(unittest.TestCase):
    def test_capture_uses_repeatable_source_and_one_target_snapshot(self):
        source = Source([{"id": 7, "created_by": "owner@example.test"}, {"id": 8, "created_by": "owner@example.test"}])
        targets = [
            {"revision": 3, "snapshot_id": "snap-1"},
            {"revision": 4, "snapshot_id": "snap-1"},
        ]
        with patch.object(backfill.db, "transaction", return_value=transaction(source)), \
             patch.object(backfill.product_store, "read", side_effect=targets) as read:
            result = backfill.capture_snapshot()
        self.assertIn("REPEATABLE READ, READ ONLY", source.statements[0])
        self.assertEqual([record.document["dataset_revision"] for record in result.records], [3, 4])
        self.assertTrue(all(call.args[2:] == ("owner@example.test", "Admin") for call in read.call_args_list))

    def test_capture_rejects_missing_or_mixed_target_dataset_snapshot(self):
        source = Source([{"id": 7, "created_by": "owner"}, {"id": 8, "created_by": "owner"}])
        for targets, message in (([None], "missing"), ([{"revision": 1, "snapshot_id": "a"}, {"revision": 1, "snapshot_id": "b"}], "changed")):
            with self.subTest(message=message), patch.object(backfill.db, "transaction", return_value=transaction(source)), \
                 patch.object(backfill.product_store, "read", side_effect=targets):
                with self.assertRaisesRegex(RuntimeError, message): backfill.capture_snapshot()

    def test_apply_creates_and_exact_rerun_reconciles_as_owner(self):
        value = snapshot()
        with patch.object(backfill.product_store, "read", side_effect=[None, {"document": value.records[0].document}]), \
             patch.object(backfill.product_store, "transact") as transact:
            report = backfill.apply_and_reconcile(value)
        self.assertEqual((report["created"], report["reconciled"]), (1, 1))
        self.assertEqual(transact.call_args.args[1:], ("owner@example.test", "Admin"))
        with patch.object(backfill.product_store, "read", side_effect=[{"document": value.records[0].document}, {"document": value.records[0].document}]), \
             patch.object(backfill.product_store, "transact") as transact:
            self.assertEqual(backfill.apply_and_reconcile(value)["already_present"], 1)
        transact.assert_not_called()

    def test_divergence_and_corrupt_snapshot_fail_before_write(self):
        value = snapshot()
        with patch.object(backfill.product_store, "read", return_value={"document": {"dataset_id": "7", "dataset_revision": 2}}), \
             patch.object(backfill.product_store, "transact") as transact:
            with self.assertRaisesRegex(RuntimeError, "diverges"): backfill.apply_and_reconcile(value)
        transact.assert_not_called()
        corrupt = backfill.DefinitionSnapshot(value.source_watermark, value.dataset_snapshot_id, value.records, "0" * 64)
        with patch.object(backfill.product_store, "read") as read:
            with self.assertRaisesRegex(RuntimeError, "identity mismatch"): backfill.apply_and_reconcile(corrupt)
        read.assert_not_called()

    def test_ambiguous_create_resolves_only_from_exact_target(self):
        value = snapshot()
        exact = {"document": value.records[0].document}
        with patch.object(backfill.product_store, "read", side_effect=[None, exact, exact]), \
             patch.object(backfill.product_store, "transact", side_effect=HTTPException(409, "conflict")):
            self.assertEqual(backfill.apply_and_reconcile(value)["created"], 1)
        with patch.object(backfill.product_store, "read", side_effect=[None, {"document": {} }]), \
             patch.object(backfill.product_store, "transact", side_effect=HTTPException(409, "conflict")):
            with self.assertRaises(HTTPException):
                backfill.apply_and_reconcile(value)

    def test_checkpoint_roundtrip_dry_run_and_apply_guard(self):
        value = snapshot()
        with tempfile.TemporaryDirectory() as temporary:
            path = Path(temporary) / "checkpoint.json"
            with patch.object(backfill, "capture_snapshot", return_value=value):
                report = operation.run(path, apply=False, resume=False)
            self.assertEqual(report["mode"], "dry-run")
            loaded, position, complete = operation.load(path)
            self.assertEqual((loaded, position, complete), (value, 0, False))
            with patch.dict(os.environ, {}, clear=True), patch.object(backfill, "capture_snapshot") as capture:
                with self.assertRaisesRegex(RuntimeError, "requires"): operation.run(path, apply=True, resume=True)
            capture.assert_not_called()

    def test_checkpoint_tamper_and_post_apply_save_failure_resume_exactly(self):
        value = snapshot()
        with tempfile.TemporaryDirectory() as temporary:
            path = Path(temporary) / "checkpoint.json"
            operation.save(path, value, 0)
            raw = json.loads(path.read_text())
            raw["next_index"] = 1
            path.write_text(json.dumps(raw))
            with self.assertRaisesRegex(RuntimeError, "identity"): operation.load(path)
            operation.save(path, value, 0)
            original_save = operation.save
            with patch.dict(os.environ, {"KAVEON_DLM_DEFINITION_MIGRATION_ENABLED": "true"}), \
                 patch.object(backfill, "apply_and_reconcile", return_value={"family": "dlm_definitions"}) as apply, \
                 patch.object(operation, "save", side_effect=RuntimeError("checkpoint failure")):
                with self.assertRaisesRegex(RuntimeError, "checkpoint failure"):
                    operation.run(path, apply=True, resume=True)
            self.assertEqual(operation.load(path)[1], 0)
            with patch.dict(os.environ, {"KAVEON_DLM_DEFINITION_MIGRATION_ENABLED": "true"}), \
                 patch.object(backfill, "apply_and_reconcile", side_effect=[{"family": "dlm_definitions"}, {"family": "dlm_definitions", "reconciled": 1, "created": 0, "already_present": 1, "source_count": 1, "source_watermark": 12, "dataset_snapshot_id": "snap-1", "snapshot_sha256": value.snapshot_sha256}]) as retry, \
                 patch.object(operation, "save", wraps=original_save):
                report = operation.run(path, apply=True, resume=True)
            self.assertEqual(retry.call_count, 2)
            self.assertTrue(report["checkpoint_complete"])


if __name__ == "__main__": unittest.main()
