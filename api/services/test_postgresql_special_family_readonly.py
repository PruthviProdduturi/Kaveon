import hashlib
import json
import os
import tempfile
import unittest
from datetime import datetime, timezone
from pathlib import Path
from unittest.mock import patch

from services import postgresql_evidence_collector as collector
from services import postgresql_special_family_migration as migration
from services import postgresql_special_family_readonly as verifier
from services.test_postgresql_special_family_retirement import baseline


class Publisher:
    def __init__(self):
        self.values = {}

    def publish_immutable(self, path, body, sha256):
        self.values[path] = body
        table = json.loads(body)["table"]
        return {"path": path, "sha256": sha256, "status": "created", "bytes": len(body),
                "row_count": table["row_count"], "key_set_sha256": table["key_sha256"],
                "content_sha256": table["content_sha256"]}

    def publish_manifest(self, body, **_kwargs):
        digest = hashlib.sha256(body).hexdigest()
        self.values[f"manifests/{digest}.json"] = body
        self.values["head.json"] = json.dumps(
            {"manifest_path": f"manifests/{digest}.json", "sha256": digest},
            sort_keys=True, separators=(",", ":")).encode()
        return {"sha256": digest, "status": "committed", "cas_attempts": 1,
                "published_last": True}

    def read(self, path, _max_bytes):
        return self.values.get(path.removeprefix("qualified/"))

    def read_with_etag(self, path, _max_bytes):
        value = self.read(path, _max_bytes)
        return None if value is None else (value, '"head-etag"')


def fixture():
    value = baseline()
    identity = migration.verify_baseline(value)
    publisher = Publisher()
    evidence = migration.publish(value, expected_head="absent", publisher=publisher,
                                 source_pending_events=0)
    captured = {"snapshot_id": "900:900:", "watermark": 3,
                "table_identities": identity["table_identities"]}
    return value, evidence, captured, publisher, identity


class Tests(unittest.TestCase):
    def test_emits_schema_valid_non_destructive_reports_after_exact_readback(self):
        value, evidence, captured, publisher, identity = fixture()
        now = datetime(2026, 9, 21, 19, 0, tzinfo=timezone.utc)
        with tempfile.TemporaryDirectory() as directory, \
             patch.dict(os.environ, {"KAVEON_SPECIAL_FAMILY_READONLY_ENABLED": "true"}), \
             patch.object(verifier, "QUALIFIED_BASELINE_SHA256",
                          identity["baseline_evidence_id"]):
            result = verifier.run(baseline=value, migration_evidence=evidence,
                prefix="qualified", output_directory=Path(directory) / "reports",
                client=publisher, now=now, capture=lambda _: captured)
            output = Path(result["output_directory"])
            self.assertEqual({item.name for item in output.iterdir()}, {
                "context_cache.json", "dlm_generation.json",
                "read-only-special-verification.json"})
            context = json.loads((output / "context_cache.json").read_text())
            dlm = json.loads((output / "dlm_generation.json").read_text())
            self.assertEqual(set(context), collector.REPORT_KEYS)
            self.assertEqual(context["source_count"], 0)
            self.assertEqual(dlm["source_count"], 3960)
            self.assertEqual(dlm["target_count"], 3960)
            receipt = json.loads((output / "read-only-special-verification.json").read_text())
            self.assertFalse(receipt["writes_fenced"])
            self.assertEqual(receipt["rows_deleted"], 0)
            self.assertEqual(receipt["baseline_evidence_id"], identity["baseline_evidence_id"])

    def test_rejects_unqualified_baseline_before_capture_or_readback(self):
        value, evidence, _, publisher, _ = fixture()
        calls = []
        with patch.dict(os.environ, {"KAVEON_SPECIAL_FAMILY_READONLY_ENABLED": "true"}), \
             self.assertRaisesRegex(RuntimeError, "qualified Sep14"):
            verifier.run(baseline=value, migration_evidence=evidence, prefix="qualified",
                output_directory=Path("unused"), client=publisher,
                capture=lambda _: calls.append(1))
        self.assertEqual(calls, [])

    def test_rejects_live_identity_drift_and_target_readback_drift(self):
        value, evidence, captured, publisher, identity = fixture()
        with patch.dict(os.environ, {"KAVEON_SPECIAL_FAMILY_READONLY_ENABLED": "true"}), \
             patch.object(verifier, "QUALIFIED_BASELINE_SHA256",
                          identity["baseline_evidence_id"]):
            changed = json.loads(json.dumps(captured))
            changed["table_identities"][migration.TABLES[0]]["row_count"] += 1
            with self.assertRaisesRegex(RuntimeError, "live PostgreSQL"):
                verifier.run(baseline=value, migration_evidence=evidence, prefix="qualified",
                    output_directory=Path("unused"), client=publisher, capture=lambda _: changed)
            publisher.values[evidence["objects"][0]["path"]] = b"corrupt"
            with self.assertRaisesRegex(RuntimeError, "table readback mismatch"):
                verifier.run(baseline=value, migration_evidence=evidence, prefix="qualified",
                    output_directory=Path("unused"), client=publisher, capture=lambda _: captured)

    def test_disabled_path_performs_no_capture(self):
        calls = []
        with patch.dict(os.environ, {}, clear=True), \
             self.assertRaisesRegex(RuntimeError, "explicit enablement"):
            verifier.run(baseline={}, migration_evidence={}, prefix="qualified",
                output_directory=Path("unused"), client=object(),
                capture=lambda _: calls.append(1))
        self.assertEqual(calls, [])


if __name__ == "__main__":
    unittest.main()
