import json
import os
import sys
import tempfile
import unittest
from pathlib import Path
from types import SimpleNamespace
from unittest.mock import patch

if "pyodbc" not in sys.modules:
    sys.modules["pyodbc"] = SimpleNamespace(Error=Exception)

from services import product_backfill, product_backfill_operation


def snapshot(count=2):
    records = []
    for index in range(1, count + 1):
        document = {"id": str(index), "name": f"Dataset {index}"}
        _, digest = product_backfill._canonical(document)
        records.append(product_backfill.SnapshotRecord(
            str(index), "owner@example.com", document, digest
        ))
    records = tuple(records)
    return product_backfill.DatasetSnapshot(
        12, records, product_backfill.snapshot_digest(records)
    )


class ProductBackfillOperationTests(unittest.TestCase):
    def test_checkpoint_round_trip_validates_snapshot_and_position(self):
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "checkpoint.json"
            product_backfill_operation.save_checkpoint(path, snapshot(), 1)
            restored, next_index, complete = product_backfill_operation.load_checkpoint(path)
        self.assertEqual(restored, snapshot())
        self.assertEqual((next_index, complete), (1, False))

    def test_checkpoint_keeps_last_complete_backup_and_recovers_if_active_file_is_lost(self):
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "checkpoint.json"
            product_backfill_operation.save_checkpoint(path, snapshot(), 0)
            product_backfill_operation.save_checkpoint(path, snapshot(), 1)
            backup = Path(str(path) + ".bak")
            self.assertTrue(backup.exists())
            path.unlink()
            restored, next_index, complete = product_backfill_operation.load_checkpoint(path)
        self.assertEqual(restored, snapshot())
        self.assertEqual((next_index, complete), (0, False))

    def test_corrupt_active_checkpoint_fails_closed_even_when_backup_exists(self):
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "checkpoint.json"
            product_backfill_operation.save_checkpoint(path, snapshot(), 0)
            product_backfill_operation.save_checkpoint(path, snapshot(), 1)
            path.write_text("{}", encoding="utf-8")
            with self.assertRaisesRegex(RuntimeError, "checkpoint identity mismatch"):
                product_backfill_operation.load_checkpoint(path)

    def test_corrupt_checkpoint_document_is_rejected(self):
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "checkpoint.json"
            product_backfill_operation.save_checkpoint(path, snapshot(), 0)
            value = json.loads(path.read_text(encoding="utf-8"))
            value["records"][0]["document"]["name"] = "tampered"
            path.write_text(json.dumps(value), encoding="utf-8")
            with self.assertRaisesRegex(RuntimeError, "checkpoint identity mismatch"):
                product_backfill_operation.load_checkpoint(path)

    def test_corrupt_checkpoint_position_cannot_skip_records(self):
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "checkpoint.json"
            product_backfill_operation.save_checkpoint(path, snapshot(), 0)
            value = json.loads(path.read_text(encoding="utf-8"))
            value["next_index"] = 2
            path.write_text(json.dumps(value), encoding="utf-8")
            with self.assertRaisesRegex(RuntimeError, "checkpoint identity mismatch"):
                product_backfill_operation.load_checkpoint(path)

    def test_default_dry_run_captures_checkpoint_without_target_write(self):
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "checkpoint.json"
            with patch.object(product_backfill_operation.product_backfill, "capture_dataset_snapshot", return_value=snapshot()), \
                 patch.object(product_backfill_operation.product_backfill, "apply_and_reconcile") as apply:
                report = product_backfill_operation.run(path, apply=False, resume=False)
            self.assertTrue(path.exists())
        self.assertEqual(report["mode"], "dry-run")
        self.assertEqual(report["next_index"], 0)
        apply.assert_not_called()

    def test_apply_control_fails_before_source_capture(self):
        with tempfile.TemporaryDirectory() as directory, \
             patch.dict(os.environ, {}, clear=True), \
             patch.object(product_backfill_operation.product_backfill, "capture_dataset_snapshot") as capture:
            with self.assertRaisesRegex(RuntimeError, "KAVEON_PRODUCT_MIGRATION_ENABLED"):
                product_backfill_operation.run(
                    Path(directory) / "checkpoint.json", apply=True, resume=False
                )
        capture.assert_not_called()

    def test_resume_starts_at_checkpoint_and_finishes_with_full_reconciliation(self):
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "checkpoint.json"
            product_backfill_operation.save_checkpoint(path, snapshot(), 1)
            reports = [
                {"created": 1, "already_present": 0},
                {
                    "family": "datasets", "source_watermark": 12, "source_count": 2,
                    "created": 0, "already_present": 2, "reconciled": 2,
                    "snapshot_sha256": snapshot().snapshot_sha256,
                    "max_target_generation": 4,
                },
            ]
            with patch.dict(os.environ, {"KAVEON_PRODUCT_MIGRATION_ENABLED": "true"}), \
                 patch.object(
                     product_backfill_operation.product_backfill,
                     "apply_and_reconcile",
                     side_effect=reports,
                 ) as apply:
                report = product_backfill_operation.run(path, apply=True, resume=True)
            restored, next_index, complete = product_backfill_operation.load_checkpoint(path)
        self.assertEqual(apply.call_args_list[0].args[0].records[0].record_id, "2")
        self.assertEqual(len(apply.call_args_list[1].args[0].records), 2)
        self.assertEqual((next_index, complete), (2, True))
        self.assertEqual(report["checkpoint_complete"], True)

    def test_checkpoint_failure_after_target_apply_leaves_position_for_exact_retry(self):
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "checkpoint.json"
            product_backfill_operation.save_checkpoint(path, snapshot(1), 0)
            with patch.dict(os.environ, {"KAVEON_PRODUCT_MIGRATION_ENABLED": "true"}), \
                 patch.object(
                     product_backfill_operation.product_backfill,
                     "apply_and_reconcile",
                     return_value={"created": 1, "already_present": 0},
                 ), patch.object(
                     product_backfill_operation,
                     "save_checkpoint",
                     side_effect=RuntimeError("injected checkpoint failure"),
                 ):
                with self.assertRaisesRegex(RuntimeError, "checkpoint failure"):
                    product_backfill_operation.run(path, apply=True, resume=True)
            _, next_index, complete = product_backfill_operation.load_checkpoint(path)
            reports = [
                {"created": 0, "already_present": 1},
                {
                    "family": "datasets", "source_watermark": 12, "source_count": 1,
                    "created": 0, "already_present": 1, "reconciled": 1,
                    "snapshot_sha256": snapshot(1).snapshot_sha256,
                    "max_target_generation": 4,
                },
            ]
            with patch.dict(os.environ, {"KAVEON_PRODUCT_MIGRATION_ENABLED": "true"}), \
                 patch.object(
                     product_backfill_operation.product_backfill,
                     "apply_and_reconcile",
                     side_effect=reports,
                 ) as retry:
                report = product_backfill_operation.run(path, apply=True, resume=True)
        self.assertEqual((next_index, complete), (0, False))
        self.assertEqual(retry.call_args_list[0].args[0].records[0].record_id, "1")
        self.assertTrue(report["checkpoint_complete"])


if __name__ == "__main__":
    unittest.main()
