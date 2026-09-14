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

from services import dlm_run_backfill as backfill
from services import dlm_run_backfill_operation as operation


class Source:
    def __init__(self, rows):
        self.rows, self.statements = rows, []

    def execute(self, sql, params=None): self.statements.append(" ".join(sql.split()))
    def query_one(self, sql, params=None): return {"watermark": 17}
    def query(self, sql, params=None):
        self.statements.append(" ".join(sql.split()))
        return {"rows": self.rows}


@contextlib.contextmanager
def transaction(source):
    yield source


def staged(root: Path, dataset_id="7", version=2, manifest=None):
    manifest = {"columns": ["a"], "schema": 1} if manifest is None else manifest
    compiled = {"dataset_id": dataset_id, "version": version, "manifest": manifest,
                "stats_rollup": {}, "usage_rollup": {}, "source_hash": "",
                "built_at": "", "status": "ready", "values_indexed": 0}
    path = root / "dlm" / dataset_id / f"v{version}" / "compiled.json"
    path.parent.mkdir(parents=True)
    path.write_bytes(json.dumps(compiled, sort_keys=True, separators=(",", ":")).encode())
    return manifest


def snapshot():
    document = {"definition_id": "7", "definition_revision": 4, "status": "ready",
                "artifact": {"path": "dlm/7/v2/compiled.json", "sha256": "a" * 64}}
    record = backfill.RunRecord("7-v2", "owner@example.test", document,
                                hashlib.sha256(backfill._canonical(document)).hexdigest())
    return backfill.RunSnapshot(17, "snap-2", (record,),
                                backfill.snapshot_digest((record,), "snap-2"))


