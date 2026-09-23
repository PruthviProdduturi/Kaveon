import importlib.util
import sys
from datetime import datetime, timezone
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "api"))
from services import postgresql_operational_evidence as evidence
from services import test_postgresql_operational_evidence as fixtures


PATH = Path(__file__).with_name("audit-postgresql-operational-receipts.py")
SPEC = importlib.util.spec_from_file_location("audit_operational_receipts", PATH)
MODULE = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(MODULE)


def write_receipts(directory, checked_at="2026-09-22T20:00:00Z"):
    values = fixtures._write_receipts(directory)
    for gate, value in values.items():
        value["checked_at"] = checked_at
        value["receipt_sha256"] = __import__("hashlib").sha256(
            evidence._canonical({key: item for key, item in value.items()
                                 if key != "receipt_sha256"})).hexdigest()
        (directory / f"{gate}.json").write_text(__import__("json").dumps(value), encoding="utf-8")


def test_audit_reports_every_gate_without_stopping_at_first_failure(tmp_path):
    write_receipts(tmp_path)
    (tmp_path / "rollback.json").unlink()
    (tmp_path / "backup_identity.json").write_text("{}", encoding="utf-8")
    result = MODULE.audit(tmp_path, now=datetime(2026, 9, 22, 20, 5, tzinfo=timezone.utc),
                          max_age_hours=24, max_rollback_seconds=900)
    by_gate = {item["gate"]: item for item in result["gates"]}
    assert result["passed"] is False
    assert len(result["gates"]) == len(evidence.GATES)
    assert by_gate["source_watermark"]["status"] == "passed"
    assert by_gate["rollback"]["status"] == "missing"
    assert by_gate["backup_identity"]["status"] == "failed"


def test_audit_passes_only_for_one_complete_fresh_set(tmp_path):
    write_receipts(tmp_path)
    result = MODULE.audit(tmp_path, now=datetime(2026, 9, 22, 20, 5, tzinfo=timezone.utc),
                          max_age_hours=24, max_rollback_seconds=900)
    assert result["passed"] is True
    assert result["passed_count"] == len(evidence.GATES)
