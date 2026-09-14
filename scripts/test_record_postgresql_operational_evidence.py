import importlib.util
import json
from pathlib import Path
from types import SimpleNamespace

import pytest

from services import postgresql_operational_evidence as evidence
from services import test_postgresql_operational_evidence as fixtures


SCRIPT = Path(__file__).with_name("record-postgresql-operational-evidence.py")
SPEC = importlib.util.spec_from_file_location("record_operational", SCRIPT)
module = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(module)


def _manifest():
    return {"schema_version": 1, "run_id": "aks-retirement-1", "probes": {
        gate: {"argv": ["probe", gate], "timeout_seconds": 30}
        for gate in evidence.GATES
    }}


def _runner(observations, *, failing_gate=None):
    def run(argv, **kwargs):
        assert kwargs["shell"] is False
        gate = argv[1]
        return SimpleNamespace(
            returncode=3 if gate == failing_gate else 0,
            stdout=json.dumps(observations[gate][1]).encode(), stderr=b"probe failure")
    return run


def test_recorder_executes_each_probe_and_signs_derived_success():
    receipts = module.record(_manifest(), checked_at="2026-09-14T18:00:00Z",
                             runner=_runner(fixtures._observations()))
    assert set(receipts) == set(evidence.GATES)
    assert receipts["source_watermark"]["details"] == {"watermark": 42}
    assert receipts["outbox_drain"]["details"] == {"pending_events": 0}
    assert receipts["write_fence"]["details"] == {"enabled": True}
    assert receipts["backup_identity"]["details"]["backup_id"] == "snapshot-1"
    for gate, receipt in receipts.items():
        assert receipt["gate"] == gate
        assert len(receipt["receipt_sha256"]) == 64


def test_failed_command_cannot_produce_receipts():
    with pytest.raises(RuntimeError, match="failed for shadow_parity"):
        module.record(_manifest(), checked_at="2026-09-14T18:00:00Z",
                      runner=_runner(fixtures._observations(), failing_gate="shadow_parity"))


def test_successful_command_with_failed_observation_cannot_be_signed():
    observations = fixtures._observations()
    observations["outbox_drain"][1]["pending_after"] = 1
    with pytest.raises(RuntimeError, match="not drained"):
        module.record(_manifest(), checked_at="2026-09-14T18:00:00Z",
                      runner=_runner(observations))


def test_manifest_requires_bounded_argv_timeout_and_all_gates(tmp_path):
    manifest = _manifest()
    manifest["probes"].pop("rollback")
    path = tmp_path / "manifest.json"
    path.write_text(json.dumps(manifest), encoding="utf-8")
    with pytest.raises(RuntimeError, match="every operational gate"):
        module._load_manifest(path)
    manifest = _manifest()
    manifest["probes"]["rollback"]["timeout_seconds"] = 1801
    path.write_text(json.dumps(manifest), encoding="utf-8")
    with pytest.raises(RuntimeError, match="timeout is invalid"):
        module._load_manifest(path)
