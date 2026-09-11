import hashlib
import sys
import unittest
from pathlib import Path

sys.path.insert(0, str(Path(__file__).parent))
from table_publication_rehearsal import VerificationError, verify


def report():
    payload = b"PAR1fixturePAR1"
    sha = hashlib.sha256(payload).hexdigest()
    head = "a" * 64
    return {
        "kind": "kaveon.immutable_table_publication_rehearsal",
        "mutations_enabled": False,
        "manifest": {"path": "tables/orders/manifest.json", "sha256": "b" * 64, "parquet_files": [{"path": "tables/orders/part.parquet", "sha256": sha}]},
        "objects": [{"path": "tables/orders/part.parquet", "sha256": sha, "size_bytes": len(payload), "content_base64": __import__("base64").b64encode(payload).decode()}],
        "operations": [
            {"name": "committed", "outcome": "committed"},
            {"name": "replayed", "outcome": "replayed", "head_unchanged": True},
            {"name": "divergent_conflict", "outcome": "conflict", "head_unchanged": True},
            {"name": "stale_cas_conflict", "outcome": "conflict", "head_unchanged": True},
        ],
        "committed_head_sha256": head,
        "restart": {"reopened": True, "head_sha256": head, "journal_outcome": "committed"},
    }


class TablePublicationRehearsalTests(unittest.TestCase):
    def test_passes_complete_offline_evidence(self):
        result = verify(report())
        self.assertEqual(result["status"], "passed")

    def test_rejects_manifest_digest_mismatch(self):
        value = report()
        value["manifest"]["parquet_files"][0]["sha256"] = "c" * 64
        with self.assertRaises(VerificationError):
            verify(value)

    def test_rejects_mutating_report(self):
        value = report()
        value["mutations_enabled"] = True
        with self.assertRaises(VerificationError):
            verify(value)


if __name__ == "__main__":
    unittest.main()