class DlmRunBackfillTests(unittest.TestCase):
    def test_capture_binds_exact_definition_revision_and_staged_artifact(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            manifest = staged(root)
            source = Source([{"id": 7, "created_by": "owner@example.test", "version": 2,
                              "manifest": json.dumps(manifest), "status": "ready"}])
            definition = {"revision": 4, "snapshot_id": "snap-2"}
            with patch.object(backfill.db, "transaction", return_value=transaction(source)), \
                 patch.object(backfill.product_store, "read", return_value=definition) as read:
                result = backfill.capture_snapshot(root)
        self.assertIn("REPEATABLE READ, READ ONLY", source.statements[0])
        self.assertEqual(result.records[0].document["definition_revision"], 4)
        self.assertEqual(result.records[0].document["artifact"]["path"], "dlm/7/v2/compiled.json")
        self.assertEqual(read.call_args.args, ("dlm_definition", "7", "owner@example.test", "Admin"))

    def test_capture_fails_closed_on_unsupported_missing_or_divergent_data(self):
        cases = [
            ({"id": 7, "created_by": "owner", "version": 2, "manifest": "{}", "status": "stale"},
             "unsupported", False),
            ({"id": 7, "created_by": "owner", "version": 2, "manifest": "not-json", "status": "ready"},
             "manifest is invalid", False),
            ({"id": 7, "created_by": "owner", "version": 2, "manifest": "{}", "status": "ready"},
             "definition 7 is missing", True),
        ]
        for row, message, create_artifact in cases:
            with self.subTest(message=message), tempfile.TemporaryDirectory() as temporary:
                root = Path(temporary)
                if create_artifact: staged(root, manifest={})
                with patch.object(backfill.db, "transaction", return_value=transaction(Source([row]))), \
                     patch.object(backfill.product_store, "read", return_value=None):
                    with self.assertRaisesRegex(RuntimeError, message):
                        backfill.capture_snapshot(root)

    def test_apply_atomically_publishes_building_then_ready_and_reruns_exactly(self):
        value = snapshot()
        exact = {"document": value.records[0].document}
        with patch.object(backfill.product_store, "read", side_effect=[None, exact]), \
             patch.object(backfill.product_store, "transact") as transact:
            report = backfill.apply_and_reconcile(value)
        self.assertEqual((report["created"], report["reconciled"]), (1, 1))
        mutations = transact.call_args.args[0]
        self.assertEqual([item.operation for item in mutations], ["create", "update"])
        self.assertEqual([item.document["status"] for item in mutations], ["building", "ready"])
        self.assertEqual(mutations[1].expected_revision, 1)
        self.assertEqual(transact.call_args.args[1:], ("owner@example.test", "Admin"))
        with patch.object(backfill.product_store, "read", side_effect=[exact, exact]), \
             patch.object(backfill.product_store, "transact") as transact:
            self.assertEqual(backfill.apply_and_reconcile(value)["already_present"], 1)
        transact.assert_not_called()

    def test_divergence_corruption_and_ambiguous_create_fail_closed(self):
        value = snapshot()
        with patch.object(backfill.product_store, "read", return_value={"document": {}}), \
             patch.object(backfill.product_store, "transact") as transact:
            with self.assertRaisesRegex(RuntimeError, "diverges"):
                backfill.apply_and_reconcile(value)
        transact.assert_not_called()
        corrupt = backfill.RunSnapshot(17, "snap-2", value.records, "0" * 64)
        with patch.object(backfill.product_store, "read") as read:
            with self.assertRaisesRegex(RuntimeError, "identity mismatch"):
                backfill.apply_and_reconcile(corrupt)
        read.assert_not_called()
        exact = {"document": value.records[0].document}
        with patch.object(backfill.product_store, "read", side_effect=[None, exact, exact]), \
             patch.object(backfill.product_store, "transact", side_effect=HTTPException(409, "conflict")):
            self.assertEqual(backfill.apply_and_reconcile(value)["created"], 1)

    def test_checkpoint_dry_run_guard_tamper_and_failure_resume(self):
        value = snapshot()
        with tempfile.TemporaryDirectory() as temporary:
            path, root = Path(temporary) / "checkpoint.json", Path(temporary) / "artifacts"
            with patch.object(backfill, "capture_snapshot", return_value=value):
                self.assertEqual(operation.run(path, root, apply=False, resume=False)["mode"], "dry-run")
            self.assertEqual(operation.load(path), (value, 0, False))
            with patch.dict(os.environ, {}, clear=True):
                with self.assertRaisesRegex(RuntimeError, "requires"):
                    operation.run(path, root, apply=True, resume=True)
            raw = json.loads(path.read_text())
            raw["next_index"] = 1
            path.write_text(json.dumps(raw))
            with self.assertRaisesRegex(RuntimeError, "identity"):
                operation.load(path)
            operation.save(path, value, 0)
            original_save = operation.save
            with patch.dict(os.environ, {"KAVEON_DLM_RUN_MIGRATION_ENABLED": "true"}), \
                 patch.object(backfill, "apply_and_reconcile", return_value={"family": "dlm_runs"}) as apply, \
                 patch.object(backfill, "restage_artifacts"), \
                 patch.object(operation, "save", side_effect=RuntimeError("checkpoint failure")):
                with self.assertRaisesRegex(RuntimeError, "checkpoint failure"):
                    with patch.dict(os.environ, {"KAVEON_DLM_ARTIFACT_PUBLISH_ENABLED": "true"}):
                        operation.run(path, root, apply=True, resume=True,
                                      publisher=SimpleNamespace(publish=lambda *_: None))
            self.assertEqual(operation.load(path)[1], 0)
            full_report = {"family": "dlm_runs", "reconciled": 1, "created": 0,
                           "already_present": 1, "source_count": 1, "source_watermark": 17,
                           "definition_snapshot_id": "snap-2", "snapshot_sha256": value.snapshot_sha256}
            with patch.dict(os.environ, {"KAVEON_DLM_RUN_MIGRATION_ENABLED": "true",
                                         "KAVEON_DLM_ARTIFACT_PUBLISH_ENABLED": "true"}), \
                 patch.object(backfill, "apply_and_reconcile",
                              side_effect=[{"family": "dlm_runs"}, full_report]) as retry, \
                 patch.object(backfill, "restage_artifacts"), \
                 patch.object(operation, "save", wraps=original_save):
                report = operation.run(path, root, apply=True, resume=True,
                                       publisher=SimpleNamespace(publish=lambda *_: None))
            self.assertEqual(retry.call_count, 2)
            self.assertTrue(report["checkpoint_complete"])

    def test_apply_requires_publisher_and_publication_failure_prevents_metadata(self):
        value = snapshot()
        with tempfile.TemporaryDirectory() as temporary:
            path, root = Path(temporary) / "checkpoint.json", Path(temporary)
            operation.save(path, value, 0)
            enabled = {"KAVEON_DLM_RUN_MIGRATION_ENABLED": "true",
                       "KAVEON_DLM_ARTIFACT_PUBLISH_ENABLED": "true"}
            with patch.dict(os.environ, enabled), patch.object(backfill, "restage_artifacts"), \
                 self.assertRaisesRegex(RuntimeError, "publication"):
                operation.run(path, root, apply=True, resume=True)
            failing = SimpleNamespace(publish=lambda *_: (_ for _ in ()).throw(RuntimeError("publish failed")))
            with patch.dict(os.environ, enabled), \
                 patch.object(backfill, "restage_artifacts"), \
                 patch.object(backfill, "apply_and_reconcile") as apply, \
                 self.assertRaisesRegex(RuntimeError, "publish failed"):
                operation.run(path, root, apply=True, resume=True, publisher=failing)
            apply.assert_not_called()

    def test_capture_stages_full_compiled_payload_and_restages_exactly(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            row = {"id": 7, "created_by": "owner", "version": 2,
                   "manifest": '{"name":"orders"}', "stats_rollup": '{"rows":12}',
                   "usage_rollup": '{"queries":3}', "source_hash": "source-1",
                   "built_at": "2026-09-14T20:00:00Z", "status": "ready", "values_indexed": 4}
            source = Source([row]); definition = {"revision": 4, "snapshot_id": "snap-2"}
            with patch.object(backfill.db, "transaction", return_value=transaction(source)), \
                 patch.object(backfill.product_store, "read", return_value=definition):
                value = backfill.capture_snapshot(root)
            path = root / "dlm" / "7" / "v2" / "compiled.json"
            compiled = json.loads(path.read_text())
            self.assertEqual((compiled["stats_rollup"]["rows"], compiled["values_indexed"]), (12, 4))
            path.unlink()
            with patch.object(backfill.db, "transaction", return_value=transaction(Source([row]))):
                backfill.restage_artifacts(value, root)
            self.assertEqual(hashlib.sha256(path.read_bytes()).hexdigest(),
                             value.records[0].document["artifact"]["sha256"])

    def test_restage_rejects_source_drift(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary); value = snapshot()
            changed = {"id": 7, "created_by": "owner", "version": 2,
                       "manifest": '{"changed":true}', "status": "ready"}
            with patch.object(backfill.db, "transaction", return_value=transaction(Source([changed]))), \
                 self.assertRaisesRegex(RuntimeError, "changed"):
                backfill.restage_artifacts(value, root)


if __name__ == "__main__": unittest.main()
