import json
import os
import tempfile
import unittest
from datetime import datetime, timezone
from pathlib import Path
from unittest.mock import patch

from services import postgresql_retirement_gate as gate
from services import postgresql_special_family_retirement as retirement
from services import postgresql_special_family_migration as lossless
from services import postgresql_baseline_identity as canonical


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
            "identities": baseline_identity()["table_identities"],
            "before": before, "deleted": dict(before),
            "remaining": {table: 0 for table in before}}


def baseline():
    tables = []
    counts = {table: 0 for table in lossless.TABLES}
    counts.update({"dlm_answers": 3866, "dlm_artifact": 10,
                   "dlm_router": 10, "dlm_value_index": 74})
    # Unit tests use precomputed identities because materializing thousands of
    # fixture rows obscures the retirement contract under test.
    for table in lossless.TABLES:
        columns = [{"name": "id", "type": "integer", "nullable": False, "ordinal": 1}]
        rows = [{"id": index} for index in range(counts[table])]
        tables.append(canonical.table_identity(table, columns, ["id"], rows))
    return canonical.build("pg-snapshot-1", tables)


def baseline_identity():
    return lossless.verify_baseline(baseline())


def migration_evidence():
    class Publisher:
        def publish_immutable(self, path, body, sha256):
            found = json.loads(body)["table"]
            return {"path": path, "sha256": sha256, "status": "created", "bytes": len(body),
                    "row_count": found["row_count"], "key_set_sha256": found["key_sha256"],
                    "content_sha256": found["content_sha256"]}

        def publish_manifest(self, body, **_kwargs):
            return {"sha256": __import__("hashlib").sha256(body).hexdigest(),
                    "status": "committed", "cas_attempts": 1, "published_last": True}
    return lossless.publish(baseline(), expected_head="head:1", publisher=Publisher())


def lossless_args():
    return {"baseline": baseline(), "migration_evidence": migration_evidence()}


class Tests(unittest.TestCase):
    def test_operator_sql_matches_restored_postgresql_schema(self):
        api_root = Path(__file__).resolve().parents[1]
        schema = (api_root / "schema_postgresql.sql").read_text(encoding="utf-8")
        dlm_schema = (api_root / "dlm" / "engine.py").read_text(encoding="utf-8")
        declared = schema + "\n" + dlm_schema
        for table in (*retirement.CONTEXT_TABLES, *retirement.DLM_TABLES,
                      "product_migration_outbox"):
            self.assertIn(f"CREATE TABLE IF NOT EXISTS {table} (", declared)
        outbox_block = schema[schema.index("CREATE TABLE IF NOT EXISTS product_migration_outbox ("):
                              schema.index(");", schema.index(
                                  "CREATE TABLE IF NOT EXISTS product_migration_outbox ("))]
        self.assertIn("source_sequence BIGSERIAL", outbox_block)
        self.assertNotRegex(outbox_block, r"(?m)^\s*id\s+")
        self.assertEqual(retirement.OUTBOX_WATERMARK_SQL,
            "SELECT COALESCE(MAX(source_sequence),0) FROM product_migration_outbox")

    def test_refuses_before_live_fence_without_calling_delete(self):
        called = []
        with patch.dict(os.environ, {"KAVEON_SPECIAL_FAMILY_RETIREMENT_ENABLED": "true",
                                    "KAVEON_POSTGRESQL_WRITE_FENCE_ENABLED": "false"}, clear=True), \
             self.assertRaisesRegex(RuntimeError, "fence is not enabled"):
            retirement.run(bundle={}, rebuild={}, fence_observation=fence(),
                           expected_counts=captured()["before"], output_directory=Path("unused"),
                           delete_runner=lambda *_: called.append(1), **lossless_args())
        self.assertEqual(called, [])

    def test_requires_complete_live_fence_receipt_before_delete(self):
        value = fence(); value["family_probes"].pop()
        called = []
        with patch.dict(os.environ, {"KAVEON_SPECIAL_FAMILY_RETIREMENT_ENABLED": "true",
                                    "KAVEON_POSTGRESQL_WRITE_FENCE_ENABLED": "true"}, clear=True), \
             self.assertRaisesRegex(RuntimeError, "complete live write-fence"):
            retirement.run(bundle={}, rebuild={}, fence_observation=value,
                           expected_counts=captured()["before"], output_directory=Path("unused"),
                           delete_runner=lambda *_: called.append(1), **lossless_args())
        self.assertEqual(called, [])

    def test_rejects_bad_rebuild_before_delete(self):
        value = rebuild(); value["repeat_probe_sha256"] = "e" * 64
        called = []
        with patch.dict(os.environ, {"KAVEON_SPECIAL_FAMILY_RETIREMENT_ENABLED": "true",
                                    "KAVEON_POSTGRESQL_WRITE_FENCE_ENABLED": "true"}, clear=True), \
             self.assertRaisesRegex(RuntimeError, "deterministic context-cache rebuild"):
            retirement.run(bundle={}, rebuild=value, fence_observation=fence(),
                           expected_counts=captured()["before"], output_directory=Path("unused"),
                           delete_runner=lambda *_: called.append(1), **lossless_args())
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
                           delete_runner=lambda *_: called.append(1), **lossless_args())
        self.assertEqual(called, [])

    def test_lossless_identity_mismatch_refuses_before_delete(self):
        called = []
        evidence = migration_evidence()
        evidence["tables"][0]["target"]["content_sha256"] = "0" * 64
        with tempfile.TemporaryDirectory() as directory, \
             patch.dict(os.environ, {"KAVEON_SPECIAL_FAMILY_RETIREMENT_ENABLED": "true",
                                     "KAVEON_POSTGRESQL_WRITE_FENCE_ENABLED": "true"}, clear=True), \
             patch.object(retirement.dlm_migration_evidence, "verify", return_value={
                 "passed": True, "bundle_sha256": "d" * 64, "definition_count": 10,
                 "run_count": 10, "target_snapshot_id": "snapshot-1"}), \
             self.assertRaisesRegex(RuntimeError, "source and target identities differ"):
            retirement.run(bundle={}, rebuild=rebuild(), fence_observation=fence(),
                output_directory=Path(directory) / "special", expected_counts=captured()["before"],
                baseline=baseline(), migration_evidence=evidence,
                delete_runner=lambda *_: called.append(1))
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
                delete_runner=lambda *_: captured(), **lossless_args())
            self.assertTrue(result["passed"])
            output = Path(result["output_directory"])
            self.assertEqual({p.name for p in output.iterdir()}, {
                "context-cache-live.json", "dlm-generation-live.json",
                "context_cache.json", "dlm_generation.json", "special-family-lossless.json"})
            dlm = json.loads((output / "dlm-generation-live.json").read_text())
            self.assertEqual(dlm["source"]["rows"]["dlm_answers"], 3866)
            self.assertEqual(dlm["deletion"]["remaining_rows"]["dlm_answers"], 0)
            self.assertEqual(dlm["deletion"]["target_snapshot_id"], "snapshot-1")


if __name__ == "__main__": unittest.main()
