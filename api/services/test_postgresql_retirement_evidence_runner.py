import hashlib
import importlib.util
import json
import os
import tempfile
import unittest
from datetime import datetime, timezone
from pathlib import Path
from unittest.mock import patch

from services import postgresql_evidence_collector as collector
from services import postgresql_retirement_gate as gate


RUNNER_PATH = Path(__file__).resolve().parents[2] / "scripts" / "run-postgresql-retirement-evidence.py"
SPEC = importlib.util.spec_from_file_location("retirement_evidence_runner", RUNNER_PATH)
runner = importlib.util.module_from_spec(SPEC)
assert SPEC.loader is not None
SPEC.loader.exec_module(runner)


def _reports(directory: Path) -> None:
    for index, (family, tables) in enumerate(gate.AUTHORITY_FAMILIES.items()):
        report = {
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
                "producer": "runner-test",
                "source_snapshot": f"postgresql:{index}",
                "target_snapshot": f"kaveondb:{index}",
            },
        }
        report["report_sha256"] = hashlib.sha256(collector._canonical(report)).hexdigest()
        (directory / f"{family}.json").write_text(json.dumps(report), encoding="utf-8")
    gates = {
        "source_watermark": {"status": "passed", "checked_at": "2026-09-10T19:00:00Z", "evidence_id": "watermark", "details": {"watermark": 21}},
        "outbox_drain": {"status": "passed", "checked_at": "2026-09-10T19:00:00Z", "evidence_id": "outbox", "details": {"pending_events": 0}},
        "write_fence": {"status": "passed", "checked_at": "2026-09-10T19:00:00Z", "evidence_id": "fence", "details": {"enabled": True}},
        "shadow_parity": {"status": "passed", "checked_at": "2026-09-10T19:00:00Z", "evidence_id": "shadow", "details": {"matched": True}},
        "restart_recovery": {"status": "passed", "checked_at": "2026-09-10T19:00:00Z", "evidence_id": "restart", "details": {"verified": True}},
        "rollback": {"status": "passed", "checked_at": "2026-09-10T19:00:00Z", "evidence_id": "rollback", "details": {"verified": True}},
        "backup_identity": {"status": "passed", "checked_at": "2026-09-10T19:00:00Z", "evidence_id": "backup", "details": {"backup_id": "snapshot-1", "backup_sha256": "a" * 64, "restore_verified": True}},
    }
    (directory / "retirement-gates.json").write_text(json.dumps(gates), encoding="utf-8")


class RetirementEvidenceRunnerTests(unittest.TestCase):
    def test_publishes_only_after_all_reports_and_gates_pass(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            reports = root / "reports"
            reports.mkdir()
            _reports(reports)
            evidence = root / "evidence.json"
            audit = root / "audit.json"
            with patch.dict(os.environ, {"KAVEON_RETIREMENT_EVIDENCE_COLLECTION_ENABLED": "true"}):
                result = runner.run(
                    reports, evidence, audit,
                    now=datetime(2026, 9, 10, 20, tzinfo=timezone.utc),
                    max_age_hours=24,
                )
            self.assertTrue(result["passed"])
            self.assertEqual(result["authority_family_count"], len(gate.AUTHORITY_FAMILIES))
            self.assertEqual(result["global_gate_count"], len(gate.GLOBAL_GATE_NAMES))
            self.assertTrue(evidence.is_file())
            self.assertTrue(audit.is_file())

    def test_stale_gate_does_not_publish_outputs(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            reports = root / "reports"
            reports.mkdir()
            _reports(reports)
            gates_path = reports / "retirement-gates.json"
            gates = json.loads(gates_path.read_text(encoding="utf-8"))
            gates["outbox_drain"]["checked_at"] = "2026-09-08T19:00:00Z"
            gates_path.write_text(json.dumps(gates), encoding="utf-8")
            evidence = root / "evidence.json"
            audit = root / "audit.json"
            with patch.dict(os.environ, {"KAVEON_RETIREMENT_EVIDENCE_COLLECTION_ENABLED": "true"}):
                with self.assertRaisesRegex(RuntimeError, "outbox_drain retirement gate evidence is not fresh"):
                    runner.run(
                        reports, evidence, audit,
                        now=datetime(2026, 9, 10, 20, tzinfo=timezone.utc),
                        max_age_hours=24,
                    )
            self.assertFalse(evidence.exists())
            self.assertFalse(audit.exists())


if __name__ == "__main__":
    unittest.main()
