import os
import unittest
from datetime import datetime, timedelta, timezone
from unittest.mock import patch

from services import context_cache_retirement as retirement
from services import postgresql_evidence_collector as collector


NOW = datetime(2026, 9, 14, 18, tzinfo=timezone.utc)


def evidence():
    return {
        "schema_version": 1,
        "observed_at": "2026-09-14T17:45:00Z",
        "source": {"snapshot_id": "pg-snapshot-7", "watermark": 44,
                   "snapshot_rows": 12, "cache_rows": 31,
                   "snapshot_schema_sha256": "a" * 64, "cache_schema_sha256": "b" * 64},
        "rebuild": {"target_snapshot_id": "kdb-generation-9",
                    "dataset_revision_sha256": "c" * 64, "active_datasets": 4,
                    "covered_datasets": 4, "failed_datasets": 0,
                    "first_probe_sha256": "d" * 64, "repeat_probe_sha256": "d" * 64},
        "deletion": {"writes_fenced": True, "deleted_snapshot_rows": 12,
                     "deleted_cache_rows": 31, "remaining_snapshot_rows": 0,
                     "remaining_cache_rows": 0, "verified_at": "2026-09-14T17:55:00Z"},
    }


class ContextCacheRetirementTests(unittest.TestCase):
    def build(self, value=None):
        with patch.dict(os.environ, {"KAVEON_CONTEXT_CACHE_RETIREMENT_ENABLED": "true"}):
            return retirement.build_report(value or evidence(), now=NOW, max_age_hours=1)

    def test_emits_collector_compatible_metadata_only_report(self):
        report = self.build()
        self.assertEqual(report["family"], "context_cache")
        self.assertEqual((report["source_count"], report["target_count"]), (0, 0))
        self.assertEqual(set(report), collector.REPORT_KEYS)
        unsigned = {key: value for key, value in report.items() if key != "report_sha256"}
        self.assertEqual(report["report_sha256"], retirement.hashlib.sha256(collector._canonical(unsigned)).hexdigest())
        encoded = retirement._canonical(report).decode()
        self.assertNotIn("cached result", encoded)

    def test_is_disabled_by_default_and_rejects_payload_fields(self):
        with patch.dict(os.environ, {}, clear=True), self.assertRaisesRegex(RuntimeError, "explicit enablement"):
            retirement.build_report(evidence(), now=NOW, max_age_hours=1)
        value = evidence(); value["source"]["question_text"] = "private"
        with self.assertRaisesRegex(RuntimeError, "forbidden field"):
            self.build(value)

    def test_requires_complete_deterministic_rebuild(self):
        for mutation, message in (
            (lambda value: value["rebuild"].update(covered_datasets=3), "coverage"),
            (lambda value: value["rebuild"].update(failed_datasets=1), "coverage"),
            (lambda value: value["rebuild"].update(repeat_probe_sha256="e" * 64), "deterministic"),
        ):
            with self.subTest(message=message):
                value = evidence(); mutation(value)
                with self.assertRaisesRegex(RuntimeError, message): self.build(value)

    def test_requires_fence_exact_deletion_and_zero_remaining_rows(self):
        for mutation, message in (
            (lambda value: value["deletion"].update(writes_fenced=False), "not fenced"),
            (lambda value: value["deletion"].update(deleted_cache_rows=30), "do not match"),
            (lambda value: value["deletion"].update(remaining_snapshot_rows=1), "rows remain"),
        ):
            with self.subTest(message=message):
                value = evidence(); mutation(value)
                with self.assertRaisesRegex(RuntimeError, message): self.build(value)

    def test_rejects_stale_future_and_oversized_counts(self):
        with self.assertRaisesRegex(RuntimeError, "not fresh"):
            with patch.dict(os.environ, {"KAVEON_CONTEXT_CACHE_RETIREMENT_ENABLED": "true"}):
                retirement.build_report(evidence(), now=NOW + timedelta(hours=2), max_age_hours=1)
        value = evidence(); value["source"]["cache_rows"] = retirement.MAX_ROWS + 1
        with self.assertRaisesRegex(RuntimeError, "row count"):
            self.build(value)
        value = evidence(); value["deletion"]["verified_at"] = "2026-09-14T18:05:00Z"
        with self.assertRaisesRegex(RuntimeError, "not fresh"):
            self.build(value)


if __name__ == "__main__": unittest.main()
