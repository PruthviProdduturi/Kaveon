import json
import os
import tempfile
import unittest
from datetime import datetime, timezone
from pathlib import Path
from unittest.mock import patch

from services import postgresql_retirement_gate as gate
from services import postgresql_retirement_runtime as runtime


NOW = datetime(2026, 9, 14, 20, 0, tzinfo=timezone.utc)


def evidence():
    timestamp = "2026-09-14T19:00:00Z"
    families = []
    for family, tables in gate.AUTHORITY_FAMILIES.items():
        entry = {
            "family": family, "tables": list(tables), "status": "passed",
            "reconciled_at": timestamp, "source_watermark": 10,
            "source_count": 0, "target_count": 0,
            "checks": {name: True for name in gate.REQUIRED_CHECKS},
            "provenance": {"producer": "live", "source_snapshot": "pg-1", "target_snapshot": "kv-1"},
            "report_sha256": "a" * 64,
        }
        families.append(entry)
    gates = {
        "source_watermark": {"watermark": 10}, "outbox_drain": {"pending_events": 0},
        "write_fence": {"enabled": True}, "shadow_parity": {"matched": True},
        "restart_recovery": {"verified": True}, "rollback": {"verified": True},
        "backup_identity": {"backup_id": "backup-1", "backup_sha256": "b" * 64, "restore_verified": True},
    }
    return {"schema_version": gate.SCHEMA_VERSION, "families": families, "gates": {
        name: {"status": "passed", "checked_at": timestamp, "evidence_id": name,
               "details": details} for name, details in gates.items()
    }}


class RetirementRuntimeTests(unittest.TestCase):
    def test_default_preserves_postgresql_authority(self):
        with patch.dict(os.environ, {}, clear=True):
            self.assertEqual(runtime.validate(), {"enabled": False, "authority": "postgresql"})

    def test_mode_rejects_incomplete_families_before_reading_evidence(self):
        with patch.dict(os.environ, {runtime.MODE_KEY: "true", runtime.AUTHORITY_KEY: "datasets"}, clear=True):
            with self.assertRaisesRegex(RuntimeError, "incomplete"):
                runtime.validate(now=NOW)

    def test_mode_requires_fresh_passing_evidence_and_engine_configuration(self):
        with tempfile.TemporaryDirectory() as temporary:
            path = Path(temporary) / "evidence.json"
            audit_path = Path(temporary) / "audit.json"
            value = evidence()
            path.write_text(json.dumps(value), encoding="utf-8")
            audit_path.write_text(json.dumps(gate.evaluate(value, now=NOW, max_age_hours=24)), encoding="utf-8")
            environment = {
                runtime.MODE_KEY: "true", runtime.EVIDENCE_KEY: str(path),
                runtime.AUDIT_KEY: str(audit_path),
                runtime.AUTHORITY_KEY: ",".join(gate.AUTHORITY_FAMILIES),
                "KAVEON_ENGINE_URL": "https://engine.example.test",
                "KAVEON_ENGINE_BRIDGE_TOKEN": "test-token",
            }
            with patch.dict(os.environ, environment, clear=True), \
                 patch.object(runtime.engine_bridge, "_verify_context", return_value=True):
                state = runtime.validate(now=NOW)
            self.assertEqual((state["enabled"], state["authority_family_count"]), (True, 16))

            tampered = json.loads(audit_path.read_text())
            tampered["evidence_sha256"] = "0" * 64
            audit_path.write_text(json.dumps(tampered), encoding="utf-8")
            with patch.dict(os.environ, environment, clear=True), \
                 patch.object(runtime.engine_bridge, "_verify_context", return_value=True), \
                 self.assertRaisesRegex(RuntimeError, "does not match"):
                runtime.validate(now=NOW)

    def test_probe_has_no_postgresql_fallback(self):
        state = {"enabled": True, "authority": "kaveondb", "authority_family_count": 16}
        with patch.object(runtime, "validate", return_value=state), \
             patch.object(runtime.product_store, "read", return_value=None) as read:
            self.assertIs(runtime.probe(), state)
        read.assert_called_once_with("dataset", "__kaveon_health__", "kaveon-system", "Admin")


class RetirementStartupTests(unittest.IsolatedAsyncioTestCase):
    async def test_retirement_startup_clears_postgresql_and_skips_warmup(self):
        import main
        from types import SimpleNamespace

        state = {"enabled": True, "authority": "kaveondb", "authority_family_count": 16}
        environment = {"METADATA_DATABASE": "legacy", "METADATA_HOST": "postgres.internal"}
        app = SimpleNamespace(state=SimpleNamespace())
        with patch.dict(os.environ, environment, clear=True), \
             patch.object(main, "start_warmup_and_heartbeat") as warmup, \
             patch.object(runtime, "validate", return_value=state):
            async with main.lifespan(app):
                self.assertEqual(os.environ["METADATA_DATABASE"], "")
                self.assertEqual(os.environ["METADATA_HOST"], "")
                self.assertEqual(app.state.postgresql_retirement, state)
        warmup.assert_not_called()


if __name__ == "__main__":
    unittest.main()
