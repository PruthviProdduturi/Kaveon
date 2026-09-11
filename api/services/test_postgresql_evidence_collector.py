import hashlib
import json
import os
import tempfile
import unittest
from pathlib import Path
from unittest.mock import patch

from services import postgresql_evidence_collector as collector
from services import postgresql_retirement_gate as gate


def report(family, tables, index=0):
    value = {
        "schema_version": 1,
        "family": family,
        "tables": list(tables),
        "status": "passed",
        "reconciled_at": "2026-09-10T19:00:00Z",
        "source_watermark": index,
        "source_count": index,
        "target_count": index,
        "checks": {name: True for name in gate.REQUIRED_CHECKS},
        "provenance": {
            "producer": "reconciler@abc123",
            "source_snapshot": f"postgresql:{index}",
            "target_snapshot": f"kaveondb:{index}",
        },
    }
    value["report_sha256"] = hashlib.sha256(collector._canonical(value)).hexdigest()
    return value


class EvidenceCollectorTests(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory()
        self.directory = Path(self.temporary.name)
        for index, (family, tables) in enumerate(gate.AUTHORITY_FAMILIES.items()):
            (self.directory / f"{family}.json").write_text(
                json.dumps(report(family, tables, index)), encoding="utf-8"
            )

    def tearDown(self):
        self.temporary.cleanup()

    def test_disabled_before_reading_reports(self):
        with patch.dict(os.environ, {}, clear=True), patch.object(Path, "is_file") as is_file:
            with self.assertRaisesRegex(RuntimeError, "collection requires"):
                collector.collect(self.directory)
        is_file.assert_not_called()

    def test_collects_exact_verified_family_set_accepted_by_gate(self):
        with patch.dict(os.environ, {"KAVEON_RETIREMENT_EVIDENCE_COLLECTION_ENABLED": "true"}):
            evidence = collector.collect(self.directory)
        audit = gate.evaluate(
            evidence,
            now=__import__("datetime").datetime(2026, 9, 10, 20, tzinfo=__import__("datetime").timezone.utc),
            max_age_hours=24,
        )
        self.assertTrue(audit["passed"])
        self.assertEqual(len(evidence["families"]), len(gate.AUTHORITY_FAMILIES))

    def test_missing_report_fails_closed(self):
        (self.directory / "charts.json").unlink()
        with patch.dict(os.environ, {"KAVEON_RETIREMENT_EVIDENCE_COLLECTION_ENABLED": "true"}):
            with self.assertRaisesRegex(RuntimeError, "missing or oversized"):
                collector.collect(self.directory)

    def test_tampered_report_fails_closed(self):
        path = self.directory / "datasets.json"
        value = json.loads(path.read_text(encoding="utf-8"))
        value["target_count"] += 1
        path.write_text(json.dumps(value), encoding="utf-8")
        with patch.dict(os.environ, {"KAVEON_RETIREMENT_EVIDENCE_COLLECTION_ENABLED": "true"}):
            with self.assertRaisesRegex(RuntimeError, "digest mismatch"):
                collector.collect(self.directory)

    def test_wrong_family_and_extra_payload_fail_closed(self):
        path = self.directory / "datasets.json"
        for mutation, message in (
            (lambda value: value.update(family="charts"), "identity mismatch"),
            (lambda value: value.update(rows=[{"private": "value"}]), "unexpected.*schema"),
        ):
            with self.subTest(message=message):
                value = report("datasets", gate.AUTHORITY_FAMILIES["datasets"])
                mutation(value)
                if "rows" not in value:
                    unsigned = {k: v for k, v in value.items() if k != "report_sha256"}
                    value["report_sha256"] = hashlib.sha256(collector._canonical(unsigned)).hexdigest()
                path.write_text(json.dumps(value), encoding="utf-8")
                with patch.dict(os.environ, {"KAVEON_RETIREMENT_EVIDENCE_COLLECTION_ENABLED": "true"}):
                    with self.assertRaisesRegex(RuntimeError, message):
                        collector.collect(self.directory)


if __name__ == "__main__":
    unittest.main()
