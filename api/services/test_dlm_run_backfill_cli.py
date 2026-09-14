import os
import tempfile
import unittest
from pathlib import Path
from unittest.mock import Mock, patch

from services import dlm_run_backfill_cli as cli


class DlmRunBackfillCliTests(unittest.TestCase):
    def test_apply_fails_before_source_read_when_adls_configuration_is_incomplete(self):
        with tempfile.TemporaryDirectory() as temporary, patch.dict(os.environ, {}, clear=True), \
             patch.object(cli.migration_checkpoint_store, "run") as run, \
             self.assertRaisesRegex(RuntimeError, "checkpoint mode"):
            cli.run(Path(temporary) / "checkpoint.json", Path(temporary), apply=True)
        run.assert_not_called()

    def test_apply_validates_every_required_storage_value(self):
        environment = {"KAVEON_MIGRATION_CHECKPOINT_MODE": "adls",
                       "KAVEON_DLM_RUN_MIGRATION_ENABLED": "true",
                       "KAVEON_DLM_ARTIFACT_PUBLISH_ENABLED": "true"}
        with tempfile.TemporaryDirectory() as temporary, patch.dict(os.environ, environment, clear=True), \
             self.assertRaisesRegex(RuntimeError, "KAVEON_ADLS_ACCOUNT"):
            cli.run(Path(temporary) / "checkpoint.json", Path(temporary), apply=True)

    def test_hydrated_checkpoint_automatically_resumes(self):
        environment = {"KAVEON_MIGRATION_CHECKPOINT_MODE": "adls",
            "KAVEON_DLM_RUN_MIGRATION_ENABLED": "true",
            "KAVEON_DLM_ARTIFACT_PUBLISH_ENABLED": "true",
            "KAVEON_ADLS_ACCOUNT": "artifact-account", "KAVEON_ADLS_CONTAINER": "artifacts",
            "KAVEON_MIGRATION_CHECKPOINT_ADLS_ACCOUNT": "checkpoint-account",
            "KAVEON_MIGRATION_CHECKPOINT_ADLS_CONTAINER": "checkpoints",
            "KAVEON_MIGRATION_CHECKPOINT_ADLS_PREFIX": "migration/run-1"}
        with tempfile.TemporaryDirectory() as temporary:
            checkpoint = Path(temporary) / "checkpoint.json"; root = Path(temporary) / "artifacts"
            publisher = Mock(); client = Mock()
            def durable(operation, path, *, apply, invoke):
                path.write_text("hydrated")
                return invoke(path)
            with patch.dict(os.environ, environment, clear=True), \
                 patch.object(cli.adls_artifact_client.AzureArtifactClient, "from_env", return_value=client), \
                 patch.object(cli.dlm_artifact_publisher, "Publisher", return_value=publisher), \
                 patch.object(cli.migration_checkpoint_store, "run", side_effect=durable), \
                 patch.object(cli.dlm_run_backfill_operation, "run", return_value={"ok": True}) as operation:
                self.assertEqual(cli.run(checkpoint, root, apply=True), {"ok": True})
        self.assertTrue(operation.call_args.kwargs["resume"])
        self.assertIs(operation.call_args.kwargs["publisher"], publisher)

    def test_dry_run_needs_no_cloud_configuration(self):
        with tempfile.TemporaryDirectory() as temporary, patch.dict(os.environ, {}, clear=True), \
             patch.object(cli.migration_checkpoint_store, "run", return_value={"mode": "dry-run"}):
            self.assertEqual(cli.run(Path(temporary) / "c.json", Path(temporary), apply=False)["mode"], "dry-run")


if __name__ == "__main__": unittest.main()
