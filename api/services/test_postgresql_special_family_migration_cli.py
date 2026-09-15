import json
import os
import tempfile
import unittest
from datetime import datetime, timezone
from pathlib import Path
from unittest.mock import patch

from services import postgresql_special_family_migration_cli as cli
from services import postgresql_operational_evidence as operational
from services.test_postgresql_special_family_adls_publisher import Client, payload


class Tests(unittest.TestCase):
    @staticmethod
    def receipt(root):
        checked = datetime.now(timezone.utc).isoformat().replace("+00:00", "Z")
        value = operational.receipt_from_observation("outbox_drain", {
            "query_id": 1, "watermark": 4, "pending_before": 0, "pending_after": 0},
            checked_at=checked, evidence_id="outbox:test")
        path = root / "outbox.json"; path.write_text(json.dumps(value), encoding="utf-8")
        return path

    def test_cli_emits_verified_evidence_without_credentials_in_arguments(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory); source = root / "baseline.json"; output = root / "evidence.json"
            source.write_text(json.dumps(payload()), encoding="utf-8")
            with patch.dict(os.environ, {"KAVEON_SPECIAL_FAMILY_MIGRATION_ENABLED": "true"},
                            clear=True):
                result = cli.main(["--baseline", str(source), "--prefix", "retirement/run-1",
                                   "--expected-head-etag", "absent", "--output", str(output),
                                   "--outbox-drain-receipt", str(self.receipt(root))],
                                  client_factory=Client)
            self.assertTrue(result["passed"])
            evidence = json.loads(output.read_text(encoding="utf-8"))
            self.assertEqual(evidence["manifest"]["status"], "committed")
            self.assertEqual(evidence["operational_observation"]["pending_events"], 0)

    def test_cli_is_disabled_and_refuses_evidence_overwrite(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory); source = root / "baseline.json"; output = root / "evidence.json"
            source.write_text(json.dumps(payload()), encoding="utf-8")
            args = ["--baseline", str(source), "--prefix", "retirement/run-1",
                    "--expected-head-etag", "absent", "--output", str(output),
                    "--outbox-drain-receipt", str(self.receipt(root))]
            with patch.dict(os.environ, {}, clear=True), self.assertRaisesRegex(RuntimeError, "enablement"):
                cli.main(args, client_factory=Client)
            output.write_text("reviewed", encoding="utf-8")
            with patch.dict(os.environ, {"KAVEON_SPECIAL_FAMILY_MIGRATION_ENABLED": "true"},
                            clear=True), self.assertRaisesRegex(RuntimeError, "overwrite"):
                cli.main(args, client_factory=Client)

    def test_evidence_writer_uses_exclusive_azure_files_compatible_create(self):
        with tempfile.TemporaryDirectory() as directory:
            output = Path(directory) / "evidence.json"
            with patch.object(os, "chmod", side_effect=OSError("unsupported")) as chmod, \
                    patch.object(os, "replace", side_effect=OSError("unsupported")) as replace:
                cli._write_new(output, {"passed": True})
            chmod.assert_not_called(); replace.assert_not_called()
            self.assertEqual(json.loads(output.read_text(encoding="utf-8")), {"passed": True})
            with self.assertRaisesRegex(RuntimeError, "overwrite"):
                cli._write_new(output, {"passed": False})


if __name__ == "__main__": unittest.main()
