import json
import os
import tempfile
import unittest
from datetime import datetime, timezone
from pathlib import Path
from unittest.mock import patch

from services import postgresql_retirement_gate as gate
from services import postgresql_special_family_retirement as retirement


def fence():
    return {"deployment_revision": "api-7", "readonly_probe_passed": True,
            "family_probes": [{"family": family, "passed": True}
                              for family in sorted(gate.AUTHORITY_FAMILIES)]}


def rebuild():
    return {"target_snapshot_id": "snapshot-1", "dataset_revision_sha256": "a" * 64,
            "active_datasets": 10, "covered_datasets": 10, "failed_datasets": 0,
            "first_probe_sha256": "b" * 64, "repeat_probe_sha256": "b" * 64}


def captured():
    before = {table: 0 for table in (*retirement.CONTEXT_TABLES, *retirement.DLM_TABLES)}
    before.update({"dlm_answers": 3866, "dlm_artifact": 10,
                   "dlm_router": 10, "dlm_value_index": 74})
    return {"snapshot_id": "pg-snapshot-1", "watermark": 3,
            "schemas": {table: "c" * 64 for table in before},
            "before": before, "deleted": dict(before),
            "remaining": {table: 0 for table in before}}


class Tests(unittest.TestCase):
    def test_refuses_before_live_fence_without_calling_delete(self):
        called = []
        with patch.dict(os.environ, {"KAVEON_SPECIAL_FAMILY_RETIREMENT_ENABLED": "true",
                                    "KAVEON_POSTGRESQL_WRITE_FENCE_ENABLED": "false"}, clear=True), \
             self.assertRaisesRegex(RuntimeError, "fence is not enabled"):
            retirement.run(bundle={}, rebuild={}, fence_observation=fence(),
                           expected_counts=captured()["before"], output_directory=Path("unused"),
                           delete_runner=lambda _expected: called.append(1))
        self.assertEqual(called, [])

    def test_requires_complete_live_fence_receipt_before_delete(self):
        value = fence(); value["family_probes"].pop()
        called = []
        with patch.dict(os.environ, {"KAVEON_SPECIAL_FAMILY_RETIREMENT_ENABLED": "true",
                                    "KAVEON_POSTGRESQL_WRITE_FENCE_ENABLED": "true"}, clear=True), \
             self.assertRaisesRegex(RuntimeError, "complete live write-fence"):
            retirement.run(bundle={}, rebuild={}, fence_observation=value,
                           expected_counts=captured()["before"], output_directory=Path("unused"),
                           delete_runner=lambda _expected: called.append(1))
        self.assertEqual(called, [])

    def test_rejects_bad_rebuild_before_delete(self):
        value = rebuild(); value["repeat_probe_sha256"] = "e" * 64
        called = []
        with patch.dict(os.environ, {"KAVEON_SPECIAL_FAMILY_RETIREMENT_ENABLED": "true",
                                    "KAVEON_POSTGRESQL_WRITE_FENCE_ENABLED": "true"}, clear=True), \
             self.assertRaisesRegex(RuntimeError, "deterministic context-cache rebuild"):
            retirement.run(bundle={}, rebuild=value, fence_observation=fence(),
                           expected_counts=captured()["before"], output_directory=Path("unused"),
                           delete_runner=lambda _expected: called.append(1))
        self.assertEqual(called, [])

    def test_requires_one_target_snapshot_before_delete(self):
        called = []
        verified = {"passed": True, "bundle_sha256": "d" * 64,
                    "definition_count": 10, "run_count": 10,
                    "target_snapshot_id": "another-snapshot"}
        with tempfile.TemporaryDirectory() as directory, \
             patch.dict(os.environ, {"KAVEON_SPECIAL_FAMILY_RETIREMENT_ENABLED": "true",
                                     "KAVEON_POSTGRESQL_WRITE_FENCE_ENABLED": "true"}, clear=True), \
             patch.object(retirement.dlm_migration_evidence, "verify", return_value=verified), \
             self.assertRaisesRegex(RuntimeError, "target snapshots do not match"):
            retirement.run(bundle={}, rebuild=rebuild(), fence_observation=fence(),
                           output_directory=Path(directory) / "special",
                           expected_counts=captured()["before"],
                           delete_runner=lambda _expected: called.append(1))
        self.assertEqual(called, [])

    def test_emits_both_observations_and_reports_after_verified_delete(self):
        verified = {"passed": True, "bundle_sha256": "d" * 64,
                    "definition_count": 10, "run_count": 10,
                    "target_snapshot_id": "snapshot-1"}
        now = datetime.now(timezone.utc)
        with tempfile.TemporaryDirectory() as directory, \
             patch.dict(os.environ, {"KAVEON_SPECIAL_FAMILY_RETIREMENT_ENABLED": "true",
                                     "KAVEON_POSTGRESQL_WRITE_FENCE_ENABLED": "true"}, clear=True), \
             patch.object(retirement.dlm_migration_evidence, "verify", return_value=verified):
            result = retirement.run(bundle={"safe": True}, rebuild=rebuild(),
                fence_observation=fence(), output_directory=Path(directory) / "special",
                expected_counts=captured()["before"], now=now,
                delete_runner=lambda _expected: captured())
            self.assertTrue(result["passed"])
            output = Path(result["output_directory"])
            self.assertEqual({p.name for p in output.iterdir()}, {
                "context-cache-live.json", "dlm-generation-live.json",
                "context_cache.json", "dlm_generation.json"})
            dlm = json.loads((output / "dlm-generation-live.json").read_text())
            self.assertEqual(dlm["source"]["rows"]["dlm_answers"], 3866)
            self.assertEqual(dlm["deletion"]["remaining_rows"]["dlm_answers"], 0)
            self.assertEqual(dlm["deletion"]["target_snapshot_id"], "snapshot-1")


if __name__ == "__main__": unittest.main()
