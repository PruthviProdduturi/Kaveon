import copy
import hashlib
import unittest
from datetime import datetime, timezone

from services import dataset_migration_rehearsal as rehearsal


NOW = datetime(2026, 9, 11, 18, 0, tzinfo=timezone.utc)


def receipt():
    value = {
        "schema_version": 1,
        "family": "datasets",
        "run_id": "aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa",
        "completed_at": "2026-09-11T17:00:00Z",
        "source_watermark": 42,
        "replay": {"first_sequence": 1, "last_sequence": 42, "pending_events": 0, "failed_events": 0},
        "parity": {
            "watermark": 42,
            "source_count": 8,
            "target_count": 8,
            "checks": {name: True for name in rehearsal.PARITY_CHECKS},
            "report_sha256": "1" * 64,
        },
        "fencing": {"source_writes_fenced": True, "fenced_watermark": 42},
        "restart": {"head_before": "2" * 64, "head_after": "2" * 64, "parity_passed": True},
        "rollback": {
            "target_writes_fenced": True,
            "source_reads_restored": True,
            "source_writes_restored": True,
            "duration_seconds": 12,
        },
    }
    value["receipt_sha256"] = hashlib.sha256(rehearsal._canonical(value)).hexdigest()
    return value


class DatasetMigrationRehearsalTests(unittest.TestCase):
    def test_complete_receipt_passes_deterministically(self):
        result = rehearsal.verify(receipt(), now=NOW, max_age_hours=24, max_rollback_seconds=60)
        self.assertTrue(result["passed"])
        self.assertEqual(result["source_watermark"], 42)

    def test_each_required_live_gate_fails_closed(self):
        mutations = (
            lambda value: value["replay"].update(pending_events=1),
            lambda value: value["parity"]["checks"].update(references=False),
            lambda value: value["fencing"].update(source_writes_fenced=False),
            lambda value: value["restart"].update(head_after="3" * 64),
            lambda value: value["rollback"].update(source_reads_restored=False),
            lambda value: value["rollback"].update(duration_seconds=61),
        )
        for mutate in mutations:
            with self.subTest(mutate=mutate):
                value = receipt()
                mutate(value)
                unsigned = {key: item for key, item in value.items() if key != "receipt_sha256"}
                value["receipt_sha256"] = hashlib.sha256(rehearsal._canonical(unsigned)).hexdigest()
                with self.assertRaises(RuntimeError):
                    rehearsal.verify(value, now=NOW, max_age_hours=24, max_rollback_seconds=60)

    def test_tampering_staleness_and_extra_fields_fail_closed(self):
        tampered = receipt()
        tampered["source_watermark"] = 43
        with self.assertRaisesRegex(RuntimeError, "digest mismatch"):
            rehearsal.verify(tampered, now=NOW, max_age_hours=24, max_rollback_seconds=60)
        stale = receipt()
        with self.assertRaisesRegex(RuntimeError, "not fresh"):
            rehearsal.verify(stale, now=datetime(2026, 9, 13, tzinfo=timezone.utc), max_age_hours=24, max_rollback_seconds=60)
        extra = receipt()
        extra["rows"] = []
        with self.assertRaisesRegex(RuntimeError, "schema is invalid"):
            rehearsal.verify(extra, now=NOW, max_age_hours=24, max_rollback_seconds=60)

    def test_sensitive_fields_are_rejected_before_digest_validation(self):
        value = copy.deepcopy(receipt())
        value["rollback"]["access_token"] = "redacted"
        with self.assertRaisesRegex(RuntimeError, "forbidden field"):
            rehearsal.verify(value, now=NOW, max_age_hours=24, max_rollback_seconds=60)


if __name__ == "__main__":
    unittest.main()
