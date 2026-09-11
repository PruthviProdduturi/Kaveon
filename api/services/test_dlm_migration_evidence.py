import hashlib
import json
import os
import tempfile
import unittest
import sys
from datetime import datetime, timedelta, timezone
from pathlib import Path
from types import SimpleNamespace
from unittest.mock import patch

if "pyodbc" not in sys.modules:
    sys.modules["pyodbc"] = SimpleNamespace(Error=Exception)

from services import dlm_migration_evidence as evidence

NOW = datetime(2026, 9, 11, 12, tzinfo=timezone.utc)


def snapshots():
    definition_record = SimpleNamespace(record_id="7")
    definition = SimpleNamespace(source_watermark=10, snapshot_sha256="a" * 64,
                                 dataset_snapshot_id="dataset-snap", records=(definition_record,))
    run_record = SimpleNamespace(record_id="7-v2", document={"definition_id": "7",
        "definition_revision": 4, "artifact": {
        "path": "dlm/7/v2/manifest.json", "sha256": "b" * 64}})
    run = SimpleNamespace(source_watermark=11, snapshot_sha256="c" * 64,
                         definition_snapshot_id="definition-snap", records=(run_record,))
    return definition, run


def inputs(root: Path):
    values = {
        "artifact-receipts.json": [{"path": "dlm/7/v2/manifest.json", "sha256": "b" * 64,
                                     "bytes": 12, "status": "verified"}],
        "definition-report.json": {"source_count": 1, "reconciled": 1, "snapshot_sha256": "a" * 64},
        "run-report.json": {"source_count": 1, "reconciled": 1, "snapshot_sha256": "c" * 64},
        "target-observations.json": {"snapshot_id": "final-snap",
                                     "definitions": [{"id": "7", "generation": 4}],
                                     "runs": [{"id": "7-v2", "generation": 2}]},
    }
    paths = {}
    for name, value in values.items():
        path = root / name
        path.write_text(json.dumps(value))
        paths[name] = path
    for name in ("definitions.json", "runs.json"):
        path = root / name
        path.write_text("checkpoint")
        paths[name] = path
    return paths


class DlmMigrationEvidenceTests(unittest.TestCase):
    def test_collect_binds_complete_checkpoints_artifacts_generations_and_reports(self):
        definition, run = snapshots()
        with tempfile.TemporaryDirectory() as temporary:
            paths = inputs(Path(temporary))
            with patch.dict(os.environ, {"KAVEON_DLM_REHEARSAL_EVIDENCE_ENABLED": "true"}), \
                 patch.object(evidence.definitions, "load", return_value=(definition, 1, True)), \
                 patch.object(evidence.runs, "load", return_value=(run, 1, True)):
                bundle = evidence.collect(paths["definitions.json"], paths["runs.json"],
                                          paths["artifact-receipts.json"], paths["definition-report.json"],
                                          paths["run-report.json"], paths["target-observations.json"],
                                          collected_at=NOW)
        result = evidence.verify(bundle, now=NOW + timedelta(minutes=10), max_age_hours=1)
        self.assertTrue(result["passed"])
        self.assertEqual((result["definition_count"], result["run_count"]), (1, 1))
        self.assertEqual(bundle["checkpoints"]["definition_sha256"],
                         hashlib.sha256(b"checkpoint").hexdigest())

    def test_collection_is_disabled_and_rejects_incomplete_checkpoint(self):
        definition, run = snapshots()
        with tempfile.TemporaryDirectory() as temporary:
            paths = inputs(Path(temporary))
            args = (paths["definitions.json"], paths["runs.json"], paths["artifact-receipts.json"],
                    paths["definition-report.json"], paths["run-report.json"],
                    paths["target-observations.json"])
            with patch.dict(os.environ, {}, clear=True), self.assertRaisesRegex(RuntimeError, "requires"):
                evidence.collect(*args, collected_at=NOW)
            with patch.dict(os.environ, {"KAVEON_DLM_REHEARSAL_EVIDENCE_ENABLED": "true"}), \
                 patch.object(evidence.definitions, "load", return_value=(definition, 0, False)), \
                 patch.object(evidence.runs, "load", return_value=(run, 1, True)), \
                 self.assertRaisesRegex(RuntimeError, "incomplete"):
                evidence.collect(*args, collected_at=NOW)

    def test_verifier_rejects_stale_tampered_missing_and_mismatched_evidence(self):
        definition, run = snapshots()
        with tempfile.TemporaryDirectory() as temporary:
            paths = inputs(Path(temporary))
            with patch.dict(os.environ, {"KAVEON_DLM_REHEARSAL_EVIDENCE_ENABLED": "true"}), \
                 patch.object(evidence.definitions, "load", return_value=(definition, 1, True)), \
                 patch.object(evidence.runs, "load", return_value=(run, 1, True)):
                bundle = evidence.collect(paths["definitions.json"], paths["runs.json"],
                                          paths["artifact-receipts.json"], paths["definition-report.json"],
                                          paths["run-report.json"], paths["target-observations.json"],
                                          collected_at=NOW)
        with self.assertRaisesRegex(RuntimeError, "stale"):
            evidence.verify(bundle, now=NOW + timedelta(hours=2), max_age_hours=1)
        cases = []
        tampered = json.loads(json.dumps(bundle)); tampered["source"]["run_watermark"] = 99
        cases.append((tampered, "identity mismatch"))
        missing = json.loads(json.dumps(bundle)); missing.pop("artifact_receipts"); missing.pop("bundle_sha256")
        cases.append((missing, "schema"))
        mismatch = json.loads(json.dumps(bundle)); mismatch.pop("bundle_sha256")
        mismatch["reconciliation"]["runs"]["snapshot_sha256"] = "0" * 64
        cases.append((mismatch, "reconciliation is mismatched"))
        generations = json.loads(json.dumps(bundle)); generations.pop("bundle_sha256")
        generations["target_observations"]["runs"] = []
        cases.append((generations, "generation coverage"))
        for value, message in cases:
            with self.subTest(message=message), self.assertRaisesRegex(RuntimeError, message):
                evidence.verify(value, now=NOW, max_age_hours=1)


if __name__ == "__main__": unittest.main()
