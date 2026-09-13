"""Static contract tests for the standalone local KaveonDB package path."""

import unittest
from pathlib import Path


ROOT = Path(__file__).resolve().parents[1]


class LocalKaveonDbPackageTests(unittest.TestCase):
    def test_compose_is_engine_only_and_localhost_bound(self):
        compose = (ROOT / "docker-compose.kavedb.yml").read_text(encoding="utf-8")
        self.assertIn("kaveon-db:", compose)
        self.assertIn('127.0.0.1:${KAVEON_DB_PORT:-8080}:8080', compose)
        self.assertIn("/health", compose)
        self.assertNotIn("postgres:", compose.lower())
        self.assertNotIn("METADATA_DB_TYPE", compose)

    def test_wrappers_use_the_standalone_compose_file(self):
        self.assertIn("docker-compose.kavedb.yml", (ROOT / "scripts/kavedb.ps1").read_text())
        self.assertIn("docker-compose.kavedb.yml", (ROOT / "scripts/kavedb.sh").read_text())


if __name__ == "__main__":
    unittest.main()
