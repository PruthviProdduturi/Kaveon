import json
import os
import tempfile
import unittest
from pathlib import Path
from unittest.mock import patch

from services import postgresql_special_family_migration_cli as cli
from services.test_postgresql_special_family_adls_publisher import Client, payload


class Tests(unittest.TestCase):
    def test_cli_emits_verified_evidence_without_credentials_in_arguments(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory); source = root / "baseline.json"; output = root / "evidence.json"
            source.write_text(json.dumps(payload()), encoding="utf-8")
            with patch.dict(os.environ, {"KAVEON_SPECIAL_FAMILY_MIGRATION_ENABLED": "true"},
                            clear=True):
                result = cli.main(["--baseline", str(source), "--prefix", "retirement/run-1",
                                   "--expected-head-etag", "absent", "--output", str(output)],
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
                    "--expected-head-etag", "absent", "--output", str(output)]
            with patch.dict(os.environ, {}, clear=True), self.assertRaisesRegex(RuntimeError, "enablement"):
                cli.main(args, client_factory=Client)
            output.write_text("reviewed", encoding="utf-8")
            with patch.dict(os.environ, {"KAVEON_SPECIAL_FAMILY_MIGRATION_ENABLED": "true"},
                            clear=True), self.assertRaisesRegex(RuntimeError, "overwrite"):
                cli.main(args, client_factory=Client)


if __name__ == "__main__": unittest.main()
